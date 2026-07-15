//! Preview panel content: directory listing, worktree status, or a 2D
//! thumbnail of a live cockpit composited from `herdr pane read` at the real
//! pane geometry (`herdr pane edges`). Port of _nic_preview, minus the ANSI
//! cell parsing and picker-sibling reflow.

use crate::app::{Entry, EntryKind};
use crate::ext;
use ratatui::style::Stylize;
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
        Line::from(vec!["runs    ".dim(), Span::raw(format!("herdr --remote {host}"))]),
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
        let dirty = if dirty.is_empty() { "clean".to_string() } else { dirty.join(", ") };
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

    let texts: HashMap<String, String> = panes
        .iter()
        .map(|p| {
            let t = ext::out(&[
                "herdr", "pane", "read", &p.id, "--source", "visible", "--format", "text",
            ])
            .unwrap_or_default();
            (p.id.clone(), t)
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

    let mut canvas = vec![vec![' '; pw]; ph];
    let mut sep = vec![vec![false; pw]; ph];
    for p in &panes {
        let (cx, cx2, cy, cy2) = (xm(p.x), xm(p.x + p.w), ym(p.y), ym(p.y + p.h));
        let (cw, ch) = (cx2.saturating_sub(cx), cy2.saturating_sub(cy));
        if cw == 0 || ch == 0 {
            continue;
        }
        let mut content: Vec<&str> = texts[&p.id].lines().collect();
        while content.last().is_some_and(|l| l.trim().is_empty()) {
            content.pop();
        }
        for (ri, ln) in content.iter().take(ch).enumerate() {
            if cy + ri >= ph {
                break;
            }
            for (ci, chr) in ln.chars().take(cw).enumerate() {
                if cx + ci < pw {
                    canvas[cy + ri][cx + ci] = chr;
                }
            }
        }
    }
    // Dim separators at internal edges.
    for p in &panes {
        let (cx, cx2, cy, cy2) = (xm(p.x), xm(p.x + p.w), ym(p.y), ym(p.y + p.h));
        if cx > 0 && cx < pw {
            for row in canvas.iter_mut().take(cy2.min(ph)).skip(cy) {
                row[cx] = '│';
            }
            for row in sep.iter_mut().take(cy2.min(ph)).skip(cy) {
                row[cx] = true;
            }
        }
        if cy > 0 && cy < ph {
            for ci in cx..cx2.min(pw) {
                canvas[cy][ci] = '─';
                sep[cy][ci] = true;
            }
            if cx > 0 && cx < pw {
                canvas[cy][cx] = '┼';
            }
        }
    }

    let lines: Vec<Line> = canvas
        .iter()
        .zip(sep.iter())
        .map(|(row, srow)| {
            let mut spans: Vec<Span> = vec![];
            let mut buf = String::new();
            let mut cur = srow.first().copied().unwrap_or(false);
            for (chr, &is_sep) in row.iter().zip(srow.iter()) {
                if is_sep != cur {
                    spans.push(if cur { buf.clone().dim() } else { Span::raw(buf.clone()) });
                    buf.clear();
                    cur = is_sep;
                }
                buf.push(*chr);
            }
            spans.push(if cur { buf.clone().dim() } else { Span::raw(buf) });
            Line::from(spans)
        })
        .collect();
    Text::from(lines)
}
