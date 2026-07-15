//! Rendering: list + preview split, footer, and the modal overlays.
//!
//! Styling mirrors the tv `herdr` channel (television/config.toml
//! [ui.theme_overrides]): every color is a named ANSI slot, so herdr-deck tracks
//! the terminal palette exactly like tv does — bright-black borders, yellow
//! input/match text, bright-yellow-on-black selection, blue count/preview
//! title, rounded borders.

use crate::app::{self, App, DelAction, Entry, EntryKind, LaunchForm, Mode, Source};
use crate::ext;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, List, ListItem, ListState, Paragraph};

const LIST_PCT: u16 = 23;

/// Inner size of the preview panel for a given terminal size — kept next to
/// the layout so preview rendering and computation agree.
pub fn preview_dims(w: u16, h: u16) -> (u16, u16) {
    let right = w - w * LIST_PCT / 100;
    (right.saturating_sub(2), h.saturating_sub(3))
}

/// Rounded, dim-bordered block with a centered title — tv's panel look.
fn panel(title: Line<'static>) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().dark_gray())
        .title(title.centered())
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let [main, footer] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(LIST_PCT), Constraint::Min(1)]).areas(main);
    let [input_area, results_area] =
        Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).areas(left);

    // Input bar: "> query▌" in yellow, "sel / total" count in blue italic.
    let source = match app.source {
        Source::Projects => "projects",
        Source::Sessions => app.session_agent.map(|a| a.id()).unwrap_or("sessions"),
    };
    let input_block = panel(Line::from(vec![
        " herdr-deck ".green().bold(),
        format!("[{source}] ").cyan(),
    ]));
    let input_inner = input_block.inner(input_area);
    f.render_widget(input_block, input_area);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            "> ".yellow(),
            app.filter.clone().yellow(),
            "▌".yellow(),
        ])),
        input_inner,
    );
    let pos = if app.filtered.is_empty() {
        0
    } else {
        app.selected + 1
    };
    f.render_widget(
        Paragraph::new(Line::from(
            format!("{pos} / {} ", app.filtered.len()).blue().italic(),
        ))
        .right_aligned(),
        input_inner,
    );

    // Results list.
    let results_block = panel(Line::from(" Results ".dark_gray()));
    let list_area = results_block.inner(results_area);
    f.render_widget(results_block, results_area);
    let items: Vec<ListItem> = app
        .filtered
        .iter()
        .map(|&i| ListItem::new(entry_line(&app.entries[i], &app.filter)))
        .collect();
    let mut state = ListState::default().with_selected(Some(app.selected));
    f.render_stateful_widget(
        List::new(items)
            .highlight_symbol("> ")
            .highlight_style(Style::new().light_yellow().bold().bg(Color::Black)),
        list_area,
        &mut state,
    );

    // Preview panel, titled with the selected entry.
    let ptitle = match app.selected_entry() {
        Some(e) => format!(" {} ", e.label),
        None => " preview ".into(),
    };
    let pblock = panel(Line::from(ptitle.blue()));
    let pinner = pblock.inner(right);
    f.render_widget(pblock, right);
    if let Some(entry) = app.selected_entry() {
        match app.preview.get(&entry.cache_key()) {
            Some(text) => f.render_widget(Paragraph::new(text.clone()), pinner),
            None => f.render_widget(Paragraph::new("…".dim()), pinner),
        }
    }

    // Footer: status message wins over the key hints.
    let footer_line = match &app.status {
        Some(s) => Line::from(s.clone().yellow()),
        None if app.source == Source::Sessions => Line::from(
            " type to filter · ⇥ agent · ^s projects · ↵ resume · ^r reload · ? help · esc quit"
                .dim(),
        ),
        None => Line::from(
            " type to filter · ^s sessions · ↵ open · ^n new · ^d delete · ^r reload · ? help · esc quit"
                .dim(),
        ),
    };
    f.render_widget(Paragraph::new(footer_line), footer);

    match &app.mode {
        Mode::List => {}
        Mode::Launch(form) => draw_launch(f, area, form, &app.agents),
        Mode::NewPath { input } => draw_new_path(f, area, input),
        Mode::ConfirmDelete { msg, action } => draw_confirm(f, area, msg, action),
        Mode::Help => draw_help(f, area),
    }
}

/// Label spans with tv-style match highlighting (matched chars in yellow).
fn label_spans(label: &str, filter: &str) -> Vec<Span<'static>> {
    let indices = if filter.is_empty() {
        None
    } else {
        app::match_indices(label, filter)
    };
    let Some(indices) = indices else {
        return vec![Span::raw(label.to_string())];
    };
    let mut spans = Vec::new();
    let mut buf = String::new();
    let mut it = indices.iter().copied().peekable();
    let mut cur = false;
    for (i, c) in label.chars().enumerate() {
        let hit = it.peek() == Some(&i);
        if hit {
            it.next();
        }
        if hit != cur && !buf.is_empty() {
            spans.push(if cur {
                std::mem::take(&mut buf).yellow()
            } else {
                Span::raw(std::mem::take(&mut buf))
            });
        }
        cur = hit;
        buf.push(c);
    }
    if !buf.is_empty() {
        spans.push(if cur { buf.yellow() } else { Span::raw(buf) });
    }
    spans
}

