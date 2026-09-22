//! Rendering for the picker, preview, command header, and modal overlays.
//!
//! The structure follows Herdr's native UI: plain accent borders, full-row
//! selection, dimmed modal backdrops, and flat action buttons. Chrome uses the
//! active Herdr theme, including custom color overrides.

use crate::app::{
    self, App, DelAction, Entry, EntryKind, HitRegion, HitTarget, LaunchForm, LaunchTarget, Mode,
    Source,
};
use crate::ext;
use crate::theme::Palette;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, List, ListItem, ListState, Paragraph, Wrap};

const NAV_MIN: u16 = 28;
const NAV_MAX: u16 = 52;

fn navigator_width(width: u16) -> u16 {
    (width / 3)
        .clamp(NAV_MIN, NAV_MAX)
        .min(width.saturating_sub(20))
}

/// Inner size of the preview panel for a given terminal size. This stays next
/// to the layout so preview rendering and computation agree.
pub fn preview_dims(w: u16, h: u16) -> (u16, u16) {
    let right = w.saturating_sub(navigator_width(w)).saturating_sub(1);
    (right.saturating_sub(2), h.saturating_sub(4))
}

fn panel(title: Line<'static>, active: bool, palette: &Palette) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Plain)
        .border_style(Style::new().fg(if active {
            palette.accent
        } else {
            palette.overlay0
        }))
        .style(Style::new().bg(palette.panel_bg).fg(palette.text))
        .title(title)
}

fn source_name(app: &App) -> String {
    match (app.source, app.session_agent) {
        (Source::Projects, _) => "projects".into(),
        (Source::Sessions, Some(agent)) => format!("sessions · {}", agent.id()),
        (Source::Sessions, None) => "sessions".into(),
        (Source::Cleanup, _) => "cleanable".into(),
    }
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let palette = &app.palette;
    let area = f.area();
    f.render_widget(
        Block::new().style(Style::new().bg(palette.panel_bg).fg(palette.text)),
        area,
    );

    let mut hits = Vec::new();
    let [header, main, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(area);
    draw_header(f, header, app, &mut hits);

    let [left, _, right] = Layout::horizontal([
        Constraint::Length(navigator_width(area.width)),
        Constraint::Length(1),
        Constraint::Min(1),
    ])
    .areas(main);
    let position = if app.filtered.is_empty() {
        0
    } else {
        app.selected + 1
    };
    let navigator = panel(
        Line::from(vec![
            Span::styled(
                format!(" {} ", source_name(app)),
                Style::new().fg(palette.accent).bold(),
            ),
            Span::styled(
                format!("{position}/{} ", app.filtered.len()),
                Style::new().fg(palette.overlay0),
            ),
        ]),
        true,
        palette,
    );
    let navigator_inner = navigator.inner(left);
    f.render_widget(navigator, left);
    let [search_area, list_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(navigator_inner);

    let query = if app.filter.is_empty() && !app.searching {
        Span::styled("search", Style::new().fg(palette.overlay0))
    } else {
        Span::styled(app.filter.clone(), Style::new().fg(palette.text))
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " / ",
                Style::new()
                    .fg(if app.searching {
                        palette.accent
                    } else {
                        palette.overlay0
                    })
                    .bold(),
            ),
            query,
            Span::styled(
                if app.searching { "█" } else { "" },
                Style::new().fg(palette.accent),
            ),
        ]))
        .style(Style::new().bg(if app.searching {
            palette.surface0
        } else {
            palette.panel_bg
        })),
        search_area,
    );
    hits.push(HitRegion::new(search_area, HitTarget::Search));

    let mut items: Vec<ListItem> = app
        .filtered
        .iter()
        .map(|&index| ListItem::new(entry_line(&app.entries[index], &app.filter, palette)))
        .collect();
    if items.is_empty() {
        let message = if app.source == Source::Cleanup && app.cleanup_loading {
            "  scanning repositories…"
        } else {
            "  no matches"
        };
        items.push(ListItem::new(Line::from(Span::styled(
            message,
            Style::new().fg(palette.overlay0).dim(),
        ))));
    }
    let selected = (!app.filtered.is_empty()).then_some(app.selected);
    let mut list_state = ListState::default().with_selected(selected);
    f.render_stateful_widget(
        List::new(items)
            .highlight_symbol("› ")
            .style(Style::new().fg(palette.text).bg(palette.panel_bg))
            .highlight_style(Style::new().bg(palette.surface1).bold()),
        list_area,
        &mut list_state,
    );
    hits.push(HitRegion::new(list_area, HitTarget::Results));
    for visible_row in 0..list_area.height as usize {
        let filtered_index = list_state.offset() + visible_row;
        if filtered_index >= app.filtered.len() {
            break;
        }
        hits.push(HitRegion::new(
            Rect::new(
                list_area.x,
                list_area.y + visible_row as u16,
                list_area.width,
                1,
            ),
            HitTarget::Result(filtered_index),
        ));
    }

    let preview_title = match app.selected_entry() {
        Some(entry) => format!(" {} ", entry.label),
        None => " preview ".into(),
    };
    let preview_block = panel(
        Line::from(Span::styled(preview_title, Style::new().fg(palette.accent))),
        false,
        palette,
    );
    let preview_inner = preview_block.inner(right);
    f.render_widget(preview_block, right);
    if let Some(entry) = app.selected_entry() {
        match app.preview.get(&entry.cache_key()) {
            Some(text) => f.render_widget(Paragraph::new(text.clone()), preview_inner),
            None => f.render_widget(
                Paragraph::new("…").style(Style::new().fg(palette.overlay0).dim()),
                preview_inner,
            ),
        }
    }

    draw_footer(f, footer, app);

    if !matches!(app.mode, Mode::List) {
        // A modal owns input. Underlying controls remain visible but cannot be
        // activated through the dimmed backdrop.
        hits.clear();
    }
    match &app.mode {
        Mode::List => {}
        Mode::Launch(form) => draw_launch(f, area, form, &app.agents, app, &mut hits),
        Mode::NewPath { input } => draw_new_path(f, area, input, app, &mut hits),
        Mode::ConfirmDelete { msg, action } => draw_confirm(f, area, msg, action, app, &mut hits),
        Mode::Help => draw_help(f, area, app, &mut hits),
    }

    app.hit_regions = hits;
}

