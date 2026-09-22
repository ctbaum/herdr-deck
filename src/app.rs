//! App state + key handling. One Mode enum drives which screen/modal is
//! active; all actions shell out through ext.rs.

use crate::ext;
use crate::sessions::{Agent as SessionAgent, Session};
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use ratatui::text::Text;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc;

#[derive(Clone)]
pub enum EntryKind {
    Workspace {
        id: String,
        status: String,
        cwd: Option<PathBuf>,
    },
    Remote(String), // ssh alias from $HERDR_DECK_REMOTES; opens herdr --remote
    Worktree(PathBuf),
    Cleanable {
        path: PathBuf,
        clean: bool,
    },
    Dir(PathBuf),
    Session(Session),
}

#[derive(Clone)]
pub struct Entry {
    pub label: String, // workspace label or ~-collapsed path
    pub kind: EntryKind,
}

impl Entry {
    pub fn launch_dir(&self) -> Option<&PathBuf> {
        match &self.kind {
            EntryKind::Workspace { cwd, .. } => cwd.as_ref(),
            EntryKind::Worktree(path)
            | EntryKind::Cleanable { path, .. }
            | EntryKind::Dir(path) => Some(path),
            EntryKind::Remote(_) | EntryKind::Session(_) => None,
        }
    }

    pub fn cache_key(&self) -> String {
        match &self.kind {
            EntryKind::Workspace { id, .. } => format!("w:{id}"),
            EntryKind::Session(s) => format!("s:{}:{}", s.agent.id(), s.id),
            EntryKind::Cleanable { path, .. } => format!("c:{}", path.to_string_lossy()),
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
                EntryKind::Cleanable { path, .. } => {
                    match_indices(&path.to_string_lossy(), filter).is_some()
                }
                _ => false,
            }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Source {
    Projects,
    Sessions,
    Cleanup,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LaunchTarget {
    SameCheckout,
    Worktree,
}

/// A rendered control that can receive mouse input. Rendering owns the exact
/// rectangles so hit testing can never drift from the visible UI.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HitTarget {
    Search,
    Results,
    Result(usize),
    Source(Source),
    CycleAgent,
    NewCockpit,
    NewPath,
    Delete,
    RemoveAll,
    Reload,
    Help,
    Quit,
    ModalSurface,
    LaunchSameCheckout,
    LaunchWorktree,
    LaunchAgent(usize),
    LaunchCheckout,
    LaunchCandidates,
    LaunchCandidate(usize),
    ToggleDangerous,
    Submit,
    Confirm,
    Cancel,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HitRegion {
    pub rect: Rect,
    pub target: HitTarget,
}

impl HitRegion {
    pub fn new(rect: Rect, target: HitTarget) -> Self {
        Self { rect, target }
    }

    pub(crate) fn contains(&self, column: u16, row: u16) -> bool {
        column >= self.rect.x
            && column < self.rect.x.saturating_add(self.rect.width)
            && row >= self.rect.y
            && row < self.rect.y.saturating_add(self.rect.height)
    }
}

pub struct LaunchForm {
    pub dir: PathBuf,
    pub workspace: Option<String>,
    pub target: LaunchTarget,
    pub agent: usize, // index into App.agents; == agents.len() means "none"
    pub branch: String,
    pub candidates: Vec<ext::WorktreeCandidate>,
    /// Candidates load on a worker thread (`wt list` can take minutes on a
    /// pathological repo); the form opens immediately and fills in later.
    pub candidates_loading: bool,
    /// Index into the filtered candidate list, not `candidates` itself.
    pub candidate_selected: Option<usize>,
    pub dangerous: bool,
    pub field: usize, // 0 location, 1 agent, 2 worktree, 3 dangerous
}

impl LaunchForm {
    fn new(dir: PathBuf, workspace: Option<String>, agent: usize) -> Self {
        Self {
            dir,
            workspace,
            target: LaunchTarget::SameCheckout,
            agent,
            branch: String::new(),
            candidates: vec![],
            candidates_loading: true,
            candidate_selected: None,
            dangerous: true,
            field: 0,
        }
    }

    fn set_target(&mut self, target: LaunchTarget) {
        self.target = target;
        self.field = 0;
        self.candidate_selected = None;
    }

    fn move_field(&mut self, delta: isize) {
        let fields: &[usize] = if self.target == LaunchTarget::Worktree {
            &[0, 1, 2, 3]
        } else {
            &[0, 1, 3]
        };
        let current = fields
            .iter()
            .position(|field| *field == self.field)
            .unwrap_or(0);
        self.field = fields[(current as isize + delta).rem_euclid(fields.len() as isize) as usize];
        self.candidate_selected = None;
    }

    pub fn matching_candidates(&self) -> Vec<usize> {
        self.candidates
            .iter()
            .enumerate()
            .filter(|(_, candidate)| match_indices(&candidate.branch, self.branch.trim()).is_some())
            .map(|(index, _)| index)
            .collect()
    }

    fn move_candidate(&mut self, delta: isize) {
        let len = self.matching_candidates().len();
        if len == 0 {
            self.candidate_selected = None;
            return;
        }
        self.candidate_selected = Some(match (self.candidate_selected, delta.is_negative()) {
            (Some(0), true) | (None, true) => len - 1,
            (Some(index), true) => index - 1,
            (Some(index), false) => (index + 1) % len,
            (None, false) => 0,
        });
    }

    /// The checkout name Enter would hand to worktrunk as a new worktree:
    /// non-empty free-form input with no exact worktree match, no highlighted
    /// candidate, and not a worktrunk shortcut. Shown in the form so a typo
    /// is visible before it silently becomes a branch.
    pub fn pending_create(&self) -> Option<&str> {
        let typed = self.branch.trim();
        if self.target != LaunchTarget::Worktree
            || typed.is_empty()
            || self.candidates_loading // an exact match may still be on its way
            || self.candidate_selected.is_some()
            || ext::worktrunk_is_shortcut(typed)
            || self.candidates.iter().any(|c| c.branch == typed)
        {
            return None;
        }
        Some(typed)
    }

    fn accept_candidate(&mut self) -> bool {
        let Some(selected) = self.candidate_selected else {
            return false;
        };
        let Some(candidate) = self
            .matching_candidates()
            .get(selected)
            .and_then(|index| self.candidates.get(*index))
        else {
            self.candidate_selected = None;
            return false;
        };
        self.branch.clone_from(&candidate.branch);
        self.candidate_selected = None;
        true
    }
}

pub enum DelAction {
    CloseWs(String),
    RemoveMerged(PathBuf),
    OfferForce(PathBuf, String), // path, branch — 'f' arms the force stage
    ForceArmed(PathBuf, String),
    RemoveAll(Vec<PathBuf>),
}

pub enum Mode {
    List,
    Launch(LaunchForm),
    NewPath { input: String },
    ConfirmDelete { msg: String, action: DelAction },
    Help,
}

/// Footer status line. Errors render red, everything else yellow.
pub struct Status {
    pub msg: String,
    pub error: bool,
}

impl Status {
    pub fn info(msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            error: false,
        }
    }

    pub fn err(msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            error: true,
        }
    }
}

/// A blocking operation queued by a key handler and executed by the event
/// loop right after the next draw, so the status message set alongside it is
/// on screen while subprocesses (worktrunk hooks, session scans) run.
pub enum Pending {
    Reload,
    Launch {
        dir: PathBuf,
        workspace: Option<String>,
        agent: Option<String>,
        branch: String,
        dangerous: bool,
    },
    Resume(Session),
    /// ^d on a worktree: `wt list` decides merged vs force-confirm, and can
    /// be slow, so it runs as a pending op instead of in the key handler.
    DeleteWorktree {
        path: PathBuf,
        label: String,
    },
    Delete(DelAction),
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
    pub status: Option<Status>,
    pub pending: Option<Pending>,
    pub quit: bool,
    pub palette: crate::theme::Palette,
    pub hit_regions: Vec<HitRegion>,
    pub hovered: Option<HitTarget>,
    // Previews run subprocesses (herdr/wt/eza) on a worker thread so a slow
    // or hung command can never freeze the UI; results drain each tick.
    preview_tx: mpsc::Sender<(String, Entry, u16, u16)>,
    preview_rx: mpsc::Receiver<(String, Text<'static>)>,
    requested: HashSet<String>,
    // Launch-form checkout candidates load the same way (wt list can be slow).
    candidates_rx: Option<mpsc::Receiver<(PathBuf, Vec<ext::WorktreeCandidate>)>>,
    // Cleanable worktrees stream in repository by repository from a bounded
    // worker scan, keeping navigation responsive while the list fills.
    cleanup_rx: Option<mpsc::Receiver<CleanupUpdate>>,
    pub cleanup_loading: bool,
}

enum CleanupUpdate {
    Batch(Vec<ext::IntegratedWt>),
    Done,
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
        // Native popups intentionally have no HERDR_PANE_ID. Using `pane
        // current` there would identify the tiled pane underneath the popup
        // and incorrectly remove it from workspace previews.
        let own_pane = std::env::var("HERDR_PANE_ID")
            .ok()
            .filter(|pane_id| !pane_id.is_empty())
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
            pending: None,
            quit: false,
            palette: crate::theme::Palette::load(),
            hit_regions: vec![],
            hovered: None,
            preview_tx,
            preview_rx,
            requested: HashSet::new(),
            candidates_rx: None,
            cleanup_rx: None,
            cleanup_loading: false,
        };
        app.reload();
        // Every ext call swallows subprocess errors, so a missing tool would
        // otherwise just look like an inexplicably empty list.
        let missing: Vec<&str> = ["wt", "zoxide", "fd"]
            .into_iter()
            .filter(|bin| !ext::command_exists(bin))
            .collect();
        if !missing.is_empty() {
            app.status = Some(Status::err(format!(
                "not found: {} · related features disabled",
                missing.join(", ")
            )));
        }
        app
    }

