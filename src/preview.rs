//! Preview panel content: directory listing, worktree status, or a 2D
//! thumbnail of a live deck composited from `herdr pane read` at the real
//! pane geometry (`herdr pane edges`).

use crate::app::{Entry, EntryKind};
use crate::ext;
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use std::collections::HashMap;
use std::path::Path;

pub fn compute(entry: &Entry, w: u16, h: u16, own_pane: &str) -> Text<'static> {
    match &entry.kind {
        EntryKind::Workspace { id, .. } => thumbnail(id, w as usize, h as usize, own_pane),
        EntryKind::Remote(host) => remote(host),
        EntryKind::Worktree(p) | EntryKind::Cleanable { path: p, .. } => worktree(p),
        EntryKind::Dir(p) => listing(p),
        EntryKind::Session(s) => session(s),
    }
}

fn session(s: &crate::sessions::Session) -> Text<'static> {
    let action = if s.native_picker {
        "opens Cursor's native session picker"
    } else {
        "recreates its project deck and resumes there"
    };
    Text::from(vec![
        Line::from(vec!["agent    ".dim(), s.agent.id().to_string().bold()]),
        Line::from(vec![
            "project  ".dim(),
            Span::raw(ext::collapse_tilde(&s.cwd.to_string_lossy())),
        ]),
        Line::from(vec![
            "updated  ".dim(),
            Span::raw(crate::sessions::age(s.updated)),
        ]),
        Line::from(vec!["session  ".dim(), Span::raw(s.id.clone())]),
        Line::raw(""),
        Line::from(s.title.clone().bold()),
        Line::raw(""),
        Line::from(action.dim()),
    ])
}

fn remote(host: &str) -> Text<'static> {
    Text::from(vec![
        Line::from(vec!["remote  ".dim(), host.to_string().bold()]),
        Line::from(vec![
            "runs    ".dim(),
            Span::raw(format!("herdr --remote {host}")),
        ]),
        Line::raw(""),
        Line::from("opens a thin client in its own Ghostty window;".dim()),
        Line::from("this herdr session is left alone.".dim()),
    ])
}

fn listing(p: &Path) -> Text<'static> {
    let s = p.to_string_lossy();
    let out = ext::out(&["eza", "-la", "--git", "--color=never", &s])
        .or_else(|| ext::out(&["ls", "-la", &s]))
        .unwrap_or_default();
    Text::raw(out)
}

fn worktree(p: &Path) -> Text<'static> {
    let mut lines: Vec<Line> = vec![];
    if let Some(i) = ext::wt_info(p) {
        lines.push(Line::from(vec!["branch  ".dim(), i.branch.clone().bold()]));
        let merged = matches!(i.main_state.as_str(), "integrated" | "empty");
        let state = if merged {
            i.main_state.clone().green()
        } else {
            i.main_state.clone().yellow()
        };
        lines.push(Line::from(vec!["state   ".dim(), state]));
        let mut dirty = vec![];
        if i.staged {
            dirty.push("staged");
        }
        if i.modified {
            dirty.push("modified");
        }
        if i.untracked {
            dirty.push("untracked");
        }
        if i.renamed {
            dirty.push("renamed");
        }
        if i.deleted {
            dirty.push("deleted");
        }
        let dirty = if dirty.is_empty() {
            "clean".to_string()
        } else {
            dirty.join(", ")
        };
        lines.push(Line::from(vec!["tree    ".dim(), Span::raw(dirty)]));
        lines.push(Line::from(vec![
            "vs main ".dim(),
            Span::raw(format!("↑{} ↓{}", i.ahead, i.behind)),
        ]));
        lines.push(Line::raw(""));
    }
    lines.extend(listing(p).lines);
    Text::from(lines)
}

struct PaneRect {
    id: String,
    x: i64,
    y: i64,
    w: i64,
    h: i64,
    focused: bool,
}

/// Remove the temporary picker pane from a self-preview and restore the pane
/// that was split to make room for it. Herdr split siblings share one complete
/// axis and touch on the other, which is enough to reconstruct their union.
fn remove_own_and_grow_sibling(panes: &mut Vec<PaneRect>, own_pane: &str) {
    let Some(index) = panes.iter().position(|pane| pane.id == own_pane) else {
        return;
    };
    let own = panes.remove(index);
    let sibling = panes.iter_mut().find(|pane| {
        let beside = pane.y == own.y
            && pane.h == own.h
            && (pane.x + pane.w == own.x || own.x + own.w == pane.x);
        let stacked = pane.x == own.x
            && pane.w == own.w
            && (pane.y + pane.h == own.y || own.y + own.h == pane.y);
        beside || stacked
    });
    if let Some(sibling) = sibling {
        let x2 = (sibling.x + sibling.w).max(own.x + own.w);
        let y2 = (sibling.y + sibling.h).max(own.y + own.h);
        sibling.x = sibling.x.min(own.x);
        sibling.y = sibling.y.min(own.y);
        sibling.w = x2 - sibling.x;
        sibling.h = y2 - sibling.y;
    }
}

