//! App state + key handling. One Mode enum drives which screen/modal is
//! active; all actions shell out through ext.rs.

use crate::ext;
use crate::sessions::{Agent as SessionAgent, Session};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::text::Text;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc;

#[derive(Clone)]
pub enum EntryKind {
    Workspace { id: String, status: String },
    Remote(String), // ssh alias from $HERDR_DECK_REMOTES; opens herdr --remote
    Worktree(PathBuf),
    Dir(PathBuf),
    Session(Session),
}

#[derive(Clone)]
pub struct Entry {
    pub label: String, // workspace label or ~-collapsed path
    pub kind: EntryKind,
}

impl Entry {
    pub fn cache_key(&self) -> String {
        match &self.kind {
            EntryKind::Workspace { id, .. } => format!("w:{id}"),
            EntryKind::Session(s) => format!("s:{}:{}", s.agent.id(), s.id),
            _ => format!("d:{}", self.label),
        }
    }

    fn matches(&self, filter: &str) -> bool {
        match_indices(&self.label, filter).is_some()
            || match &self.kind {
                EntryKind::Session(s) => {
                    match_indices(s.agent.id(), filter).is_some()
                        || match_indices(&s.cwd.to_string_lossy(), filter).is_some()
                }
                _ => false,
            }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum Source {
    Projects,
    Sessions,
}

pub struct LaunchForm {
    pub dir: PathBuf,
    pub agent: usize, // index into App.agents; == agents.len() means "none"
    pub branch: String,
    pub dangerous: bool,
    pub field: usize, // 0 agent, 1 branch, 2 dangerous
}

pub enum DelAction {
    CloseWs(String),
    RemoveMerged(PathBuf),
    OfferForce(PathBuf, String), // path, branch — 'f' arms the force stage
    ForceArmed(PathBuf, String),
}

pub enum Mode {
    List,
    Launch(LaunchForm),
    NewPath { input: String },
    ConfirmDelete { msg: String, action: DelAction },
    Help,
}

pub struct App {
    pub entries: Vec<Entry>,
    pub filter: String,
    pub filtered: Vec<usize>,
    pub selected: usize, // index into filtered
    pub mode: Mode,
    pub preview: HashMap<String, Text<'static>>,
    pub agents: Vec<String>, // detected agent binaries; launch form adds "none"
    pub source: Source,
    pub session_agent: Option<SessionAgent>,
    pub status: Option<String>,
    pub quit: bool,
    // Previews run subprocesses (herdr/wt/eza) on a worker thread so a slow
    // or hung command can never freeze the UI; results drain each tick.
    preview_tx: mpsc::Sender<(String, Entry, u16, u16)>,
    preview_rx: mpsc::Receiver<(String, Text<'static>)>,
    requested: HashSet<String>,
}

/// Case-insensitive subsequence match; tier order is preserved among matches
/// (mirrors the tv channel's no_sort), so no scoring needed. Returns the char
/// indices of the matched chars so the UI can highlight them (tv's match_fg).
pub fn match_indices(hay: &str, needle: &str) -> Option<Vec<usize>> {
    let mut idx = Vec::new();
    let mut ni = needle.chars().map(|c| c.to_ascii_lowercase());
    let mut want = ni.next();
    for (i, c) in hay.chars().enumerate() {
        match want {
            None => break,
            Some(w) if c.to_ascii_lowercase() == w => {
                idx.push(i);
                want = ni.next();
            }
            _ => {}
        }
    }
    want.is_none().then_some(idx)
}

impl App {
    pub fn new() -> Self {
        let own_pane = ext::json(&["herdr", "pane", "current"])
            .and_then(|v| v["result"]["pane"]["pane_id"].as_str().map(String::from))
            .unwrap_or_default();
        let (preview_tx, req_rx) = mpsc::channel::<(String, Entry, u16, u16)>();
        let (res_tx, preview_rx) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok((key, entry, w, h)) = req_rx.recv() {
                let text = crate::preview::compute(&entry, w, h, &own_pane);
                if res_tx.send((key, text)).is_err() {
                    break;
                }
            }
        });
        let mut app = App {
            entries: vec![],
            filter: String::new(),
            filtered: vec![],
            selected: 0,
            mode: Mode::List,
            preview: HashMap::new(),
            agents: ext::detected_agents(),
            source: Source::Projects,
            session_agent: None,
            status: None,
            quit: false,
            preview_tx,
            preview_rx,
            requested: HashSet::new(),
        };
        app.reload();
        app
    }

    pub fn reload(&mut self) {
        self.entries.clear();
        match self.source {
            Source::Projects => {
                for w in ext::workspaces() {
                    self.entries.push(Entry {
                        label: w.label,
                        kind: EntryKind::Workspace { id: w.id, status: w.status },
                    });
                }
                for host in ext::remotes() {
                    self.entries.push(Entry { label: host.clone(), kind: EntryKind::Remote(host) });
                }
                let (wt, other) = ext::dirs();
                for p in wt {
                    self.entries.push(Entry {
                        label: ext::collapse_tilde(&p.to_string_lossy()),
                        kind: EntryKind::Worktree(p),
                    });
                }
                for p in other {
                    self.entries.push(Entry {
                        label: ext::collapse_tilde(&p.to_string_lossy()),
                        kind: EntryKind::Dir(p),
                    });
                }
            }
            Source::Sessions => {
                self.entries.extend(crate::sessions::list().into_iter().map(|session| Entry {
                    label: session.title.clone(),
                    kind: EntryKind::Session(session),
                }));
            }
        }
        self.invalidate_previews();
        self.apply_filter();
    }

    pub fn invalidate_previews(&mut self) {
        self.preview.clear();
        self.requested.clear();
    }

    pub fn apply_filter(&mut self) {
        self.filtered = (0..self.entries.len())
            .filter(|&i| {
                let entry = &self.entries[i];
                let agent_matches = match (&entry.kind, self.session_agent) {
                    (EntryKind::Session(s), Some(agent)) => s.agent == agent,
                    _ => true,
                };
                agent_matches && entry.matches(&self.filter)
            })
            .collect();
        self.selected = self.selected.min(self.filtered.len().saturating_sub(1));
    }

    pub fn selected_entry(&self) -> Option<&Entry> {
        self.filtered.get(self.selected).map(|&i| &self.entries[i])
    }

    /// Default launch-form agent: claude when installed, else the first
    /// detected agent (or "none" when nothing is detected).
    fn default_agent(&self) -> usize {
        self.agents.iter().position(|a| a == "claude").unwrap_or(0)
    }

    /// Queue the selected entry's preview for the worker thread. Called from
    /// the event loop only when input is idle (debounce for free).
    pub fn request_preview(&mut self, w: u16, h: u16) {
        let Some(entry) = self.selected_entry() else { return };
        let key = entry.cache_key();
        if self.preview.contains_key(&key) || self.requested.contains(&key) {
            return;
        }
        let entry = entry.clone();
        self.requested.insert(key.clone());
        let _ = self.preview_tx.send((key, entry, w, h));
    }

    /// Pull any finished previews into the cache (non-blocking).
    pub fn drain_previews(&mut self) {
        while let Ok((key, text)) = self.preview_rx.try_recv() {
            self.preview.insert(key, text);
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        self.status = None;
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.quit = true;
            return;
        }
        match self.mode {
            Mode::List => self.key_list(key),
            Mode::Launch(_) => self.key_launch(key),
            Mode::NewPath { .. } => self.key_new_path(key),
            Mode::ConfirmDelete { .. } => self.key_confirm_delete(key),
            Mode::Help => self.mode = Mode::List,
        }
    }

    fn key_list(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => {
                if self.filter.is_empty() {
                    self.quit = true;
                } else {
                    self.filter.clear();
                    self.apply_filter();
                }
            }
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => {
                self.selected = (self.selected + 1).min(self.filtered.len().saturating_sub(1))
            }
            KeyCode::Char('k') if ctrl => self.selected = self.selected.saturating_sub(1),
            KeyCode::Char('j') if ctrl => {
                self.selected = (self.selected + 1).min(self.filtered.len().saturating_sub(1))
            }
            KeyCode::Enter => self.open_selected(),
            KeyCode::Char('s') if ctrl => {
                self.source = if self.source == Source::Projects {
                    Source::Sessions
                } else {
                    Source::Projects
                };
                self.session_agent = None;
                self.filter.clear();
                self.selected = 0;
                self.reload();
            }
            KeyCode::Tab if self.source == Source::Sessions => self.cycle_session_agent(1),
            KeyCode::BackTab if self.source == Source::Sessions => self.cycle_session_agent(-1),
            KeyCode::Char('n') if ctrl => self.mode = Mode::NewPath { input: "~/".into() },
            KeyCode::Char('d') if ctrl => self.delete_selected(),
            KeyCode::Char('r') if ctrl => self.reload(),
            KeyCode::Char('?') if self.filter.is_empty() => self.mode = Mode::Help,
            KeyCode::Backspace => {
                self.filter.pop();
                self.apply_filter();
            }
            KeyCode::Char(c) if !ctrl => {
                self.filter.push(c);
                self.apply_filter();
            }
            _ => {}
        }
    }

