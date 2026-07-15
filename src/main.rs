mod app;
mod ext;
mod preview;
mod sessions;
mod ui;

use ratatui::crossterm::event::{self, DisableFocusChange, EnableFocusChange, Event, KeyEventKind};
use ratatui::crossterm::execute;
use std::time::Duration;

fn main() -> std::io::Result<()> {
    if std::env::var("HERDR_ENV").as_deref() != Ok("1") {
        eprintln!("wt-cockpit: not inside a herdr session (HERDR_ENV != 1)");
        std::process::exit(1);
    }
    let mut app = app::App::new();
    let mut terminal = ratatui::init();
    // Picker semantics: navigating away (e.g. Ctrl+hjkl pane nav) closes the
    // picker instead of leaving a stray pane behind.
    let _ = execute!(std::io::stdout(), EnableFocusChange);
    loop {
        app.drain_previews();
        terminal.draw(|f| ui::draw(f, &mut app))?;
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => app.handle_key(k),
                Event::FocusLost => {
                    // herdr quirk: the temp picker pane runs zoomed, and
                    // focusing away can leave the zoom stuck on the newly
                    // focused pane (hiding its siblings). Clear it on the
                    // way out — a no-op when nothing is zoomed.
                    ext::out(&["herdr", "pane", "zoom", "--off"]);
                    app.quit = true;
                }
                Event::Resize(..) => app.invalidate_previews(),
                _ => {}
            }
        } else {
            // Idle: queue the selected entry's preview for the worker thread,
            // so subprocess cost never lands on a keystroke (free debounce)
            // and a hung command can't freeze input.
            let size = terminal.size()?;
            let (w, h) = ui::preview_dims(size.width, size.height);
            app.request_preview(w, h);
        }
        if app.quit {
            break;
        }
    }
    let _ = execute!(std::io::stdout(), DisableFocusChange);
    ratatui::restore();
    if let Some(session) = app.resume.take() {
        return Err(sessions::resume(session));
    }
    Ok(())
}