fn draw_header(f: &mut Frame, area: Rect, app: &App, hits: &mut Vec<HitRegion>) {
    let end = area.x.saturating_add(area.width);
    let mut x = area.x;
    for (label, target, active) in [
        (
            " 1 projects ",
            HitTarget::Source(Source::Projects),
            app.source == Source::Projects,
        ),
        (
            " 2 sessions ",
            HitTarget::Source(Source::Sessions),
            app.source == Source::Sessions,
        ),
        (
            " 3 cleanable ",
            HitTarget::Source(Source::Cleanup),
            app.source == Source::Cleanup,
        ),
    ] {
        x = draw_inline_button(f, area.y, x, end, label, target, active, app, hits);
    }

    x = x.saturating_add(1);
    let mut actions = Vec::new();
    if app.selected_entry().and_then(Entry::launch_dir).is_some() {
        actions.push((" ^o cockpit ", HitTarget::NewCockpit));
    }
    match app.source {
        Source::Projects => {
            actions.push((" ^n mkdir ", HitTarget::NewPath));
            actions.push((" ^d delete ", HitTarget::Delete));
        }
        Source::Sessions => actions.push((" tab agent ", HitTarget::CycleAgent)),
        Source::Cleanup => {
            actions.push((" ^d delete ", HitTarget::Delete));
            actions.push((" ^x remove all ", HitTarget::RemoveAll));
        }
    }
    actions.extend([
        (" ^r reload ", HitTarget::Reload),
        (" ? help ", HitTarget::Help),
        (" q close ", HitTarget::Quit),
    ]);
    for (label, target) in actions {
        x = draw_inline_button(f, area.y, x, end, label, target, false, app, hits);
    }
}