    fn open_selected(&mut self) {
        let Some(entry) = self.selected_entry() else { return };
        match &entry.kind {
            EntryKind::Workspace { id, .. } => {
                ext::focus_workspace(id);
                self.quit = true;
            }
            EntryKind::Remote(host) => {
                ext::open_remote(host);
                self.quit = true;
            }
            EntryKind::Worktree(p) | EntryKind::Dir(p) => {
                self.mode = Mode::Launch(LaunchForm {
                    dir: p.clone(),
                    agent: self.default_agent(),
                    branch: String::new(),
                    dangerous: true,
                    field: 0,
                });
            }
            EntryKind::Session(session) => {
                let session = session.clone();
                match ext::launch_session_deck(&session) {
                    Ok(()) => self.quit = true,
                    Err(e) => self.status = Some(e),
                }
            }
        }
    }

    fn cycle_session_agent(&mut self, delta: isize) {
        let order = [
            SessionAgent::Claude,
            SessionAgent::Codex,
            SessionAgent::Cursor,
            SessionAgent::Pi,
        ];
        let available: Vec<Option<SessionAgent>> = std::iter::once(None)
            .chain(order.into_iter().filter(|agent| {
                self.entries.iter().any(
                    |e| matches!(&e.kind, EntryKind::Session(s) if s.agent == *agent),
                )
            }).map(Some))
            .collect();
        let current = available
            .iter()
            .position(|agent| *agent == self.session_agent)
            .unwrap_or(0) as isize;
        let next = (current + delta).rem_euclid(available.len() as isize) as usize;
        self.session_agent = available[next];
        self.selected = 0;
        self.apply_filter();
    }

