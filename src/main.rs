mod app;
mod editor;
mod ext;
mod preview;
mod recent;
mod sessions;
mod theme;
mod ui;

use ratatui::crossterm::event::{
    self, DisableFocusChange, DisableMouseCapture, EnableFocusChange, EnableMouseCapture, Event,
    KeyEventKind,
};
use ratatui::crossterm::execute;
use std::time::Duration;

fn plugin_context_cwd(context: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(context).ok()?;
    value["workspace_cwd"]
        .as_str()
        .or_else(|| value["focused_pane_cwd"].as_str())
        .filter(|cwd| !cwd.is_empty())
        .map(String::from)
}

fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if matches!(args.first().map(String::as_str), Some("--help" | "-h")) {
        println!("herdr-deck\n\nUSAGE:\n  herdr-deck");
        return Ok(());
    }
    if args.first().map(String::as_str) == Some("--print-plugin-cwd") {
        if let Ok(context) = std::env::var("HERDR_PLUGIN_CONTEXT_JSON")
            && let Some(cwd) = plugin_context_cwd(&context)
        {
            println!("{cwd}");
        }
        return Ok(());
    }
    let plugin_command = match args.first().map(String::as_str) {
        Some("--open-link") => Some(editor::open_clicked_link()),
        Some("--restore-editors") => Some(editor::restore_editors()),
        Some("--cleanup-editor-event") => Some(editor::cleanup_event()),
        Some("--record-workspace-focus") => Some(recent::record_focus()),
        Some("--toggle-project") => Some(recent::toggle_project()),
        _ => None,
    };
    if let Some(result) = plugin_command {
        if let Err(error) = result {
            eprintln!("herdr-deck: {error}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if std::env::var("HERDR_ENV").as_deref() != Ok("1") {
        eprintln!("herdr-deck: not inside a herdr session (HERDR_ENV != 1)");
        std::process::exit(1);
    }
    let mut app = app::App::new();
    let mut terminal = ratatui::init();
    execute!(std::io::stdout(), EnableFocusChange, EnableMouseCapture)?;
    let result = (|| -> std::io::Result<()> {
        loop {
            app.drain_previews();
            app.drain_candidates();
            app.drain_cleanup();
            terminal.draw(|f| ui::draw(f, &mut app))?;
            // Blocking work runs immediately after a draw so its status is
            // visible while worktrunk or Herdr subprocesses are active.
            if app.pending.is_some() {
                app.run_pending();
                if app.quit {
                    break;
                }
                continue;
            }
            if event::poll(Duration::from_millis(100))? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => app.handle_key(key),
                    Event::Mouse(mouse) => app.handle_mouse(mouse),
                    Event::FocusLost => app.quit = true,
                    Event::Resize(..) => {
                        app.invalidate_previews();
                        app.clear_hit_regions();
                    }
                    _ => {}
                }
            } else {
                // Idle preview requests keep subprocess cost off input events.
                let size = terminal.size()?;
                let (width, height) = ui::preview_dims(size.width, size.height);
                app.request_preview(width, height);
            }
            if app.quit {
                break;
            }
        }
        Ok(())
    })();
    let _ = execute!(std::io::stdout(), DisableMouseCapture, DisableFocusChange);
    ratatui::restore();
    result
}

#[cfg(test)]
mod tests {
    use super::plugin_context_cwd;

    #[test]
    fn plugin_context_prefers_workspace_cwd() {
        let context = r#"{
            "workspace_cwd":"/repo",
            "focused_pane_cwd":"/repo/subdir"
        }"#;
        assert_eq!(plugin_context_cwd(context).as_deref(), Some("/repo"));
        assert_eq!(
            plugin_context_cwd(r#"{"focused_pane_cwd":"/fallback"}"#).as_deref(),
            Some("/fallback")
        );
    }
}