#[derive(Clone, Copy, Default)]
struct Cell {
    ch: char,
    style: Style,
}

#[derive(Clone, Copy, Default)]
struct BorderCell {
    links: u8,
    focused: bool,
}

const UP: u8 = 1;
const DOWN: u8 = 2;
const LEFT: u8 = 4;
const RIGHT: u8 = 8;

fn ansi_color(n: u16, bright: bool) -> Option<Color> {
    Some(match (n, bright) {
        (0, false) => Color::Black,
        (1, false) => Color::Red,
        (2, false) => Color::Green,
        (3, false) => Color::Yellow,
        (4, false) => Color::Blue,
        (5, false) => Color::Magenta,
        (6, false) => Color::Cyan,
        (7, false) => Color::Gray,
        (0, true) => Color::DarkGray,
        (1, true) => Color::LightRed,
        (2, true) => Color::LightGreen,
        (3, true) => Color::LightYellow,
        (4, true) => Color::LightBlue,
        (5, true) => Color::LightMagenta,
        (6, true) => Color::LightCyan,
        (7, true) => Color::White,
        _ => return None,
    })
}

fn apply_sgr(style: &mut Style, params: &[u16]) {
    let params = if params.is_empty() { &[0][..] } else { params };
    let mut i = 0;
    while i < params.len() {
        let p = params[i];
        match p {
            0 => *style = Style::default(),
            1 => style.add_modifier.insert(Modifier::BOLD),
            2 => style.add_modifier.insert(Modifier::DIM),
            3 => style.add_modifier.insert(Modifier::ITALIC),
            4 => style.add_modifier.insert(Modifier::UNDERLINED),
            5 => style.add_modifier.insert(Modifier::SLOW_BLINK),
            6 => style.add_modifier.insert(Modifier::RAPID_BLINK),
            7 => style.add_modifier.insert(Modifier::REVERSED),
            8 => style.add_modifier.insert(Modifier::HIDDEN),
            9 => style.add_modifier.insert(Modifier::CROSSED_OUT),
            22 => style.add_modifier.remove(Modifier::BOLD | Modifier::DIM),
            23 => style.add_modifier.remove(Modifier::ITALIC),
            24 => style.add_modifier.remove(Modifier::UNDERLINED),
            25 => style
                .add_modifier
                .remove(Modifier::SLOW_BLINK | Modifier::RAPID_BLINK),
            27 => style.add_modifier.remove(Modifier::REVERSED),
            28 => style.add_modifier.remove(Modifier::HIDDEN),
            29 => style.add_modifier.remove(Modifier::CROSSED_OUT),
            30..=37 => style.fg = ansi_color(p - 30, false),
            38 | 48 => {
                let foreground = p == 38;
                let color = match params.get(i + 1) {
                    Some(2) if i + 4 < params.len() => {
                        i += 4;
                        Some(Color::Rgb(
                            params[i - 2].min(255) as u8,
                            params[i - 1].min(255) as u8,
                            params[i].min(255) as u8,
                        ))
                    }
                    Some(5) if i + 2 < params.len() => {
                        i += 2;
                        Some(Color::Indexed(params[i].min(255) as u8))
                    }
                    _ => None,
                };
                if foreground {
                    style.fg = color;
                } else {
                    style.bg = color;
                }
            }
            39 => style.fg = None,
            40..=47 => style.bg = ansi_color(p - 40, false),
            49 => style.bg = None,
            90..=97 => style.fg = ansi_color(p - 90, true),
            100..=107 => style.bg = ansi_color(p - 100, true),
            _ => {}
        }
        i += 1;
    }
}