    fn delete_selected(&mut self) {
        let Some(entry) = self.selected_entry() else { return };
        match &entry.kind {
            EntryKind::Workspace { id, .. } => {
                self.mode = Mode::ConfirmDelete {
                    msg: format!("close workspace {}?", entry.label),
                    action: DelAction::CloseWs(id.clone()),
                };
            }
            EntryKind::Worktree(p) => {
                let Some(info) = ext::wt_info(p) else {
                    self.status = Some(format!("{} is not a worktrunk worktree", entry.label));
                    return;
                };
                let branch = if info.branch.is_empty() {
                    entry.label.clone()
                } else {
                    info.branch.clone()
                };
                if info.main_state == "integrated" || info.main_state == "empty" {
                    self.mode = Mode::ConfirmDelete {
                        msg: format!("remove merged worktree {branch}?"),
                        action: DelAction::RemoveMerged(p.clone()),
                    };
                } else {
                    self.mode = Mode::ConfirmDelete {
                        msg: format!(
                            "{branch} is not merged (state: {}).",
                            info.main_state
                        ),
                        action: DelAction::OfferForce(p.clone(), branch),
                    };
                }
            }
            EntryKind::Dir(_) | EntryKind::Remote(_) | EntryKind::Session(_) => {
                self.status = Some("not a workspace or worktree; nothing to delete".into());
            }
        }
    }

