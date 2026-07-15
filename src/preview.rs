//! Preview panel content: directory listing, worktree status, or a 2D
//! thumbnail of a live cockpit composited from `herdr pane read` at the real
//! pane geometry (`herdr pane edges`). Port of _nic_preview, minus the
//! picker-sibling reflow.

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
        EntryKind::Worktree(p) => worktree(p),
        EntryKind::Dir(p) => listing(p),
    }
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
}

#[derive(Clone, Copy, Default)]
struct Cell {
    ch: char,
    style: Style,
}

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
                    })
                    .collect()
            })
        })
        .unwrap_or_default();
    // ponytail: dropping our own pane leaves a blank gap in the thumbnail;
    // sibling-grow reflow (see _nic_preview) if it ever bothers you
    panes.retain(|p| p.id != own_pane && !p.id.is_empty());
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
    for p in &panes {
        let (cx, cx2, cy, cy2) = (xm(p.x), xm(p.x + p.w), ym(p.y), ym(p.y + p.h));
        let (cw, ch) = (cx2.saturating_sub(cx), cy2.saturating_sub(cy));
        if cw == 0 || ch == 0 {
            continue;
        }
        let mut content: &[Vec<Cell>] = &texts[&p.id];
        while content
            .last()
            .is_some_and(|l| l.iter().all(|c| c.ch.is_whitespace()))
        {
            content = &content[..content.len() - 1];
        }
        for (ri, ln) in content.iter().take(ch).enumerate() {
            if cy + ri >= ph {
                break;
            }
            for (ci, cell) in ln.iter().take(cw).enumerate() {
                if cx + ci < pw {
                    canvas[cy + ri][cx + ci] = *cell;
                }
            }
        }
    }
    // Dim separators at internal edges.
    for p in &panes {
        let (cx, cx2, cy, cy2) = (xm(p.x), xm(p.x + p.w), ym(p.y), ym(p.y + p.h));
        if cx > 0 && cx < pw {
            for row in canvas.iter_mut().take(cy2.min(ph)).skip(cy) {
                row[cx] = Cell {
                    ch: '│',
                    style: Style::new().dark_gray(),
                };
            }
        }
        if cy > 0 && cy < ph {
            for cell in canvas[cy].iter_mut().take(cx2.min(pw)).skip(cx) {
                *cell = Cell {
                    ch: '─',
                    style: Style::new().dark_gray(),
                };
            }
            if cx > 0 && cx < pw {
                canvas[cy][cx].ch = '┼';
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
}