/// Convert SGR-colored terminal output into ratatui cells. In particular,
/// 24-bit `38;2`/`48;2` colors remain RGB colors all the way to the backend.
fn ansi_lines(input: &str) -> Vec<Vec<Cell>> {
    let bytes = input.as_bytes();
    let mut lines = vec![Vec::new()];
    let mut style = Style::default();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b && bytes.get(i + 1) == Some(&b'[') {
            let start = i + 2;
            let mut end = start;
            while end < bytes.len() && !(0x40..=0x7e).contains(&bytes[end]) {
                end += 1;
            }
            if end == bytes.len() {
                break;
            }
            if bytes[end] == b'm' {
                let params: Vec<u16> = input[start..end]
                    .split(';')
                    .map(|s| s.parse().unwrap_or(0))
                    .collect();
                apply_sgr(&mut style, &params);
            }
            i = end + 1;
            continue;
        }
        let ch = input[i..].chars().next().expect("valid UTF-8 boundary");
        i += ch.len_utf8();
        match ch {
            '\n' => lines.push(Vec::new()),
            '\r' => {}
            _ if ch.is_control() => {}
            _ => lines.last_mut().unwrap().push(Cell { ch, style }),
        }
    }
    lines
}

fn non_whitespace(line: &[Cell]) -> usize {
    line.iter().filter(|cell| !cell.ch.is_whitespace()).count()
}

/// Crop a pane into its thumbnail interior without resampling terminal cells.
///
/// Text-mode glyphs cannot be downscaled without distortion. For Neovim-like
/// screens (a dense final statusline), center the body crop on its visual mass
/// and pin the matching statusline slice at the bottom. Other panes are
/// bottom-aligned so recent shell/agent output remains visible.
fn blit_fidelity(
    canvas: &mut [Vec<Cell>],
    content: &[Vec<Cell>],
    x: usize,
    y: usize,
    w: usize,
    h: usize,
) {
    if content.is_empty() || w == 0 || h == 0 {
        return;
    }
    let src_h = content.len();
    let src_w = content.iter().map(Vec::len).max().unwrap_or(0);
    if src_w == 0 {
        return;
    }

    let last_count = non_whitespace(&content[src_h - 1]);
    let last_background = content[src_h - 1]
        .iter()
        .filter(|cell| cell.style.bg.is_some())
        .count();
    let has_statusline = src_h > 1 && last_count * 4 >= src_w && last_background * 2 >= src_w;
    let body_end = src_h - usize::from(has_statusline);
    let body_slots = h - usize::from(has_statusline);
    if body_slots == 0 || body_end == 0 {
        return;
    }

    let body_start = if has_statusline {
        let (weighted_rows, weight): (usize, usize) = content[..body_end]
            .iter()
            .enumerate()
            .map(|(row, line)| {
                let count = non_whitespace(line);
                (row * count, count)
            })
            .fold((0, 0), |(row_sum, count_sum), (row, count)| {
                (row_sum + row, count_sum + count)
            });
        let anchor = weighted_rows.checked_div(weight).unwrap_or(body_end / 2);
        anchor
            .saturating_sub(body_slots.saturating_sub(1) / 2)
            .min(body_end.saturating_sub(body_slots))
    } else {
        body_end.saturating_sub(body_slots)
    };
    let visible_rows = body_slots.min(body_end - body_start);

    let densest = content[body_start..body_start + visible_rows]
        .iter()
        .max_by_key(|line| non_whitespace(line));
    let horizontal_anchor = densest
        .and_then(|line| {
            let left = line.iter().position(|cell| !cell.ch.is_whitespace())?;
            let right = line.iter().rposition(|cell| !cell.ch.is_whitespace())?;
            Some((left + right) / 2)
        })
        .unwrap_or(src_w / 2);
    let source_x = horizontal_anchor
        .saturating_sub(w.saturating_sub(1) / 2)
        .min(src_w.saturating_sub(w));
    let destination_x = x + w.saturating_sub(src_w) / 2;

    let copy_row = |canvas: &mut [Vec<Cell>], source: &[Cell], dy: usize, sx: usize| {
        for (dx, cell) in source.iter().skip(sx).take(w).enumerate() {
            if let Some(row) = canvas.get_mut(y + dy)
                && let Some(dst) = row.get_mut(destination_x + dx)
            {
                *dst = *cell;
            }
        }
    };
    for dy in 0..visible_rows {
        copy_row(canvas, &content[body_start + dy], dy, source_x);
    }
    if has_statusline {
        copy_row(canvas, &content[src_h - 1], h - 1, source_x);
    }
}

fn connect_horizontal(borders: &mut [Vec<BorderCell>], x: usize, y: usize, focused: bool) {
    if y >= borders.len() || x + 1 >= borders[y].len() {
        return;
    }
    borders[y][x].links |= RIGHT;
    borders[y][x + 1].links |= LEFT;
    borders[y][x].focused |= focused;
    borders[y][x + 1].focused |= focused;
}