    pub fn reload(&mut self) {
        self.entries.clear();
        self.cleanup_rx = None;
        self.cleanup_loading = false;
        match self.source {
            Source::Projects => {
                for w in ext::workspaces() {
                    self.entries.push(Entry {
                        label: w.label,
                        kind: EntryKind::Workspace {
                            id: w.id,
                            status: w.status,
                            cwd: w.cwd,
                        },
                    });
                }
                for host in ext::remotes() {
                    self.entries.push(Entry {
                        label: host.clone(),
                        kind: EntryKind::Remote(host),
                    });
                }
                for d in ext::dirs() {
                    let label = ext::collapse_tilde(&d.path.to_string_lossy());
                    let kind = if d.kind == ext::DirKind::Worktree {
                        EntryKind::Worktree(d.path)
                    } else {
                        EntryKind::Dir(d.path)
                    };
                    self.entries.push(Entry { label, kind });
                }
            }
            Source::Sessions => {
                self.entries
                    .extend(crate::sessions::list().into_iter().map(|session| Entry {
                        label: session.title.clone(),
                        kind: EntryKind::Session(session),
                    }));
            }
            Source::Cleanup => {
                let (tx, rx) = mpsc::channel();
                self.cleanup_rx = Some(rx);
                self.cleanup_loading = true;
                std::thread::spawn(move || {
                    ext::scan_integrated_worktrees(|batch| {
                        tx.send(CleanupUpdate::Batch(batch)).is_ok()
                    });
                    let _ = tx.send(CleanupUpdate::Done);
                });
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
        let Some(entry) = self.selected_entry() else {
            return;
        };
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

    fn open_new_cockpit(&mut self) {
        let Some(entry) = self.selected_entry() else {
            self.status = Some(Status::info("no local project selected"));
            return;
        };
        let workspace = match &entry.kind {
            EntryKind::Workspace { id, .. } => Some(id.clone()),
            _ => None,
        };
        let Some(dir) = entry.launch_dir().cloned() else {
            self.status = Some(Status::info("no local project selected"));
            return;
        };
        self.open_launch_form(dir, workspace);
    }

    /// Open the launch form immediately and fetch its checkout candidates on
    /// a one-off worker thread; `drain_candidates` fills them in.
    fn open_launch_form(&mut self, dir: PathBuf, workspace: Option<String>) {
        let (tx, rx) = mpsc::channel();
        self.candidates_rx = Some(rx);
        let d = dir.clone();
        std::thread::spawn(move || {
            let _ = tx.send((d.clone(), ext::worktree_candidates(&d)));
        });
        self.mode = Mode::Launch(LaunchForm::new(dir, workspace, self.default_agent()));
    }

    /// Deliver finished candidate loads to the launch form (non-blocking).
    /// Results for a form that was closed or reopened on another dir are
    /// dropped.
    pub fn drain_candidates(&mut self) {
        let Some(rx) = &self.candidates_rx else {
            return;
        };
        while let Ok((dir, candidates)) = rx.try_recv() {
            if let Mode::Launch(form) = &mut self.mode
                && form.dir == dir
                && form.candidates_loading
            {
                form.candidates = candidates;
                form.candidates_loading = false;
            }
        }
    }

    /// Add finished repository scans to the cleanable list without blocking
    /// input. Sorting each batch keeps the visible order deterministic even
    /// though repositories finish out of order.
    pub fn drain_cleanup(&mut self) {
        let Some(rx) = &self.cleanup_rx else {
            return;
        };
        let updates: Vec<_> = rx.try_iter().collect();
        for update in updates {
            match update {
                CleanupUpdate::Batch(worktrees) => {
                    if worktrees.is_empty() {
                        continue;
                    }
                    let selected = self.selected_entry().map(Entry::cache_key);
                    self.entries
                        .extend(worktrees.into_iter().map(|worktree| Entry {
                            label: format!("{}/{}", worktree.project, worktree.branch),
                            kind: EntryKind::Cleanable {
                                path: worktree.path,
                                clean: worktree.clean,
                            },
                        }));
                    self.entries.sort_by(|a, b| a.label.cmp(&b.label));
                    self.apply_filter();
                    if let Some(selected) = selected
                        && let Some(position) = self
                            .filtered
                            .iter()
                            .position(|index| self.entries[*index].cache_key() == selected)
                    {
                        self.selected = position;
                    }
                }
                CleanupUpdate::Done => self.cleanup_loading = false,
            }
        }
    }

    /// Execute the queued blocking operation (see [`Pending`]).
    pub fn run_pending(&mut self) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        match pending {
            Pending::Reload => {
                self.reload();
                self.status = None;
            }
            Pending::Launch {
                dir,
                workspace,
                agent,
                branch,
                dangerous,
            } => match ext::launch_deck(
                &dir,
                workspace.as_deref(),
                agent.as_deref(),
                branch.trim(),
                dangerous,
            ) {
                Ok(()) => self.quit = true,
                Err(e) => {
                    self.status = Some(Status::err(e));
                    self.mode = Mode::List;
                }
            },
            Pending::Resume(session) => match ext::launch_session_deck(&session) {
                Ok(()) => self.quit = true,
                Err(e) => self.status = Some(Status::err(e)),
            },
            Pending::DeleteWorktree { path, label } => {
                let Some(info) = ext::wt_info(&path) else {
                    self.status =
                        Some(Status::info(format!("{label} is not a worktrunk worktree")));
                    return;
                };
                self.status = None;
                let branch = if info.branch.is_empty() {
                    label
                } else {
                    info.branch.clone()
                };
                self.mode = if info.integrated() {
                    Mode::ConfirmDelete {
                        msg: format!("remove merged worktree {branch}?"),
                        action: DelAction::RemoveMerged(path),
                    }
                } else {
                    Mode::ConfirmDelete {
                        msg: format!("{branch} is not merged (state: {}).", info.main_state),
                        action: DelAction::OfferForce(path, branch),
                    }
                };
            }
            Pending::Delete(action) => {
                let status = match action {
                    DelAction::CloseWs(id) => {
                        ext::close_workspace(&id);
                        Status::info("workspace closed")
                    }
                    DelAction::RemoveMerged(p) => {
                        if ext::wt_remove(&p, false) {
                            Status::info("removed merged worktree")
                        } else {
                            Status::err("wt remove failed (dirty tree?)")
                        }
                    }
                    DelAction::ForceArmed(p, branch) => {
                        if ext::wt_remove(&p, true) {
                            Status::info(format!("force-removed worktree {branch}"))
                        } else {
                            Status::err("wt remove -f -D failed")
                        }
                    }
                    DelAction::RemoveAll(paths) => {
                        let r = ext::wt_remove_clean(&paths);
                        let msg = format!(
                            "removed {} · skipped {} · failed {}",
                            r.removed, r.skipped, r.failed
                        );
                        if r.failed > 0 {
                            Status::err(msg)
                        } else {
                            Status::info(msg)
                        }
                    }
                    // Never queued: 'f' re-arms the modal instead.
                    DelAction::OfferForce(..) => return,
                };
                self.reload();
                self.status = Some(status);
            }
        }
    }

    fn hit_target_at(&self, column: u16, row: u16) -> Option<HitTarget> {
        self.hit_regions
            .iter()
            .rev()
            .find(|region| region.contains(column, row))
            .map(|region| region.target.clone())
    }

    pub fn clear_hit_regions(&mut self) {
        self.hit_regions.clear();
        self.hovered = None;
    }

    /// Mouse behavior follows Herdr's navigator: pointer movement highlights
    /// rows, a single left-button press activates, and wheel events move by
    /// three rows.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        let target = self.hit_target_at(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Moved => {
                self.hovered.clone_from(&target);
                match target {
                    Some(HitTarget::Result(index)) if index < self.filtered.len() => {
                        self.selected = index;
                    }
                    Some(HitTarget::LaunchCandidate(index)) => {
                        if let Mode::Launch(form) = &mut self.mode {
                            form.field = 2;
                            form.candidate_selected = Some(index);
                        }
                    }
                    _ => {}
                }
            }
            MouseEventKind::ScrollUp => self.mouse_scroll(target.as_ref(), -3),
            MouseEventKind::ScrollDown => self.mouse_scroll(target.as_ref(), 3),
            MouseEventKind::Down(MouseButton::Left) => {
                self.status = None;
                self.activate_hit_target(target);
            }
            _ => {}
        }
    }

    fn mouse_scroll(&mut self, target: Option<&HitTarget>, delta: isize) {
        match target {
            Some(HitTarget::Results | HitTarget::Result(_)) if matches!(self.mode, Mode::List) => {
                let last = self.filtered.len().saturating_sub(1);
                self.selected = self.selected.saturating_add_signed(delta).min(last);
            }
            Some(HitTarget::LaunchCandidates | HitTarget::LaunchCandidate(_)) => {
                if let Mode::Launch(form) = &mut self.mode {
                    for _ in 0..delta.unsigned_abs() {
                        form.move_candidate(delta.signum());
                    }
                }
            }
            _ => {}
        }
    }

    fn activate_hit_target(&mut self, target: Option<HitTarget>) {
        match target {
            Some(HitTarget::Search | HitTarget::Results | HitTarget::LaunchCandidates) => {}
            Some(HitTarget::Result(index)) if matches!(self.mode, Mode::List) => {
                if index < self.filtered.len() {
                    self.selected = index;
                    self.open_selected();
                }
            }
            Some(HitTarget::Source(source)) => self.switch_source(source),
            Some(HitTarget::CycleAgent) if self.source == Source::Sessions => {
                self.cycle_session_agent(1)
            }
            Some(HitTarget::NewCockpit) => self.open_new_cockpit(),
            Some(HitTarget::NewPath) => self.mode = Mode::NewPath { input: "~/".into() },
            Some(HitTarget::Delete) => self.delete_selected(),
            Some(HitTarget::RemoveAll) => self.confirm_remove_all(),
            Some(HitTarget::Reload) => self.queue_reload(),
            Some(HitTarget::Help) => self.mode = Mode::Help,
            Some(HitTarget::Quit) => self.quit = true,
            Some(HitTarget::LaunchSameCheckout) => {
                if let Mode::Launch(form) = &mut self.mode {
                    form.set_target(LaunchTarget::SameCheckout);
                }
            }
            Some(HitTarget::LaunchWorktree) => {
                if let Mode::Launch(form) = &mut self.mode {
                    form.set_target(LaunchTarget::Worktree);
                }
            }
            Some(HitTarget::LaunchAgent(index)) => {
                if let Mode::Launch(form) = &mut self.mode
                    && index <= self.agents.len()
                {
                    form.agent = index;
                    form.field = 1;
                }
            }
            Some(HitTarget::LaunchCheckout) => {
                if let Mode::Launch(form) = &mut self.mode {
                    form.field = 2;
                    form.candidate_selected = None;
                }
            }
            Some(HitTarget::LaunchCandidate(index)) => {
                if let Mode::Launch(form) = &mut self.mode {
                    form.field = 2;
                    form.candidate_selected = Some(index);
                    form.accept_candidate();
                }
            }
            Some(HitTarget::ToggleDangerous) => {
                if let Mode::Launch(form) = &mut self.mode
                    && self
                        .agents
                        .get(form.agent)
                        .is_some_and(|agent| ext::dangerous_toggleable(agent))
                {
                    form.field = 3;
                    form.dangerous = !form.dangerous;
                }
            }
            Some(HitTarget::Submit) => match self.mode {
                Mode::Launch(_) => self.submit_launch(),
                Mode::NewPath { .. } => self.submit_new_path(),
                _ => {}
            },
            Some(HitTarget::Confirm) => self.activate_confirmation(),
            Some(HitTarget::Cancel) => self.mode = Mode::List,
            Some(HitTarget::ModalSurface) => {}
            None if !matches!(self.mode, Mode::List) => self.mode = Mode::List,
            _ => {}
        }
    }

    fn switch_source(&mut self, source: Source) {
        if self.source == source {
            return;
        }
        self.source = source;
        self.session_agent = None;
        self.filter.clear();
        self.selected = 0;
        self.status = Some(Status::info(match source {
            Source::Projects => "loading projects…",
            Source::Sessions => "loading sessions…",
            Source::Cleanup => "scanning repositories for cleanable worktrees…",
        }));
        self.pending = Some(Pending::Reload);
    }

    fn queue_reload(&mut self) {
        self.status = Some(Status::info("reloading…"));
        self.pending = Some(Pending::Reload);
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
                let source = match self.source {
                    Source::Sessions => Source::Projects,
                    Source::Projects | Source::Cleanup => Source::Sessions,
                };
                self.switch_source(source);
            }
            KeyCode::Char('g') if ctrl => {
                let source = if self.source == Source::Cleanup {
                    Source::Projects
                } else {
                    Source::Cleanup
                };
                self.switch_source(source);
            }
            KeyCode::Tab if self.source == Source::Sessions => self.cycle_session_agent(1),
            KeyCode::BackTab if self.source == Source::Sessions => self.cycle_session_agent(-1),
            KeyCode::Char('o') if ctrl => self.open_new_cockpit(),
            KeyCode::Char('n') if ctrl => self.mode = Mode::NewPath { input: "~/".into() },
            KeyCode::Char('d') if ctrl => self.delete_selected(),
            KeyCode::Char('x') if ctrl && self.source == Source::Cleanup => {
                self.confirm_remove_all()
            }
            KeyCode::Char('r') if ctrl => self.queue_reload(),
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
        let Some(entry) = self.selected_entry() else {
            return;
        };
        match &entry.kind {
            EntryKind::Workspace { id, .. } => {
                ext::focus_workspace(id);
                self.quit = true;
            }
            EntryKind::Remote(host) => {
                ext::open_remote(host);
                self.quit = true;
            }
            EntryKind::Worktree(p) | EntryKind::Cleanable { path: p, .. } | EntryKind::Dir(p) => {
                self.open_launch_form(p.clone(), None);
            }
            EntryKind::Session(session) => {
                let session = session.clone();
                self.pending = Some(Pending::Resume(session));
                self.status = Some(Status::info("resuming session…"));
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
            .chain(
                order
                    .into_iter()
                    .filter(|agent| {
                        self.entries
                            .iter()
                            .any(|e| matches!(&e.kind, EntryKind::Session(s) if s.agent == *agent))
                    })
                    .map(Some),
            )
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
        let Some(entry) = self.selected_entry() else {
            return;
        };
        match &entry.kind {
            EntryKind::Workspace { id, .. } => {
                self.mode = Mode::ConfirmDelete {
                    msg: format!("close workspace {}?", entry.label),
                    action: DelAction::CloseWs(id.clone()),
                };
            }
            EntryKind::Worktree(p) => {
                let (path, label) = (p.clone(), entry.label.clone());
                self.status = Some(Status::info("checking worktree state…"));
                self.pending = Some(Pending::DeleteWorktree { path, label });
            }
            EntryKind::Cleanable { path, clean } => {
                if *clean {
                    self.mode = Mode::ConfirmDelete {
                        msg: format!("remove clean integrated worktree {}?", entry.label),
                        action: DelAction::RemoveMerged(path.clone()),
                    };
                } else {
                    self.status = Some(Status::info(
                        "integrated, but dirty; clean the worktree before removal",
                    ));
                }
            }
            EntryKind::Dir(_) | EntryKind::Remote(_) | EntryKind::Session(_) => {
                self.status = Some(Status::info(
                    "not a workspace or worktree; nothing to delete",
                ));
            }
        }
    }

    fn confirm_remove_all(&mut self) {
        if self.cleanup_loading {
            self.status = Some(Status::info(
                "wait for the repository scan to finish before removing all",
            ));
            return;
        }
        let mut paths = Vec::new();
        let mut projects = HashSet::new();
        let mut dirty = 0;
        for &index in &self.filtered {
            let entry = &self.entries[index];
            if let EntryKind::Cleanable { path, clean } = &entry.kind {
                if *clean {
                    paths.push(path.clone());
                    projects.insert(entry.label.split('/').next().unwrap_or(""));
                } else {
                    dirty += 1;
                }
            }
        }
        if paths.is_empty() {
            self.status = Some(Status::info(if dirty == 0 {
                "no clean integrated worktrees in this view".into()
            } else {
                format!("no removable worktrees; {dirty} integrated worktrees are dirty")
            }));
            return;
        }
        let skipped = if dirty == 0 {
            String::new()
        } else {
            format!("; {dirty} dirty will be skipped")
        };
        self.mode = Mode::ConfirmDelete {
            msg: format!(
                "remove {} clean integrated worktrees across {} projects{skipped}?",
                paths.len(),
                projects.len()
            ),
            action: DelAction::RemoveAll(paths),
        };
    }

    fn key_confirm_delete(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.mode = Mode::List,
            KeyCode::Char('f')
                if matches!(
                    self.mode,
                    Mode::ConfirmDelete {
                        action: DelAction::OfferForce(..),
                        ..
                    }
                ) =>
            {
                self.activate_confirmation()
            }
            KeyCode::Char('y')
                if !matches!(
                    self.mode,
                    Mode::ConfirmDelete {
                        action: DelAction::OfferForce(..),
                        ..
                    }
                ) =>
            {
                self.activate_confirmation()
            }
            _ => {}
        }
    }

    fn activate_confirmation(&mut self) {
        // Take the mode by value: arms either consume the action or restore it.
        let Mode::ConfirmDelete { action, .. } = std::mem::replace(&mut self.mode, Mode::List)
        else {
            return;
        };
        match action {
            DelAction::OfferForce(p, branch) => {
                self.mode = Mode::ConfirmDelete {
                    msg: format!("force-remove {branch}? deletes uncommitted work AND the branch"),
                    action: DelAction::ForceArmed(p, branch),
                };
            }
            // Confirmed removals run as pending ops so the status below is
            // visible while wt/herdr subprocesses do the work.
            action @ (DelAction::CloseWs(_)
            | DelAction::RemoveMerged(_)
            | DelAction::RemoveAll(_)
            | DelAction::ForceArmed(..)) => {
                self.status = Some(Status::info(match &action {
                    DelAction::CloseWs(_) => "closing workspace…",
                    DelAction::RemoveAll(_) => "removing clean worktrees…",
                    _ => "removing worktree…",
                }));
                self.pending = Some(Pending::Delete(action));
            }
        }
    }

    fn key_launch(&mut self, key: KeyEvent) {
        let n_agents = self.agents.len() + 1; // + "none"
        let agents = &self.agents;
        let Mode::Launch(form) = &mut self.mode else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.mode = Mode::List,
            KeyCode::Tab => form.move_field(1),
            KeyCode::BackTab => form.move_field(-1),
            KeyCode::Down if form.field == 2 => form.move_candidate(1),
            KeyCode::Up if form.field == 2 => form.move_candidate(-1),
            KeyCode::Down => form.move_field(1),
            KeyCode::Up => form.move_field(-1),
            KeyCode::Enter => {
                if form.field == 2 && form.accept_candidate() {
                    return;
                }
                self.submit_launch();
            }
            KeyCode::Left if form.field == 0 => form.set_target(LaunchTarget::SameCheckout),
            KeyCode::Right if form.field == 0 => form.set_target(LaunchTarget::Worktree),
            KeyCode::Char(' ') if form.field == 0 => form.set_target(match form.target {
                LaunchTarget::SameCheckout => LaunchTarget::Worktree,
                LaunchTarget::Worktree => LaunchTarget::SameCheckout,
            }),
            KeyCode::Left if form.field == 1 => form.agent = (form.agent + n_agents - 1) % n_agents,
            KeyCode::Right | KeyCode::Char(' ') if form.field == 1 => {
                form.agent = (form.agent + 1) % n_agents
            }
            KeyCode::Char(' ')
                if form.field == 3
                    && agents
                        .get(form.agent)
                        .is_some_and(|a| ext::dangerous_toggleable(a)) =>
            {
                form.dangerous = !form.dangerous
            }
            KeyCode::Backspace if form.field == 2 => {
                form.branch.pop();
                form.candidate_selected = None;
            }
            KeyCode::Char(c) if form.field == 2 => {
                form.branch.push(c);
                form.candidate_selected = None;
            }
            _ => {}
        }
    }

    fn submit_launch(&mut self) {
        let Mode::Launch(form) = &self.mode else {
            return;
        };
        let (branch, workspace) = match form.target {
            LaunchTarget::SameCheckout => (String::new(), form.workspace.clone()),
            LaunchTarget::Worktree if form.branch.trim().is_empty() => {
                self.status = Some(Status::info("choose or enter a worktree"));
                return;
            }
            LaunchTarget::Worktree => (form.branch.clone(), None),
        };
        let msg = if branch.is_empty() {
            "building new cockpit…"
        } else {
            "resolving worktree, running hooks…"
        };
        self.pending = Some(Pending::Launch {
            dir: form.dir.clone(),
            workspace,
            agent: self.agents.get(form.agent).cloned(),
            branch,
            dangerous: form.dangerous,
        });
        self.status = Some(Status::info(msg));
    }

    fn key_new_path(&mut self, key: KeyEvent) {
        let Mode::NewPath { input } = &mut self.mode else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.mode = Mode::List,
            KeyCode::Backspace => {
                input.pop();
            }
            KeyCode::Char(c) => input.push(c),
            KeyCode::Enter => self.submit_new_path(),
            _ => {}
        }
    }

    fn submit_new_path(&mut self) {
        let Mode::NewPath { input } = &self.mode else {
            return;
        };
        let raw = input.trim().to_string();
        if raw.is_empty() {
            self.mode = Mode::List;
            return;
        }
        let mut path = ext::expand_tilde(&raw);
        if !path.starts_with('/') {
            path = format!("{}/{path}", ext::home());
        }
        match std::fs::create_dir_all(&path) {
            Ok(()) => self.open_launch_form(PathBuf::from(path), None),
            Err(error) => {
                self.status = Some(Status::err(format!("mkdir failed: {error}")));
                self.mode = Mode::List;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app() -> App {
        let (preview_tx, _preview_requests) = mpsc::channel();
        let (_preview_results_tx, preview_rx) = mpsc::channel();
        App {
            entries: vec![],
            filter: String::new(),
            filtered: vec![],
            selected: 0,
            mode: Mode::List,
            preview: HashMap::new(),
            agents: vec!["claude".into(), "codex".into()],
            source: Source::Projects,
            session_agent: None,
            status: None,
            pending: None,
            quit: false,
            palette: crate::theme::Palette::terminal(),
            hit_regions: vec![],
            hovered: None,
            preview_tx,
            preview_rx,
            requested: HashSet::new(),
            candidates_rx: None,
            cleanup_rx: None,
            cleanup_loading: false,
        }
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn launch_form(branches: &[&str]) -> LaunchForm {
        LaunchForm {
            dir: PathBuf::from("/repo"),
            workspace: None,
            target: LaunchTarget::Worktree,
            agent: 0,
            branch: String::new(),
            candidates: branches
                .iter()
                .map(|branch| ext::WorktreeCandidate {
                    branch: (*branch).into(),
                    path: PathBuf::from(format!("/repo.wt/{branch}")),
                    current: false,
                })
                .collect(),
            candidates_loading: false,
            candidate_selected: None,
            dangerous: true,
            field: 2,
        }
    }

    #[test]
    fn checkout_candidates_filter_and_accept_without_blocking_free_form_input() {
        let mut form = launch_form(&["main", "feature/picker", "fix-preview"]);
        form.branch = "pick".into();
        assert_eq!(form.matching_candidates(), vec![1]);
        assert!(!form.accept_candidate());

        form.move_candidate(1);
        assert!(form.accept_candidate());
        assert_eq!(form.branch, "feature/picker");
        assert_eq!(form.candidate_selected, None);
    }

    #[test]
    fn create_hint_only_for_free_form_input_without_exact_match() {
        let mut form = launch_form(&["main", "feature/picker"]);
        assert_eq!(form.pending_create(), None); // empty input

        form.branch = "pick".into(); // fuzzy match exists, but Enter creates
        assert_eq!(form.pending_create(), Some("pick"));

        form.move_candidate(1); // highlighted candidate: Enter accepts it
        assert_eq!(form.pending_create(), None);
        form.candidate_selected = None;

        form.branch = "feature/picker".into(); // exact worktree: opens it
        assert_eq!(form.pending_create(), None);

        for shortcut in ["^", "-", "@", "pr:12"] {
            form.branch = shortcut.into(); // worktrunk resolves these
            assert_eq!(form.pending_create(), None, "{shortcut}");
        }

        form.branch = "  spaced  ".into();
        assert_eq!(form.pending_create(), Some("spaced"));

        // While candidates are still loading, an exact match may be on its
        // way, so no create hint.
        form.candidates_loading = true;
        assert_eq!(form.pending_create(), None);
    }

    #[test]
    fn checkout_candidate_navigation_wraps() {
        let mut form = launch_form(&["main", "feature/picker"]);
        form.move_candidate(-1);
        assert_eq!(form.candidate_selected, Some(1));
        form.move_candidate(1);
        assert_eq!(form.candidate_selected, Some(0));
    }

    #[test]
    fn launch_fields_skip_the_hidden_worktree_input() {
        let mut form = launch_form(&["main"]);
        form.set_target(LaunchTarget::SameCheckout);
        form.move_field(1);
        assert_eq!(form.field, 1);
        form.move_field(1);
        assert_eq!(form.field, 3);
        form.move_field(1);
        assert_eq!(form.field, 0);
        form.move_field(-1);
        assert_eq!(form.field, 3);

        form.set_target(LaunchTarget::Worktree);
        form.move_field(1);
        form.move_field(1);
        assert_eq!(form.field, 2);
    }

    #[test]
    fn launch_target_controls_checkout_handoff() {
        let mut app = test_app();
        let mut form = launch_form(&["main", "feature/picker"]);
        form.target = LaunchTarget::SameCheckout;
        form.branch = "stale-branch".into();
        app.mode = Mode::Launch(form);
        app.submit_launch();
        assert!(matches!(
            app.pending,
            Some(Pending::Launch { ref branch, .. }) if branch.is_empty()
        ));

        app.pending = None;
        let mut form = launch_form(&["main"]);
        form.branch.clear();
        app.mode = Mode::Launch(form);
        app.submit_launch();
        assert!(app.pending.is_none());
        assert_eq!(
            app.status.as_ref().map(|status| status.msg.as_str()),
            Some("choose or enter a worktree")
        );

        let mut form = launch_form(&["main"]);
        form.branch = "feature/new-cockpit".into();
        app.mode = Mode::Launch(form);
        app.submit_launch();
        assert!(matches!(
            app.pending,
            Some(Pending::Launch { ref branch, .. }) if branch == "feature/new-cockpit"
        ));
    }

    #[test]
    fn new_cockpit_uses_the_selected_workspace_directory() {
        let mut app = test_app();
        app.entries.push(Entry {
            label: "project/main".into(),
            kind: EntryKind::Workspace {
                id: "workspace-1".into(),
                status: "idle".into(),
                cwd: Some(PathBuf::from("/repo")),
            },
        });
        app.filtered.push(0);

        app.open_new_cockpit();
        let Mode::Launch(form) = &app.mode else {
            panic!("new cockpit should open the launch form");
        };
        assert_eq!(form.dir, PathBuf::from("/repo"));
        assert_eq!(form.workspace.as_deref(), Some("workspace-1"));
        assert_eq!(form.target, LaunchTarget::SameCheckout);

        app.submit_launch();
        assert!(matches!(
            app.pending,
            Some(Pending::Launch {
                workspace: Some(ref id),
                ref branch,
                ..
            }) if id == "workspace-1" && branch.is_empty()
        ));
    }

    #[test]
    fn mouse_hover_and_wheel_follow_visible_result_rows() {
        let mut app = test_app();
        app.filtered = (0..6).collect();
        app.hit_regions = vec![
            HitRegion::new(Rect::new(0, 0, 20, 6), HitTarget::Results),
            HitRegion::new(Rect::new(0, 2, 20, 1), HitTarget::Result(2)),
        ];

        app.handle_mouse(mouse(MouseEventKind::Moved, 4, 2));
        assert_eq!(app.selected, 2);
        assert_eq!(app.hovered, Some(HitTarget::Result(2)));

        app.handle_mouse(mouse(MouseEventKind::ScrollDown, 4, 2));
        assert_eq!(app.selected, 5);
        app.handle_mouse(mouse(MouseEventKind::ScrollUp, 4, 2));
        assert_eq!(app.selected, 2);
    }

    #[test]
    fn clicking_launch_location_and_candidate_updates_the_form() {
        let mut app = test_app();
        let mut form = launch_form(&["main", "feature/picker"]);
        form.set_target(LaunchTarget::SameCheckout);
        app.mode = Mode::Launch(form);
        app.hit_regions = vec![HitRegion::new(
            Rect::new(3, 2, 20, 1),
            HitTarget::LaunchWorktree,
        )];
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 4, 2));
        let Mode::Launch(form) = &app.mode else {
            panic!("launch form should remain open");
        };
        assert_eq!(form.target, LaunchTarget::Worktree);

        app.mode = Mode::Launch(launch_form(&["main", "feature/picker"]));
        app.hit_regions = vec![HitRegion::new(
            Rect::new(3, 3, 20, 1),
            HitTarget::LaunchCandidate(1),
        )];

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 4, 3));
        let Mode::Launch(form) = &app.mode else {
            panic!("launch form should remain open");
        };
        assert_eq!(form.branch, "feature/picker");

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 40, 20));
        assert!(matches!(app.mode, Mode::List));
    }

    #[test]
    fn confirmation_click_queues_the_existing_delete_action() {
        let mut app = test_app();
        app.mode = Mode::ConfirmDelete {
            msg: "close workspace?".into(),
            action: DelAction::CloseWs("workspace-1".into()),
        };
        app.hit_regions = vec![HitRegion::new(Rect::new(3, 3, 12, 1), HitTarget::Confirm)];

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 4, 3));
        assert!(matches!(
            app.pending,
            Some(Pending::Delete(DelAction::CloseWs(_)))
        ));
        assert!(matches!(app.mode, Mode::List));
    }

    #[test]
    fn force_removal_still_requires_f_before_y_from_the_keyboard() {
        let mut app = test_app();
        app.mode = Mode::ConfirmDelete {
            msg: "not merged".into(),
            action: DelAction::OfferForce(PathBuf::from("/repo.wt/topic"), "topic".into()),
        };

        app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
        assert!(matches!(
            app.mode,
            Mode::ConfirmDelete {
                action: DelAction::OfferForce(..),
                ..
            }
        ));

        app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
        assert!(matches!(
            app.mode,
            Mode::ConfirmDelete {
                action: DelAction::ForceArmed(..),
                ..
            }
        ));
    }
}