fn draw_footer(f: &mut Frame, area: Rect, app: &App) {
    let palette = &app.palette;
    if let Some(status) = &app.status
        && !matches!(app.mode, Mode::Launch(_))
    {
        let color = if status.error {
            palette.red
        } else {
            palette.yellow
        };
        f.render_widget(
            Paragraph::new(format!(" {}", status.msg)).style(Style::new().fg(color)),
            area,
        );
        return;
    }
    if app.source == Source::Cleanup && app.cleanup_loading {
        f.render_widget(
            Paragraph::new(" scanning repositories; results appear as they are found")
                .style(Style::new().fg(palette.yellow)),
            area,
        );
        return;
    }

    let mut hints = if app.searching {
        vec![
            ("type", "filter"),
            ("↑↓/^j^k", "move"),
            ("↵", "open"),
            ("^u", "clear"),
            ("esc", "navigation"),
        ]
    } else {
        vec![
            ("j/k", "move"),
            ("h/l", "source"),
            ("g/G", "first/last"),
            ("/", "search"),
            ("↵", "open"),
        ]
    };
    if !app.searching {
        match app.source {
            Source::Projects => {
                hints.extend([("^o", "cockpit"), ("^n", "mkdir"), ("^d", "delete")])
            }
            Source::Sessions => hints.push(("tab", "agent")),
            Source::Cleanup => hints.extend([("^d", "delete"), ("^x", "remove all")]),
        }
        hints.extend([("?", "help"), ("q", "close")]);
    }
    let mut spans = vec![Span::styled(
        if app.searching { " SEARCH  " } else { " NAV  " },
        Style::new().fg(palette.accent).bold(),
    )];
    for (key, description) in hints {
        spans.push(Span::styled(key, Style::new().fg(palette.text).bold()));
        spans.push(Span::styled(
            format!(" {description}  "),
            Style::new().fg(palette.overlay0),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

#[allow(clippy::too_many_arguments)]
fn draw_inline_button(
    f: &mut Frame,
    y: u16,
    x: u16,
    end: u16,
    label: &str,
    target: HitTarget,
    active: bool,
    app: &App,
    hits: &mut Vec<HitRegion>,
) -> u16 {
    let width = label.chars().count() as u16;
    if x.saturating_add(width) > end {
        return x;
    }
    let rect = Rect::new(x, y, width, 1);
    let hovered = app.hovered.as_ref() == Some(&target);
    let palette = &app.palette;
    let style = if active {
        Style::new()
            .fg(palette.contrast_fg())
            .bg(palette.accent)
            .bold()
    } else if hovered {
        Style::new().fg(palette.text).bg(palette.surface1).bold()
    } else {
        Style::new().fg(palette.overlay0)
    };
    f.render_widget(Paragraph::new(label).style(style), rect);
    hits.push(HitRegion::new(rect, target));
    x.saturating_add(width)
}

fn label_spans(label: &str, filter: &str, palette: &Palette) -> Vec<Span<'static>> {
    let indices = if filter.is_empty() {
        None
    } else {
        app::match_indices(label, filter)
    };
    let Some(indices) = indices else {
        return vec![Span::raw(label.to_string())];
    };
    let mut spans = Vec::new();
    let mut buffer = String::new();
    let mut matches = indices.iter().copied().peekable();
    let mut highlighted = false;
    for (index, character) in label.chars().enumerate() {
        let hit = matches.peek() == Some(&index);
        if hit {
            matches.next();
        }
        if hit != highlighted && !buffer.is_empty() {
            spans.push(if highlighted {
                Span::styled(
                    std::mem::take(&mut buffer),
                    Style::new().fg(palette.accent).bold(),
                )
            } else {
                Span::raw(std::mem::take(&mut buffer))
            });
        }
        highlighted = hit;
        buffer.push(character);
    }
    if !buffer.is_empty() {
        spans.push(if highlighted {
            Span::styled(buffer, Style::new().fg(palette.accent).bold())
        } else {
            Span::raw(buffer)
        });
    }
    spans
}

fn entry_line(entry: &Entry, filter: &str, palette: &Palette) -> Line<'static> {
    let mut spans = match &entry.kind {
        EntryKind::Workspace { status, .. } => {
            let dot = match status.as_str() {
                "working" => Span::styled("●", Style::new().fg(palette.yellow)),
                "blocked" => Span::styled("●", Style::new().fg(palette.red)),
                "done" => Span::styled("●", Style::new().fg(palette.green)),
                _ => Span::styled("●", Style::new().fg(palette.accent)),
            };
            vec![dot, Span::raw(" ")]
        }
        EntryKind::Remote(_) => vec![Span::styled("⇄ ", Style::new().fg(palette.accent))],
        EntryKind::Cleanable { clean, .. } => vec![if *clean {
            Span::styled("✓ ", Style::new().fg(palette.green))
        } else {
            Span::styled("! ", Style::new().fg(palette.yellow))
        }],
        EntryKind::Session(session) => {
            let icon = match session.agent {
                crate::sessions::Agent::Claude => "C",
                crate::sessions::Agent::Codex => "X",
                crate::sessions::Agent::Cursor => "↗",
                crate::sessions::Agent::Pi => "π",
            };
            vec![Span::styled(
                format!("{icon} "),
                Style::new().fg(palette.accent),
            )]
        }
        _ => vec![Span::styled("▸ ", Style::new().fg(palette.overlay0))],
    };
    spans.extend(label_spans(&entry.label, filter, palette));
    Line::from(spans)
}

fn centered(area: Rect, width: u16, height: u16) -> Option<Rect> {
    let width = width.min(area.width.saturating_sub(4));
    let height = height.min(area.height.saturating_sub(2));
    if width < 4 || height < 4 {
        return None;
    }
    Some(Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    ))
}

fn dim_background(f: &mut Frame, area: Rect) {
    let buffer = f.buffer_mut();
    for y in area.y..area.y.saturating_add(area.height) {
        for x in area.x..area.x.saturating_add(area.width) {
            let cell = &mut buffer[(x, y)];
            cell.set_style(cell.style().add_modifier(Modifier::DIM));
        }
    }
}

fn modal_shell(
    f: &mut Frame,
    area: Rect,
    width: u16,
    height: u16,
    border: Color,
    palette: &Palette,
    hits: &mut Vec<HitRegion>,
) -> Option<(Rect, Rect)> {
    dim_background(f, area);
    let popup = centered(area, width, height)?;
    f.render_widget(Clear, popup);
    let block = Block::bordered()
        .border_type(BorderType::Plain)
        .border_style(Style::new().fg(border))
        .style(Style::new().bg(palette.panel_bg).fg(palette.text));
    let inner = block.inner(popup);
    f.render_widget(block, popup);
    hits.push(HitRegion::new(popup, HitTarget::ModalSurface));
    Some((popup, inner))
}

fn draw_button(
    f: &mut Frame,
    rect: Rect,
    label: &str,
    target: HitTarget,
    primary_color: Option<Color>,
    app: &App,
    hits: &mut Vec<HitRegion>,
) {
    let hovered = app.hovered.as_ref() == Some(&target);
    let palette = &app.palette;
    let background = if let Some(color) = primary_color {
        color
    } else if hovered {
        palette.surface1
    } else {
        palette.surface0
    };
    let foreground = if primary_color.is_some() {
        palette.contrast_fg()
    } else {
        palette.text
    };
    f.render_widget(
        Paragraph::new(label)
            .alignment(Alignment::Center)
            .style(Style::new().fg(foreground).bg(background).bold()),
        rect,
    );
    hits.push(HitRegion::new(rect, target));
}

fn button_row(inner: Rect, labels: &[&str]) -> Vec<Rect> {
    let widths: Vec<u16> = labels
        .iter()
        .map(|label| label.chars().count() as u16 + 2)
        .collect();
    let total = widths.iter().sum::<u16>() + widths.len().saturating_sub(1) as u16 * 2;
    let mut x = inner.x + inner.width.saturating_sub(total) / 2;
    widths
        .into_iter()
        .map(|width| {
            let available = inner.x.saturating_add(inner.width).saturating_sub(x);
            let rect = Rect::new(
                x,
                inner.y + inner.height.saturating_sub(1),
                width.min(available),
                1,
            );
            x = x.saturating_add(width).saturating_add(2);
            rect
        })
        .collect()
}

#[derive(Clone)]
struct AgentOption {
    index: usize,
    text: String,
}

fn agent_option_rows(agents: &[String], selected: usize, width: usize) -> Vec<Vec<AgentOption>> {
    let mut rows = vec![Vec::new()];
    let mut used = 0;
    for (index, name) in agents
        .iter()
        .map(String::as_str)
        .chain(["none"])
        .enumerate()
    {
        let text = format!(
            " {} {} ",
            if selected == index { "(●)" } else { "( )" },
            name
        );
        let item_width = text.chars().count();
        if used + item_width > width && used > 0 {
            rows.push(Vec::new());
            used = 0;
        }
        rows.last_mut().unwrap().push(AgentOption { index, text });
        used += item_width;
    }
    rows
}

const LAUNCH_W: u16 = 68;

fn draw_launch(
    f: &mut Frame,
    area: Rect,
    form: &LaunchForm,
    agents: &[String],
    app: &App,
    hits: &mut Vec<HitRegion>,
) {
    let palette = &app.palette;
    let option_rows = agent_option_rows(agents, form.agent, LAUNCH_W.saturating_sub(4) as usize);
    let worktree = form.target == LaunchTarget::Worktree;
    let height = (if worktree { 21 } else { 13 }) + option_rows.len() as u16;
    let Some((_, inner)) = modal_shell(f, area, LAUNCH_W, height, palette.accent, palette, hits)
    else {
        return;
    };
    f.render_widget(
        Paragraph::new(format!(
            " new cockpit: {}",
            ext::collapse_tilde(&form.dir.to_string_lossy())
        ))
        .style(Style::new().fg(palette.text).bold()),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    if let Some(status) = &app.status {
        f.render_widget(
            Paragraph::new(format!(" ! {}", status.msg)).style(Style::new().fg(if status.error {
                palette.red
            } else {
                palette.yellow
            })),
            Rect::new(inner.x, inner.y + 1, inner.width, 1),
        );
    }

    let mut y = inner.y + 2;
    f.render_widget(
        Paragraph::new(" location").style(Style::new().fg(if form.field == 0 {
            palette.accent
        } else {
            palette.overlay0
        })),
        Rect::new(inner.x, y, inner.width, 1),
    );
    y += 1;
    let mut x = inner.x + 1;
    for (target, selected, label) in [
        (
            HitTarget::LaunchSameCheckout,
            form.target == LaunchTarget::SameCheckout,
            "same checkout",
        ),
        (
            HitTarget::LaunchWorktree,
            form.target == LaunchTarget::Worktree,
            "worktree…",
        ),
    ] {
        let text = format!(" {} {label} ", if selected { "(●)" } else { "( )" });
        let width = text.chars().count() as u16;
        let rect = Rect::new(x, y, width, 1);
        let hovered = app.hovered.as_ref() == Some(&target);
        let style = if selected {
            Style::new()
                .fg(palette.contrast_fg())
                .bg(palette.accent)
                .bold()
        } else if hovered {
            Style::new().fg(palette.text).bg(palette.surface1)
        } else {
            Style::new().fg(palette.subtext0)
        };
        f.render_widget(Paragraph::new(text).style(style), rect);
        hits.push(HitRegion::new(rect, target));
        x += width;
    }
    y += 1;
    let location_help = if worktree {
        " choose an existing worktree or type a branch to create one"
    } else {
        " shares files and Git state; editor, agent, and shell are new"
    };
    f.render_widget(
        Paragraph::new(location_help).style(Style::new().fg(palette.overlay0)),
        Rect::new(inner.x, y, inner.width, 1),
    );
    y += 2;

    f.render_widget(
        Paragraph::new(" agent").style(Style::new().fg(if form.field == 1 {
            palette.accent
        } else {
            palette.overlay0
        })),
        Rect::new(inner.x, y, inner.width, 1),
    );
    y += 1;
    for row in option_rows {
        let mut x = inner.x + 1;
        for option in row {
            let width = option.text.chars().count() as u16;
            let rect = Rect::new(x, y, width, 1);
            let target = HitTarget::LaunchAgent(option.index);
            let hovered = app.hovered.as_ref() == Some(&target);
            let selected = option.index == form.agent;
            let style = if selected {
                Style::new()
                    .fg(palette.contrast_fg())
                    .bg(palette.accent)
                    .bold()
            } else if hovered {
                Style::new().fg(palette.text).bg(palette.surface1)
            } else {
                Style::new().fg(palette.subtext0)
            };
            f.render_widget(Paragraph::new(option.text).style(style), rect);
            hits.push(HitRegion::new(rect, target));
            x += width;
        }
        y += 1;
    }

    if worktree {
        f.render_widget(
            Paragraph::new(" worktree").style(Style::new().fg(if form.field == 2 {
                palette.accent
            } else {
                palette.overlay0
            })),
            Rect::new(inner.x, y, inner.width, 1),
        );
        y += 1;
        let checkout = Rect::new(inner.x, y, inner.width, 1);
        f.render_widget(Clear, checkout);
        let input = if form.branch.is_empty() {
            Line::from(vec![
                Span::styled(
                    " branch, shortcut, or existing worktree",
                    Style::new().fg(palette.overlay0),
                ),
                Span::styled(
                    if form.field == 2 { "█" } else { "" },
                    Style::new().fg(palette.accent),
                ),
            ])
        } else {
            Line::from(vec![
                Span::styled(format!(" {}", form.branch), Style::new().fg(palette.text)),
                Span::styled(
                    if form.field == 2 { "█" } else { "" },
                    Style::new().fg(palette.accent),
                ),
            ])
        };
        f.render_widget(
            Paragraph::new(input).style(Style::new().bg(palette.surface0)),
            checkout,
        );
        hits.push(HitRegion::new(checkout, HitTarget::LaunchCheckout));
        y += 1;

        let candidate_area = Rect::new(inner.x, y, inner.width, 5.min(inner.height));
        hits.push(HitRegion::new(candidate_area, HitTarget::LaunchCandidates));
        let matches = form.matching_candidates();
        if form.candidates_loading {
            f.render_widget(
                Paragraph::new(" loading worktrees…").style(Style::new().fg(palette.overlay0)),
                candidate_area,
            );
        } else if matches.is_empty() {
            f.render_widget(
                Paragraph::new(" no matching worktrees").style(Style::new().fg(palette.overlay0)),
                candidate_area,
            );
        } else {
            let first = form
                .candidate_selected
                .map(|selected| selected.saturating_sub(4))
                .unwrap_or(0);
            for (row, (filtered_index, candidate_index)) in
                matches.iter().enumerate().skip(first).take(5).enumerate()
            {
                let candidate = &form.candidates[*candidate_index];
                let selected = form.candidate_selected == Some(filtered_index);
                let rect = Rect::new(
                    candidate_area.x,
                    candidate_area.y + row as u16,
                    candidate_area.width,
                    1,
                );
                let target = HitTarget::LaunchCandidate(filtered_index);
                let hovered = app.hovered.as_ref() == Some(&target);
                let marker = if selected { "→" } else { " " };
                let current = if candidate.current { "  @ current" } else { "" };
                let style = if selected || hovered {
                    Style::new()
                        .fg(palette.contrast_fg())
                        .bg(palette.accent)
                        .bold()
                } else {
                    Style::new().fg(palette.subtext0)
                };
                f.render_widget(
                    Paragraph::new(format!(" {marker} {}{current}", candidate.branch)).style(style),
                    rect,
                );
                hits.push(HitRegion::new(rect, target));
            }
        }
        if let Some(name) = form.pending_create() {
            f.render_widget(
                Paragraph::new(format!(" ↵ create worktree '{name}'"))
                    .style(Style::new().fg(palette.yellow)),
                Rect::new(inner.x, y + 5, inner.width, 1),
            );
        }
        y += 6;
    }

    let toggleable = agents
        .get(form.agent)
        .is_some_and(|agent| ext::dangerous_toggleable(agent));
    let dangerous_rect = Rect::new(inner.x, y, inner.width, 1);
    let dangerous_target = HitTarget::ToggleDangerous;
    let dangerous_style = if !toggleable {
        Style::new()
            .fg(palette.overlay0)
            .add_modifier(Modifier::DIM)
    } else if app.hovered.as_ref() == Some(&dangerous_target) || form.field == 3 {
        Style::new().fg(palette.accent).bold()
    } else {
        Style::new().fg(palette.subtext0)
    };
    f.render_widget(
        Paragraph::new(format!(
            " [{}] dangerous mode",
            if form.dangerous { "x" } else { " " }
        ))
        .style(dangerous_style),
        dangerous_rect,
    );
    if toggleable {
        hits.push(HitRegion::new(dangerous_rect, dangerous_target));
    }
    y += 1;
    let help = if form.field == 4 {
        " enter to open · shift-tab back · esc cancel"
    } else if worktree && form.field == 2 {
        " type branch, ^, -, @, pr:N, or mr:N · ↑/↓ candidates · tab next"
    } else {
        " j/k fields · h/l change · space toggle · tab next"
    };
    f.render_widget(
        Paragraph::new(help).style(Style::new().fg(palette.overlay0)),
        Rect::new(inner.x, y, inner.width, 1),
    );

    let primary = if form.pending_create().is_some() {
        "↵ create + open"
    } else {
        "↵ open cockpit"
    };
    let primary_label = if form.field == 4 {
        format!("› {primary} ‹")
    } else {
        primary.into()
    };
    let buttons = button_row(inner, &[&primary_label, "esc cancel"]);
    draw_button(
        f,
        buttons[0],
        &primary_label,
        HitTarget::Submit,
        (form.field == 4).then_some(palette.accent),
        app,
        hits,
    );
    draw_button(
        f,
        buttons[1],
        "esc cancel",
        HitTarget::Cancel,
        None,
        app,
        hits,
    );
}

fn draw_new_path(f: &mut Frame, area: Rect, input: &str, app: &App, hits: &mut Vec<HitRegion>) {
    let palette = &app.palette;
    let Some((_, inner)) = modal_shell(f, area, 52, 8, palette.accent, palette, hits) else {
        return;
    };
    f.render_widget(
        Paragraph::new(" new directory").style(Style::new().fg(palette.text).bold()),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    f.render_widget(
        Paragraph::new(" path").style(Style::new().fg(palette.overlay0)),
        Rect::new(inner.x, inner.y + 2, inner.width, 1),
    );
    let input_rect = Rect::new(inner.x, inner.y + 3, inner.width, 1);
    f.render_widget(
        Paragraph::new(format!(" {input}█"))
            .style(Style::new().fg(palette.text).bg(palette.surface0)),
        input_rect,
    );
    hits.push(HitRegion::new(input_rect, HitTarget::Search));
    let buttons = button_row(inner, &["↵ create", "esc cancel"]);
    draw_button(
        f,
        buttons[0],
        "↵ create",
        HitTarget::Submit,
        Some(palette.accent),
        app,
        hits,
    );
    draw_button(
        f,
        buttons[1],
        "esc cancel",
        HitTarget::Cancel,
        None,
        app,
        hits,
    );
}

fn draw_confirm(
    f: &mut Frame,
    area: Rect,
    message: &str,
    action: &DelAction,
    app: &App,
    hits: &mut Vec<HitRegion>,
) {
    let palette = &app.palette;
    let width = (message.chars().count() as u16 + 4).clamp(40, 72);
    let Some((_, inner)) = modal_shell(f, area, width, 7, palette.red, palette, hits) else {
        return;
    };
    f.render_widget(
        Paragraph::new(" delete?").style(Style::new().fg(palette.red).bold()),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    f.render_widget(
        Paragraph::new(format!(" {message}"))
            .style(Style::new().fg(palette.text))
            .wrap(Wrap { trim: false }),
        Rect::new(inner.x, inner.y + 1, inner.width, 2),
    );
    let primary = match action {
        DelAction::OfferForce(..) => "f force remove",
        DelAction::ForceArmed(..) => "y confirm force",
        DelAction::RemoveAll(..) => "y remove all",
        _ => "y confirm",
    };
    let buttons = button_row(inner, &[primary, "esc cancel"]);
    draw_button(
        f,
        buttons[0],
        primary,
        HitTarget::Confirm,
        Some(palette.red),
        app,
        hits,
    );
    draw_button(
        f,
        buttons[1],
        "esc cancel",
        HitTarget::Cancel,
        None,
        app,
        hits,
    );
}

fn draw_help(f: &mut Frame, area: Rect, app: &App, hits: &mut Vec<HitRegion>) {
    let palette = &app.palette;
    let rows = [
        ("mouse", "hover to preview; click to open; wheel to move"),
        ("j / k", "move through results"),
        ("h / l", "previous / next source"),
        ("g / G", "first / last result"),
        ("1 / 2 / 3", "projects / sessions / cleanable"),
        ("/", "search; esc returns to navigation"),
        ("^j / ^k", "move while typing a search"),
        (
            "↵",
            "workspace: focus · remote: open · directory: new cockpit",
        ),
        ("tab", "sessions: cycle agent filter (shift-tab reverses)"),
        ("^o", "new cockpit for the selected local project"),
        ("^n", "new directory, then new-cockpit form"),
        ("^d", "workspace: close · worktree: merge-gated remove"),
        ("^x", "cleanable: remove all visible clean entries"),
        ("^r", "reload the list"),
        ("q", "close"),
        ("esc", "leave search / clear search / close"),
        ("", "launch form: j/k fields, h/l changes the value"),
        ("", "dangerous mode starts enabled; disable before launch"),
    ];
    let Some((_, inner)) = modal_shell(
        f,
        area,
        72,
        rows.len() as u16 + 5,
        palette.accent,
        palette,
        hits,
    ) else {
        return;
    };
    f.render_widget(
        Paragraph::new(" help").style(Style::new().fg(palette.text).bold()),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    for (index, (key, description)) in rows.iter().enumerate() {
        let y = inner.y + 2 + index as u16;
        if y >= inner.y + inner.height.saturating_sub(1) {
            break;
        }
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!(" {key:9}"), Style::new().fg(palette.accent).bold()),
                Span::styled((*description).to_string(), Style::new().fg(palette.text)),
            ])),
            Rect::new(inner.x, y, inner.width, 1),
        );
    }
    let buttons = button_row(inner, &["esc close"]);
    draw_button(
        f,
        buttons[0],
        "esc close",
        HitTarget::Cancel,
        None,
        app,
        hits,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_options_wrap_within_the_modal_and_keep_every_option() {
        let agents: Vec<String> = ["claude", "codex", "copilot", "cursor", "opencode", "droid"]
            .into_iter()
            .map(String::from)
            .collect();
        let rows = agent_option_rows(&agents, 0, LAUNCH_W as usize - 4);
        assert!(rows.len() > 1, "six agents should wrap");
        for row in &rows {
            let width: usize = row.iter().map(|option| option.text.chars().count()).sum();
            assert!(width <= LAUNCH_W as usize - 4, "{width}");
        }
        assert_eq!(rows.iter().map(Vec::len).sum::<usize>(), agents.len() + 1);
        assert!(rows[0][0].text.contains("(●) claude"));
    }

    #[test]
    fn navigator_stays_sidebar_sized_on_wide_screens() {
        assert_eq!(navigator_width(300), NAV_MAX);
        assert_eq!(navigator_width(90), 30);
        assert_eq!(navigator_width(40), 20);
    }

    #[test]
    fn centered_modal_keeps_a_margin_and_rejects_tiny_areas() {
        assert_eq!(
            centered(Rect::new(0, 0, 100, 30), 40, 10),
            Some(Rect::new(30, 10, 40, 10))
        );
        assert_eq!(centered(Rect::new(0, 0, 3, 3), 40, 10), None);
    }

    #[test]
    fn hit_regions_include_left_top_and_exclude_right_bottom_edges() {
        let region = HitRegion::new(Rect::new(2, 3, 4, 2), HitTarget::Help);
        assert!(region.contains(2, 3));
        assert!(region.contains(5, 4));
        assert!(!region.contains(6, 4));
        assert!(!region.contains(5, 5));
    }
}