fn connect_vertical(borders: &mut [Vec<BorderCell>], x: usize, y: usize, focused: bool) {
    if y + 1 >= borders.len() || x >= borders[y].len() || x >= borders[y + 1].len() {
        return;
    }
    borders[y][x].links |= DOWN;
    borders[y + 1][x].links |= UP;
    borders[y][x].focused |= focused;
    borders[y + 1][x].focused |= focused;
}

fn border_char(links: u8) -> char {
    match links {
        12 => '─', // left + right
        3 => '│',  // up + down
        10 => '┌', // down + right
        6 => '┐',  // down + left
        9 => '└',  // up + right
        5 => '┘',  // up + left
        11 => '├', // up + down + right
        7 => '┤',  // up + down + left
        14 => '┬', // down + left + right
        13 => '┴', // up + left + right
        15 => '┼',
        1 | 2 => '│',
        4 | 8 => '─',
        _ => ' ',
    }
}

fn thumbnail(ws: &str, pw: usize, ph: usize, own_pane: &str) -> Text<'static> {
    let (pw, ph) = (pw.max(24), ph.max(8));
    let Some(pl) = ext::json(&["herdr", "pane", "list", "--workspace", ws]) else {
        return Text::raw("(herdr unavailable)");
    };
    let Some(first) = pl["result"]["panes"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|p| p["pane_id"].as_str())
        .map(String::from)
    else {
        return Text::raw("(workspace has no panes)");
    };
    let mut panes: Vec<PaneRect> = ext::json(&["herdr", "pane", "edges", "--pane", &first])
        .and_then(|v| {
            v["result"]["edges"]["layout"]["panes"].as_array().map(|a| {
                a.iter()
                    .map(|p| PaneRect {
                        id: p["pane_id"].as_str().unwrap_or("").into(),
                        x: p["rect"]["x"].as_i64().unwrap_or(0),
                        y: p["rect"]["y"].as_i64().unwrap_or(0),
                        w: p["rect"]["width"].as_i64().unwrap_or(0),
                        h: p["rect"]["height"].as_i64().unwrap_or(0),
                        focused: p["focused"].as_bool().unwrap_or(false),
                    })
                    .collect()
            })
        })
        .unwrap_or_default();
    panes.retain(|pane| !pane.id.is_empty());
    remove_own_and_grow_sibling(&mut panes, own_pane);
    if panes.is_empty() {
        return Text::raw("(only this pane)");
    }

    let texts: HashMap<String, Vec<Vec<Cell>>> = panes
        .iter()
        .map(|p| {
            let t = ext::out(&[
                "herdr", "pane", "read", &p.id, "--source", "visible", "--format", "ansi",
            ])
            .unwrap_or_default();
            (p.id.clone(), ansi_lines(&t))
        })
        .collect();

    // Rects are absolute screen coords; normalize to the bounding box, then
    // scale onto the pw × ph canvas.
    let x0 = panes.iter().map(|p| p.x).min().unwrap_or(0);
    let y0 = panes.iter().map(|p| p.y).min().unwrap_or(0);
    let ww = panes.iter().map(|p| p.x + p.w).max().unwrap_or(1) - x0;
    let wh = panes.iter().map(|p| p.y + p.h).max().unwrap_or(1) - y0;
    let (ww, wh) = (ww.max(1) as f64, wh.max(1) as f64);
    let xm = |x: i64| (((x - x0) as f64 / ww) * pw as f64).round() as usize;
    let ym = |y: i64| (((y - y0) as f64 / wh) * ph as f64).round() as usize;

    let mut canvas = vec![
        vec![
            Cell {
                ch: ' ',
                style: Style::default()
            };
            pw
        ];
        ph
    ];
    let mut borders = vec![vec![BorderCell::default(); pw]; ph];
    for p in &panes {
        let (cx, cx2, cy, cy2) = (xm(p.x), xm(p.x + p.w), ym(p.y), ym(p.y + p.h));
        // Rect endpoints are pane boundaries, so adjacent panes must use the
        // same scaled coordinate. This gives shared edges one column/row and
        // lets the connection grid form real T and cross junctions.
        let (left, right) = (cx.min(pw - 1), cx2.min(pw - 1));
        let (top, bottom) = (cy.min(ph - 1), cy2.min(ph - 1));
        if right <= left || bottom <= top {
            continue;
        }

        for x in left..right {
            connect_horizontal(&mut borders, x, top, p.focused);
            connect_horizontal(&mut borders, x, bottom, p.focused);
        }
        for y in top..bottom {
            connect_vertical(&mut borders, left, y, p.focused);
            connect_vertical(&mut borders, right, y, p.focused);
        }

        blit_fidelity(
            &mut canvas,
            &texts[&p.id],
            left + 1,
            top + 1,
            right.saturating_sub(left + 1),
            bottom.saturating_sub(top + 1),
        );
    }
    for (y, row) in borders.iter().enumerate() {
        for (x, border) in row.iter().enumerate() {
            if border.links != 0 {
                canvas[y][x] = Cell {
                    ch: border_char(border.links),
                    style: if border.focused {
                        Style::new().yellow()
                    } else {
                        Style::new().dark_gray()
                    },
                };
            }
        }
    }

    let lines: Vec<Line> = canvas
        .iter()
        .map(|row| {
            let mut spans: Vec<Span> = vec![];
            let mut buf = String::new();
            let mut cur = row.first().map(|c| c.style).unwrap_or_default();
            for cell in row {
                if cell.style != cur {
                    spans.push(Span::styled(std::mem::take(&mut buf), cur));
                    buf.clear();
                    cur = cell.style;
                }
                buf.push(cell.ch);
            }
            spans.push(Span::styled(buf, cur));
            Line::from(spans)
        })
        .collect();
    Text::from(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_true_color_and_resets_it() {
        let lines = ansi_lines("\x1b[38;2;12;34;56mR\x1b[39mN");
        assert_eq!(lines[0][0].style.fg, Some(Color::Rgb(12, 34, 56)));
        assert_eq!(lines[0][1].style.fg, None);
    }

    #[test]
    fn parses_background_and_256_color() {
        let lines = ansi_lines("\x1b[48;2;1;2;3mB\x1b[38;5;200mI");
        assert_eq!(lines[0][0].style.bg, Some(Color::Rgb(1, 2, 3)));
        assert_eq!(lines[0][1].style.fg, Some(Color::Indexed(200)));
        assert_eq!(lines[0][1].style.bg, Some(Color::Rgb(1, 2, 3)));
    }

    #[test]
    fn preserves_modifiers_across_colors() {
        let lines = ansi_lines("\x1b[1;31mB\x1b[22;0mN");
        assert!(lines[0][0].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(lines[0][0].style.fg, Some(Color::Red));
        assert_eq!(lines[0][1].style, Style::default());
    }

    #[test]
    fn crops_cells_verbatim_and_pins_a_statusline() {
        let background = Style::default().bg(Color::Black);
        let content: Vec<Vec<Cell>> = ["      ", " ABCD ", " EFGH ", "      ", "STATUS"]
            .into_iter()
            .enumerate()
            .map(|(row, line)| {
                line.chars()
                    .map(|ch| Cell {
                        ch,
                        style: if row == 4 {
                            background
                        } else {
                            Style::default()
                        },
                    })
                    .collect()
            })
            .collect();
        let mut canvas = vec![vec![Cell::default(); 4]; 3];
        blit_fidelity(&mut canvas, &content, 0, 0, 4, 3);

        assert_eq!(canvas[0].iter().map(|c| c.ch).collect::<String>(), "ABCD");
        assert_eq!(canvas[1].iter().map(|c| c.ch).collect::<String>(), "EFGH");
        assert_eq!(canvas[2].iter().map(|c| c.ch).collect::<String>(), "TATU");
        assert_eq!(canvas[2][0].style.bg, Some(Color::Black));
    }

    #[test]
    fn self_preview_removes_picker_and_regrows_its_sibling() {
        let pane = |id: &str, x, y, w, h| PaneRect {
            id: id.to_string(),
            x,
            y,
            w,
            h,
            focused: false,
        };
        let mut panes = vec![
            pane("nvim", 0, 0, 70, 60),
            pane("picker", 70, 0, 30, 60),
            pane("terminal", 0, 60, 100, 20),
        ];

        remove_own_and_grow_sibling(&mut panes, "picker");

        assert_eq!(panes.len(), 2);
        let nvim = panes.iter().find(|pane| pane.id == "nvim").unwrap();
        assert_eq!((nvim.x, nvim.y, nvim.w, nvim.h), (0, 0, 100, 60));
    }

    #[test]
    fn renders_merged_pane_border_junctions() {
        assert_eq!(border_char(DOWN | RIGHT), '┌');
        assert_eq!(border_char(UP | DOWN | RIGHT), '├');
        assert_eq!(border_char(UP | DOWN | LEFT | RIGHT), '┼');
    }
}