    fn key_confirm_delete(&mut self, key: KeyEvent) {
        // Take the mode by value: arms either consume the action or restore it.
        let Mode::ConfirmDelete { msg, action } = std::mem::replace(&mut self.mode, Mode::List)
        else {
            return;
        };
        match (key.code, action) {
            (KeyCode::Esc, _) => {}
            (KeyCode::Char('y'), DelAction::CloseWs(id)) => {
                ext::close_workspace(&id);
                self.status = Some("workspace closed".into());
                self.reload();
            }
            (KeyCode::Char('y'), DelAction::RemoveMerged(p)) => {
                self.status = Some(if ext::wt_remove(&p, false) {
                    "removed merged worktree".into()
                } else {
                    "wt remove failed (dirty tree?)".into()
                });
                self.reload();
            }
            (KeyCode::Char('f'), DelAction::OfferForce(p, branch)) => {
                self.mode = Mode::ConfirmDelete {
                    msg: format!("force-remove {branch}? deletes uncommitted work AND the branch"),
                    action: DelAction::ForceArmed(p, branch),
                };
            }
            (KeyCode::Char('y'), DelAction::ForceArmed(p, branch)) => {
                self.status = Some(if ext::wt_remove(&p, true) {
                    format!("force-removed worktree {branch}")
                } else {
                    "wt remove -f -D failed".into()
                });
                self.reload();
            }
            (_, action) => self.mode = Mode::ConfirmDelete { msg, action },
        }
    }

    fn key_launch(&mut self, key: KeyEvent) {
        let n_agents = self.agents.len() + 1; // + "none"
        let agents = &self.agents;
        let Mode::Launch(form) = &mut self.mode else { return };
        match key.code {
            KeyCode::Esc => self.mode = Mode::List,
            KeyCode::Tab | KeyCode::Down => form.field = (form.field + 1) % 3,
            KeyCode::BackTab | KeyCode::Up => form.field = (form.field + 2) % 3,
            KeyCode::Enter => {
                let (dir, idx, branch) = (form.dir.clone(), form.agent, form.branch.clone());
                let dangerous = form.dangerous;
                let agent = self.agents.get(idx).cloned();
                match ext::launch_deck(&dir, agent.as_deref(), branch.trim(), dangerous)
                {
                    Ok(()) => self.quit = true,
                    Err(e) => {
                        self.status = Some(e);
                        self.mode = Mode::List;
                    }
                }
            }
            KeyCode::Left if form.field == 0 => {
                form.agent = (form.agent + n_agents - 1) % n_agents
            }
            KeyCode::Right | KeyCode::Char(' ') if form.field == 0 => {
                form.agent = (form.agent + 1) % n_agents
            }
            KeyCode::Char(' ')
                if form.field == 2
                    && agents.get(form.agent).is_some_and(|a| ext::dangerous_toggleable(a)) =>
            {
                form.dangerous = !form.dangerous
            }
            KeyCode::Backspace if form.field == 1 => {
                form.branch.pop();
            }
            KeyCode::Char(c) if form.field == 1 => form.branch.push(c),
            _ => {}
        }
    }

    fn key_new_path(&mut self, key: KeyEvent) {
        let Mode::NewPath { input } = &mut self.mode else { return };
        match key.code {
            KeyCode::Esc => self.mode = Mode::List,
            KeyCode::Backspace => {
                input.pop();
            }
            KeyCode::Char(c) => input.push(c),
            KeyCode::Enter => {
                let raw = input.trim().to_string();
                if raw.is_empty() {
                    self.mode = Mode::List;
                    return;
                }
                let mut p = ext::expand_tilde(&raw);
                if !p.starts_with('/') {
                    p = format!("{}/{p}", ext::home());
                }
                match std::fs::create_dir_all(&p) {
                    Ok(()) => {
                        self.mode = Mode::Launch(LaunchForm {
                            dir: PathBuf::from(p),
                            agent: self.default_agent(),
                            branch: String::new(),
                            dangerous: true,
                            field: 0,
                        });
                    }
                    Err(e) => {
                        self.status = Some(format!("mkdir failed: {e}"));
                        self.mode = Mode::List;
                    }
                }
            }
            _ => {}
        }
    }
}