fn entry_line(e: &Entry, filter: &str) -> Line<'static> {
    let mut spans = match &e.kind {
        EntryKind::Workspace { status, .. } => {
            let dot = match status.as_str() {
                "working" => "●".yellow(),
                "blocked" => "●".red(),
                "done" => "●".green(),
                _ => "●".cyan(),
            };
            vec![dot, Span::raw(" ")]
        }
        EntryKind::Remote(_) => vec!["⇄ ".cyan()],
        EntryKind::Session(s) => {
            let icon = match s.agent {
                crate::sessions::Agent::Claude => "C",
                crate::sessions::Agent::Codex => "X",
                crate::sessions::Agent::Cursor => "↗",
                crate::sessions::Agent::Pi => "π",
            };
            vec![format!("{icon} ").cyan()]
        }
        _ => vec!["▸ ".dim()],
    };
    spans.extend(label_spans(&e.label, filter));
    Line::from(spans)
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

fn modal(f: &mut Frame, area: Rect, title: Line<'static>, lines: Vec<Line<'static>>, w: u16) {
    let rect = centered(area, w, lines.len() as u16 + 2);
    f.render_widget(Clear, rect);
    let block = panel(title);
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    f.render_widget(Paragraph::new(lines), inner);
}

fn field_label(text: &str, active: bool) -> Span<'static> {
    if active {
        text.to_string().bold().yellow()
    } else {
        Span::raw(text.to_string())
    }
}

fn draw_launch(f: &mut Frame, area: Rect, form: &LaunchForm, agents: &[String]) {
    let check = |on: bool| if on { "[x]" } else { "[ ]" };
    // Detected agents + "none"; selected option rendered bold.
    let mut agent_spans = vec![field_label(" agent    ", form.field == 0)];
    for (i, name) in agents
        .iter()
        .map(String::as_str)
        .chain(["none"])
        .enumerate()
    {
        let radio = format!(
            "{} {}   ",
            if form.agent == i { "(•)" } else { "( )" },
            name
        );
        agent_spans.push(if form.agent == i {
            radio.bold()
        } else {
            Span::raw(radio)
        });
    }
    let agent_width = agent_spans
        .iter()
        .map(|s| s.content.chars().count())
        .sum::<usize>();
    let lines = vec![
        Line::from(agent_spans),
        Line::from(vec![
            field_label(" branch   ", form.field == 1),
            form.branch.clone().yellow(),
            if form.field == 1 {
                "▌".yellow()
            } else {
                Span::raw("")
            },
        ]),
        Line::from(vec![
            // Greyed out for agents without a known dangerous mode and
            // "none", where the toggle has no effect.
            if agents
                .get(form.agent)
                .is_some_and(|a| ext::dangerous_toggleable(a))
            {
                field_label(
                    &format!(" {} dangerous", check(form.dangerous)),
                    form.field == 2,
                )
            } else {
                format!(" {} dangerous", check(form.dangerous)).dim()
            },
        ]),
        Line::raw(""),
        Line::from(" ⇥ field · space/←→ toggle · ↵ launch · esc back".dim()),
    ];
    let title = format!(
        " launch: {} ",
        ext::collapse_tilde(&form.dir.to_string_lossy())
    );
    let w = (agent_width as u16 + 2).max(52);
    modal(f, area, Line::from(title.green().bold()), lines, w);
}

fn draw_new_path(f: &mut Frame, area: Rect, input: &str) {
    let lines = vec![
        Line::from(vec![
            " path ".dim(),
            input.to_string().yellow(),
            "▌".yellow(),
        ]),
        Line::raw(""),
        Line::from(" ↵ mkdir + launch · esc back".dim()),
    ];
    modal(
        f,
        area,
        Line::from(" new directory ".green().bold()),
        lines,
        52,
    );
}

fn draw_confirm(f: &mut Frame, area: Rect, msg: &str, action: &DelAction) {
    let hint = match action {
        DelAction::OfferForce(..) => Line::from(vec![
            " f".bold().red(),
            " force-remove · ".dim(),
            "esc".bold(),
            " cancel".dim(),
        ]),
        DelAction::ForceArmed(..) => Line::from(vec![
            " y".bold().red(),
            " confirm force-remove · ".dim(),
            "esc".bold(),
            " cancel".dim(),
        ]),
        _ => Line::from(vec![
            " y".bold(),
            " confirm · ".dim(),
            "esc".bold(),
            " cancel".dim(),
        ]),
    };
    let w = (msg.chars().count() as u16 + 4).clamp(36, area.width);
    let lines = vec![Line::raw(format!(" {msg}")), Line::raw(""), hint];
    modal(f, area, Line::from(" delete ".red().bold()), lines, w);
}

fn draw_help(f: &mut Frame, area: Rect) {
    let lines: Vec<Line> = [
        ("type", "filter the list (esc clears)"),
        ("^s", "switch projects / agent sessions source"),
        ("⇥", "sessions: filter by agent (shift-tab reverses)"),
        (
            "↵",
            "workspace: focus · remote ⇄: new window · dir: launch form",
        ),
        ("^n", "new directory (mkdir -p), then launch form"),
        ("^d", "workspace: close · worktree: merge-gated remove"),
        ("^r", "reload the list"),
        ("esc", "back / quit"),
        ("", ""),
        ("", "new worktree = ↵ on a repo + fill the branch field"),
        (
            "",
            "remotes (⇄) come from $HERDR_DECK_REMOTES, one window each",
        ),
        (
            "",
            "dangerous mode is enabled by default; disable it before launch",
        ),
    ]
    .iter()
    .map(|(k, v)| Line::from(vec![format!(" {k:5} ").bold(), Span::raw(v.to_string())]))
    .collect();
    modal(f, area, Line::from(" help ".green().bold()), lines, 62);
}
