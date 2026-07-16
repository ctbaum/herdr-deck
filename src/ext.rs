//! Subprocess wrappers around herdr / wt / zoxide / fd / git, plus the
//! opinionated deck builder.

use crate::sessions::{Agent as SessionAgent, Session};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn out(argv: &[&str]) -> Option<String> {
    let o = Command::new(argv[0]).args(&argv[1..]).output().ok()?;
    if !o.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&o.stdout).into_owned())
}

pub fn json(argv: &[&str]) -> Option<Value> {
    serde_json::from_str(&out(argv)?).ok()
}

pub fn home() -> String {
    std::env::var("HOME").unwrap_or_default()
}

pub fn expand_tilde(p: &str) -> String {
    match p.strip_prefix("~") {
        Some(rest) => format!("{}{}", home(), rest),
        None => p.to_string(),
    }
}

pub fn collapse_tilde(p: &str) -> String {
    match p.strip_prefix(&home()) {
        Some(rest) => format!("~{rest}"),
        None => p.to_string(),
    }
}

// ---------------------------------------------------------------------------
// herdr workspaces

pub struct Ws {
    pub id: String,
    pub label: String,
    pub status: String,
}

/// Live workspaces, blocked agents first, then clustered by project basename
/// in first-appearance order.
pub fn workspaces() -> Vec<Ws> {
    let Some(v) = json(&["herdr", "workspace", "list"]) else {
        return vec![];
    };
    let mut ws: Vec<Ws> = v["result"]["workspaces"]
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .map(|w| Ws {
            id: w["workspace_id"].as_str().unwrap_or("").into(),
            label: w["label"].as_str().unwrap_or("").into(),
            status: w["agent_status"].as_str().unwrap_or("").into(),
        })
        .collect();
    let mut first: HashMap<String, usize> = HashMap::new();
    for (i, w) in ws.iter().enumerate() {
        let g = w.label.split('/').next().unwrap_or("").to_string();
        first.entry(g).or_insert(i);
    }
    ws.sort_by_key(|w| {
        let rank = if w.status == "blocked" { 0 } else { 1 };
        let g = w.label.split('/').next().unwrap_or("");
        (rank, first.get(g).copied().unwrap_or(usize::MAX))
    });
    ws
}

pub fn ws_id_for_label(label: &str) -> Option<String> {
    workspaces()
        .into_iter()
        .find(|w| w.label == label)
        .map(|w| w.id)
}

pub fn focus_workspace(id: &str) {
    out(&["herdr", "workspace", "focus", id]);
}

pub fn close_workspace(id: &str) {
    out(&["herdr", "workspace", "close", id]);
}

fn close_pane(id: &str) {
    out(&["herdr", "pane", "close", id]);
}

fn herdr_worktree_list(path: &Path) -> Option<Value> {
    let cwd = path.to_str()?;
    json(&["herdr", "worktree", "list", "--cwd", cwd, "--json"])
}

fn project_name_from_repo_key(key: &str) -> Option<String> {
    let path = Path::new(key);
    let name = path.file_name()?.to_string_lossy();
    if matches!(name.as_ref(), ".git" | ".bare" | "bare") {
        return path
            .parent()?
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
    }
    if let Some(name) = name.strip_suffix(".git")
        && !name.is_empty()
    {
        return Some(name.to_string());
    }
    Some(name.into_owned())
}

fn herdr_repo_name(value: &Value) -> Option<String> {
    let source = &value["result"]["source"];
    source["repo_key"]
        .as_str()
        .and_then(project_name_from_repo_key)
        .or_else(|| {
            source["repo_name"]
                .as_str()
                .filter(|name| !name.is_empty())
                .map(String::from)
        })
}

// ---------------------------------------------------------------------------
// remote herdr servers

/// Remote herdr hosts: ssh aliases from $HERDR_DECK_REMOTES (comma/space
/// separated). Running `herdr --remote` inside a pane would nest herdr in
/// herdr, so open_remote gives each remote its own terminal window instead.
pub fn remotes() -> Vec<String> {
    std::env::var("HERDR_DECK_REMOTES")
        .unwrap_or_default()
        .split([',', ' '])
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

// ponytail: macOS + Ghostty hardcoded; make the spawn command a config knob
// when someone on another terminal or Linux wants this
pub fn open_remote(host: &str) {
    // `open` forwards the caller's env into the new app, and herdr-deck runs
    // inside a herdr pane; the inherited HERDR_* vars make the thin client
    // refuse to start ("nested herdr"), so scrub them.
    let mut c = Command::new("open");
    c.args(["-na", "Ghostty", "--args", "-e", "herdr", "--remote", host]);
    for (k, _) in std::env::vars().filter(|(k, _)| k.starts_with("HERDR_")) {
        c.env_remove(k);
    }
    let _ = c.output();
}

// ---------------------------------------------------------------------------
// directory tiers

/// (worktrees, other dirs): zoxide frecency then fd fallback, existing dirs
/// only, deduped; linked git worktrees floated into their own tier. A linked
/// worktree is any dir whose `.git` is a file (`gitdir: …`) rather than a
/// directory — no dependence on the worktree-path layout.
pub fn dirs() -> (Vec<PathBuf>, Vec<PathBuf>) {
    let z = out(&["zoxide", "query", "-l"]).unwrap_or_default();
    let f = out(&[
        "fd",
        "-H",
        "-d",
        "2",
        "-t",
        "d",
        "-E",
        ".Trash",
        ".",
        &home(),
    ])
    .unwrap_or_default();
    let mut seen = HashSet::new();
    let (mut wt, mut other) = (vec![], vec![]);
    for line in z.lines().chain(f.lines()) {
        let p = line.trim_end_matches('/');
        if p.is_empty() || !seen.insert(p.to_string()) || !Path::new(p).is_dir() {
            continue;
        }
        if Path::new(p).join(".git").is_file() {
            wt.push(PathBuf::from(p));
        } else {
            other.push(PathBuf::from(p));
        }
    }
    (wt, other)
}

// ---------------------------------------------------------------------------
// worktrunk

pub struct WtInfo {
    pub branch: String,
    pub main_state: String,
    pub staged: bool,
    pub modified: bool,
    pub untracked: bool,
    pub renamed: bool,
    pub deleted: bool,
    pub ahead: i64,
    pub behind: i64,
}

impl WtInfo {
    pub fn integrated(&self) -> bool {
        matches!(self.main_state.as_str(), "integrated" | "empty")
    }

    pub fn clean(&self) -> bool {
        !self.staged && !self.modified && !self.untracked && !self.renamed && !self.deleted
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntegratedWt {
    pub path: PathBuf,
    pub project: String,
    pub branch: String,
    pub clean: bool,
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct BatchRemoval {
    pub removed: usize,
    pub skipped: usize,
    pub failed: usize,
}

fn integrated_rows(value: &Value, project: &str) -> Vec<IntegratedWt> {
    value
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter(|row| {
            row["is_main"].as_bool() != Some(true)
                && matches!(row["main_state"].as_str(), Some("integrated" | "empty"))
        })
        .filter_map(|row| {
            let path = PathBuf::from(row["path"].as_str()?);
            let branch = row["branch"]
                .as_str()
                .filter(|branch| !branch.is_empty())
                .map(String::from)
                .or_else(|| {
                    path.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                })?;
            let tree = &row["working_tree"];
            let clean = ["staged", "modified", "untracked", "renamed", "deleted"]
                .into_iter()
                .all(|flag| tree[flag].as_bool() != Some(true));
            Some(IntegratedWt {
                path,
                project: project.to_string(),
                branch,
                clean,
            })
        })
        .collect()
}

/// Integrated/empty linked worktrees from every repository represented in the
/// normal directory source. Worktrunk is queried once per common git dir, so a
/// single known checkout discovers its sibling worktrees too.
pub fn integrated_worktrees() -> Vec<IntegratedWt> {
    let (seeds, _) = dirs();
    let mut repositories = HashSet::new();
    let mut seen_paths = HashSet::new();
    let mut result = Vec::new();
    for seed in seeds {
        let Some(seed_str) = seed.to_str() else {
            continue;
        };
        let common = out(&["git", "-C", seed_str, "rev-parse", "--git-common-dir"])
            .map(|path| PathBuf::from(path.trim()))
            .unwrap_or_else(|| seed.clone());
        let common = if common.is_absolute() {
            common
        } else {
            seed.join(common)
        };
        if !repositories.insert(common.clone()) {
            continue;
        }
        let project = project_name_from_repo_key(&common.to_string_lossy())
            .unwrap_or_else(|| common.to_string_lossy().into_owned());
        let Some(rows) = json(&["wt", "-C", seed_str, "list", "--format=json"]) else {
            continue;
        };
        for worktree in integrated_rows(&rows, &project) {
            if worktree.path.is_dir() && seen_paths.insert(worktree.path.clone()) {
                result.push(worktree);
            }
        }
    }
    result.sort_by(|a, b| (&a.project, &a.branch).cmp(&(&b.project, &b.branch)));
    result
}

#[derive(Debug, PartialEq, Eq)]
struct SwitchResult {
    path: PathBuf,
    branch: String,
}

fn worktrunk_is_shortcut(name: &str) -> bool {
    matches!(name, "^" | "-" | "@") || name.contains(':')
}

fn parse_switch_result(value: &Value) -> Option<SwitchResult> {
    let path = value["path"].as_str().filter(|path| !path.is_empty())?;
    Some(SwitchResult {
        path: PathBuf::from(path),
        branch: value["branch"].as_str().unwrap_or_default().to_string(),
    })
}

fn branch_exists(dir: &Path, branch: &str) -> bool {
    let Some(d) = dir.to_str() else { return false };
    let local = format!("refs/heads/{branch}");
    if out(&["git", "-C", d, "show-ref", "--verify", "--quiet", &local]).is_some() {
        return true;
    }
    let remote = format!("refs/remotes/{branch}");
    if out(&["git", "-C", d, "show-ref", "--verify", "--quiet", &remote]).is_some() {
        return true;
    }
    out(&[
        "git",
        "-C",
        d,
        "for-each-ref",
        "--format=%(refname:strip=3)",
        "refs/remotes",
    ])
    .is_some_and(|branches| branches.lines().any(|candidate| candidate == branch))
}

fn wt_switch(dir: &Path, selection: &str) -> Result<SwitchResult, String> {
    let d = dir
        .to_str()
        .ok_or_else(|| "repository path is not valid UTF-8".to_string())?;
    let create = !worktrunk_is_shortcut(selection) && !branch_exists(dir, selection);
    let mut argv = vec!["wt", "-C", d, "switch"];
    if create {
        argv.push("--create");
    }
    argv.extend([selection, "--no-cd", "--format=json"]);
    let value = json(&argv).ok_or_else(|| format!("wt switch failed for '{selection}'"))?;
    parse_switch_result(&value)
        .ok_or_else(|| format!("wt switch returned no worktree path for '{selection}'"))
}

#[derive(Default, Debug, PartialEq, Eq)]
struct RemovalCleanup {
    workspaces: Vec<String>,
    panes: Vec<String>,
}

fn path_is_within(candidate: &str, root: &Path) -> bool {
    Path::new(candidate).starts_with(root)
}

fn pane_cleanup_targets(value: &Value, root: &Path, own_pane: &str) -> RemovalCleanup {
    let mut by_workspace: HashMap<String, Vec<(String, bool)>> = HashMap::new();
    for pane in value["result"]["panes"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let Some(workspace) = pane["workspace_id"].as_str() else {
            continue;
        };
        let id = pane["pane_id"].as_str().unwrap_or_default().to_string();
        let matches = [pane["cwd"].as_str(), pane["foreground_cwd"].as_str()]
            .into_iter()
            .flatten()
            .any(|cwd| path_is_within(cwd, root));
        by_workspace
            .entry(workspace.to_string())
            .or_default()
            .push((id, matches));
    }

    let mut cleanup = RemovalCleanup::default();
    for (workspace, panes) in by_workspace {
        if panes.iter().any(|(_, matches)| *matches) && panes.iter().all(|(_, matches)| *matches) {
            cleanup.workspaces.push(workspace);
        } else {
            cleanup.panes.extend(
                panes
                    .into_iter()
                    .filter(|(id, matches)| *matches && id != own_pane)
                    .map(|(id, _)| id),
            );
        }
    }
    cleanup.workspaces.sort();
    cleanup.panes.sort();
    cleanup
}

fn removal_cleanup(path: &Path) -> RemovalCleanup {
    let own_pane = std::env::var("HERDR_PANE_ID").unwrap_or_default();
    let mut cleanup = json(&["herdr", "pane", "list"])
        .map(|value| pane_cleanup_targets(&value, path, &own_pane))
        .unwrap_or_default();

    if let Some(value) = herdr_worktree_list(path)
        && let Some(workspace) = value["result"]["worktrees"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|worktree| {
                worktree["path"]
                    .as_str()
                    .is_some_and(|p| Path::new(p) == path)
            })
            .and_then(|worktree| worktree["open_workspace_id"].as_str())
        && !cleanup.workspaces.iter().any(|id| id == workspace)
    {
        cleanup.workspaces.push(workspace.to_string());
        cleanup.workspaces.sort();
    }
    cleanup
}

/// is_current row of `wt -C <path> list --format json`.
pub fn wt_info(path: &Path) -> Option<WtInfo> {
    let p = path.to_str()?;
    let rows = json(&["wt", "-C", p, "list", "--format", "json"])?;
    let w = rows
        .as_array()?
        .iter()
        .find(|w| w["is_current"].as_bool() == Some(true))?;
    Some(WtInfo {
        branch: w["branch"].as_str().unwrap_or("").into(),
        main_state: w["main_state"].as_str().unwrap_or("").into(),
        staged: w["working_tree"]["staged"].as_bool().unwrap_or(false),
        modified: w["working_tree"]["modified"].as_bool().unwrap_or(false),
        untracked: w["working_tree"]["untracked"].as_bool().unwrap_or(false),
        renamed: w["working_tree"]["renamed"].as_bool().unwrap_or(false),
        deleted: w["working_tree"]["deleted"].as_bool().unwrap_or(false),
        ahead: w["main"]["ahead"].as_i64().unwrap_or(0),
        behind: w["main"]["behind"].as_i64().unwrap_or(0),
    })
}

pub fn wt_remove(path: &Path, force: bool) -> bool {
    let cleanup = removal_cleanup(path);
    let p = path.to_string_lossy();
    let ok = if force {
        out(&["wt", "-C", &p, "remove", "-f", "-D"]).is_some()
    } else {
        out(&["wt", "-C", &p, "remove"]).is_some()
    };
    if ok {
        for workspace in cleanup.workspaces {
            close_workspace(&workspace);
        }
        for pane in cleanup.panes {
            close_pane(&pane);
        }
        zoxide_purge(path);
    }
    ok
}

/// Revalidate every candidate immediately before removal. No force flags are
/// ever used: a worktree that became dirty or unintegrated is skipped, and a
/// Worktrunk refusal is reported as a failure.
pub fn wt_remove_clean(paths: &[PathBuf]) -> BatchRemoval {
    let mut result = BatchRemoval::default();
    for path in paths {
        let Some(info) = wt_info(path) else {
            result.failed += 1;
            continue;
        };
        if !info.integrated() || !info.clean() {
            result.skipped += 1;
        } else if wt_remove(path, false) {
            result.removed += 1;
        } else {
            result.failed += 1;
        }
    }
    result
}

/// Drop a removed worktree's path and every zoxide entry beneath it. The `/`
/// boundary stops `foo` from purging `foo-2`.
pub fn zoxide_purge(root: &Path) {
    let Some(list) = out(&["zoxide", "query", "-l"]) else {
        return;
    };
    let root = root.to_string_lossy();
    let prefix = format!("{root}/");
    for p in list.lines() {
        if p == root || p.starts_with(&prefix) {
            out(&["zoxide", "remove", p]);
        }
    }
}

// ---------------------------------------------------------------------------
// deck builder

/// How an agent enters yolo/dangerous mode when the launch toggle is on.
/// `Flag` is appended to the command; `Env` prefixes the pane command; `None`
/// means no toggle (the agent has no known dangerous-mode mechanism).
enum Dangerous {
    Flag(&'static str),
    Env(&'static str),
    None,
}

/// herdr agent id → (PATH binary, dangerous mechanism). Binary is the id
/// unless listed otherwise (cursor's binary is cursor-agent). Flags per the
/// per-agent yolo-flags research table. Doubles as the fallback agent
/// list when `herdr integration status` can't be read.
struct Spec {
    id: &'static str,
    bin: &'static str,
    danger: Dangerous,
}

const AGENTS: &[Spec] = &[
    Spec {
        id: "claude",
        bin: "claude",
        danger: Dangerous::Flag("--dangerously-skip-permissions"),
    },
    Spec {
        id: "codex",
        bin: "codex",
        danger: Dangerous::Flag("--dangerously-bypass-approvals-and-sandbox"),
    },
    Spec {
        id: "copilot",
        bin: "copilot",
        danger: Dangerous::Flag("--yolo"),
    },
    Spec {
        id: "cursor",
        bin: "cursor-agent",
        danger: Dangerous::Flag("--force"),
    },
    Spec {
        id: "devin",
        bin: "devin",
        danger: Dangerous::Flag("--permission-mode dangerous"),
    },
    Spec {
        id: "droid",
        bin: "droid",
        danger: Dangerous::Flag("--skip-permissions-unsafe"),
    },
    Spec {
        id: "kimi",
        bin: "kimi",
        danger: Dangerous::Flag("--yolo"),
    },
    Spec {
        id: "opencode",
        bin: "opencode",
        danger: Dangerous::Env("OPENCODE_PERMISSION='{\"*\":\"allow\"}'"),
    },
    Spec {
        id: "kilo",
        bin: "kilo",
        danger: Dangerous::Flag("--auto"),
    },
    Spec {
        id: "hermes",
        bin: "hermes",
        danger: Dangerous::Flag("--yolo"),
    },
    Spec {
        id: "qodercli",
        bin: "qodercli",
        danger: Dangerous::Flag("--yolo"),
    },
    Spec {
        id: "pi",
        bin: "pi",
        danger: Dangerous::None,
    },
    Spec {
        id: "omp",
        bin: "omp",
        danger: Dangerous::Flag("--yolo"),
    },
    Spec {
        id: "mastracode",
        bin: "mastracode",
        danger: Dangerous::None,
    },
];

fn spec(id: &str) -> Option<&'static Spec> {
    AGENTS.iter().find(|s| s.id == id)
}

fn agent_binary(id: &str) -> &str {
    spec(id).map(|s| s.bin).unwrap_or(id)
}

/// Whether the launch form's dangerous toggle does anything for this agent.
/// False for agents without a known dangerous mode and unknown ids.
pub fn dangerous_toggleable(id: &str) -> bool {
    matches!(
        spec(id).map(|s| &s.danger),
        Some(Dangerous::Flag(_) | Dangerous::Env(_))
    )
}

/// Pane command for a non-editor agent, applying its dangerous mechanism when
/// `on`: append a flag, or prefix an env. Uses the PATH binary (cursor-agent).
fn agent_command(id: &str, on: bool) -> String {
    let bin = agent_binary(id);
    match (on, spec(id).map(|s| &s.danger)) {
        (true, Some(Dangerous::Flag(f))) => format!("{bin} {f}"),
        (true, Some(Dangerous::Env(e))) => format!("{e} {bin}"),
        _ => bin.to_string(),
    }
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

fn claude_launch_args(resume: Option<&Session>, dangerous: bool) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(session) = resume.filter(|s| s.agent == SessionAgent::Claude) {
        args.extend(["--resume".into(), session.id.clone()]);
    }
    if dangerous {
        args.push("--dangerously-skip-permissions".into());
    }
    args
}

fn codex_launch_args(resume: Option<&Session>, dangerous: bool) -> Vec<String> {
    if let Some(session) = resume.filter(|s| s.agent == SessionAgent::Codex) {
        return vec![
            "resume".into(),
            session.id.clone(),
            "--dangerously-bypass-approvals-and-sandbox".into(),
        ];
    }
    if dangerous {
        vec!["--dangerously-bypass-approvals-and-sandbox".into()]
    } else {
        Vec::new()
    }
}

fn shell_join(args: &[String]) -> String {
    args.iter()
        .map(|arg| shell_quote(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

fn session_command(session: &Session) -> String {
    match session.agent {
        SessionAgent::Claude => String::new(), // spawned inside nvim via HERDR_NVIM_AGENT
        SessionAgent::Codex => format!(
            "codex {}",
            shell_join(&codex_launch_args(Some(session), true))
        ),
        SessionAgent::Cursor if session.native_picker => "cursor-agent ls".into(),
        SessionAgent::Cursor => format!(
            "{} --resume {}",
            agent_command("cursor", true),
            shell_quote(&session.id)
        ),
        SessionAgent::Pi => match &session.file {
            Some(path) => format!("pi --session {}", shell_quote(&path.to_string_lossy())),
            None => format!("pi --session {}", shell_quote(&session.id)),
        },
    }
}

/// Supported agent ids from `herdr integration status` (line format
/// "claude: current (v7) (/path)"), falling back to the AGENTS table.
fn supported_agent_ids() -> Vec<String> {
    let Some(text) = out(&["herdr", "integration", "status"]) else {
        return AGENTS.iter().map(|s| s.id.to_string()).collect();
    };
    text.lines()
        .filter_map(|l| l.split_once(':').map(|(id, _)| id.trim().to_string()))
        .filter(|id| !id.is_empty())
        .collect()
}

pub fn detected_agents() -> Vec<String> {
    let path = std::env::var("PATH").unwrap_or_default();
    let dirs: Vec<&str> = path.split(':').filter(|d| !d.is_empty()).collect();
    supported_agent_ids()
        .into_iter()
        .filter(|id| {
            let bin = agent_binary(id);
            dirs.iter().any(|d| Path::new(d).join(bin).is_file())
        })
        .collect()
}

fn project_name_from_worktrees(porcelain: &str) -> Option<String> {
    let first = porcelain.split("\n\n").next()?;
    let mut lines = first.lines();
    let root = lines.next()?.strip_prefix("worktree ")?.trim();
    let bare = lines.any(|line| line == "bare");
    let path = Path::new(root);
    let name = path.file_name()?.to_string_lossy();

    if bare && matches!(name.as_ref(), "bare" | ".bare") {
        return path
            .parent()?
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
    }
    if bare
        && let Some(name) = name.strip_suffix(".git")
        && !name.is_empty()
    {
        return Some(name.to_string());
    }
    Some(name.into_owned())
}

fn in_git_repo(dir: &Path) -> bool {
    let Some(d) = dir.to_str() else { return false };
    out(&["git", "-C", d, "rev-parse", "--git-dir"]).is_some()
}

/// Build a deck workspace: nvim + agent + full-width terminal + lazygit
/// tab. `agent` is a detected agent name, or None for a plain deck.
/// Returns Ok(()) once the workspace is focused (caller should quit).
pub fn launch_deck(
    dir: &Path,
    agent: Option<&str>,
    branch: &str,
    dangerous: bool,
) -> Result<(), String> {
    launch_deck_inner(dir, agent, branch, dangerous, None)
}

/// Recreate the normal deck at a saved session's original cwd, with
/// the agent pane (or Claude-in-nvim provider) resuming that conversation.
pub fn launch_session_deck(session: &Session) -> Result<(), String> {
    if !session.cwd.is_dir() {
        return Err(format!(
            "session directory no longer exists: {}",
            collapse_tilde(&session.cwd.to_string_lossy())
        ));
    }
    launch_deck_inner(
        &session.cwd,
        Some(session.agent.id()),
        "",
        true,
        Some(session),
    )
}

fn launch_deck_inner(
    dir: &Path,
    agent: Option<&str>,
    branch: &str,
    dangerous: bool,
    resume: Option<&Session>,
) -> Result<(), String> {
    let mut target = dir.to_path_buf();
    let mut resolved_branch = String::new();

    // Worktrunk owns worktree creation, hook execution, shortcut/PR resolution,
    // and the resulting path. Its JSON result avoids guessing the configured
    // worktree path from the requested branch name.
    if !branch.is_empty() {
        if !in_git_repo(dir) {
            return Err("not a git repository (branch given)".into());
        }
        let switched = wt_switch(dir, branch)?;
        target = switched.path;
        resolved_branch = switched.branch;
    }
    let target_str = target.to_string_lossy().into_owned();

    // Prefer Herdr's canonical repository identity; it already understands
    // bare backing stores and linked worktrees. Git porcelain is the fallback
    // for older Herdr versions or directories not registered with Herdr.
    let mut project = String::new();
    let mut br = String::new();
    if in_git_repo(&target) {
        let herdr_meta = herdr_worktree_list(&target);
        if let Some(name) = herdr_meta.as_ref().and_then(herdr_repo_name) {
            project = name;
        } else if let Some(porcelain) =
            out(&["git", "-C", &target_str, "worktree", "list", "--porcelain"])
            && let Some(name) = project_name_from_worktrees(&porcelain)
        {
            project = name;
        }
        br = out(&["git", "-C", &target_str, "symbolic-ref", "--short", "HEAD"])
            .map(|s| s.trim().to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| resolved_branch.clone());
    }
    let basename = target
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| target_str.clone());
    // Keep the cockpit tab anchored to the project rather than the deck
    // implementation. For linked worktrees this is the main worktree's
    // basename; plain directories use their own basename.
    let tab_name = if project.is_empty() {
        basename.clone()
    } else {
        project.clone()
    };
    let mut label = match (project.is_empty(), br.is_empty()) {
        (false, false) => format!("{project}/{br}"),
        (false, true) => project,
        (true, _) => basename,
    };
    // A resumed conversation gets its own deck instead of accidentally
    // focusing a live generic project deck that does not contain it.
    if let Some(session) = resume {
        let tag = if session.native_picker {
            "sessions".to_string()
        } else {
            session.id.chars().take(8).collect()
        };
        label = format!("{label}/{}-{tag}", session.agent.id());
    }

    // A deck for this label already exists: focus it instead of rebuilding.
    if let Some(existing) = ws_id_for_label(&label) {
        focus_workspace(&existing);
        return Ok(());
    }

    // Env via `workspace create --env` (a `pane run` prefix would be echoed
    // onto the pane). Claude and Codex both start through nvim so their IDE
    // servers exist before the Herdr terminal provider launches the agent.
    let launch_args = match agent {
        Some("claude") => claude_launch_args(resume, dangerous),
        Some("codex") => codex_launch_args(resume, dangerous),
        _ => Vec::new(),
    };
    let launch_args = serde_json::to_string(&launch_args)
        .map_err(|error| format!("could not encode editor-agent arguments: {error}"))?;
    let launch_args = format!("HERDR_NVIM_AGENT_ARGS_JSON={launch_args}");
    let mut create: Vec<&str> = vec![
        "herdr",
        "workspace",
        "create",
        "--cwd",
        &target_str,
        "--label",
        &label,
        "--no-focus",
    ];
    if agent == Some("claude") {
        create.extend(["--env", "HERDR_NVIM_AGENT=claude", "--env", &launch_args]);
    } else if agent == Some("codex") {
        create.extend(["--env", "HERDR_NVIM_AGENT=codex", "--env", &launch_args]);
    }
    let created = json(&create).ok_or("herdr workspace create failed")?;
    let root_pane = &created["result"]["root_pane"];
    let ws = root_pane["workspace_id"]
        .as_str()
        .ok_or("no workspace_id in create result")?;
    let root = root_pane["pane_id"]
        .as_str()
        .ok_or("no pane_id in create result")?;
    let root_tab = root_pane["tab_id"].as_str().unwrap_or("");

    out(&["herdr", "tab", "rename", root_tab, &tab_name]);

    // Full-width terminal on the bottom row.
    out(&[
        "herdr",
        "pane",
        "split",
        root,
        "--direction",
        "down",
        "--ratio",
        "0.8",
        "--cwd",
        &target_str,
        "--no-focus",
    ]);

    match agent {
        // Editor-integrated agents are spawned by nvim after their IDE server
        // starts (env above). None gets the same layout with plain nvim.
        Some("claude") | Some("codex") | None => {
            out(&["herdr", "pane", "run", root, "nvim"]);
        }
        // Every other agent gets its own pane on the top-right. Dangerous is
        // agent-specific (AGENTS table): append a flag, or prefix an env.
        Some(a) => {
            let cmd = resume
                .map(session_command)
                .unwrap_or_else(|| agent_command(a, dangerous));
            let split = json(&[
                "herdr",
                "pane",
                "split",
                root,
                "--direction",
                "right",
                "--ratio",
                "0.7",
                "--cwd",
                &target_str,
                "--no-focus",
            ])
            .ok_or("pane split for agent failed")?;
            let agent_pane = split["result"]["pane"]["pane_id"]
                .as_str()
                .ok_or("no pane_id in split result")?;
            out(&["herdr", "pane", "run", root, "nvim"]);
            out(&["herdr", "pane", "run", agent_pane, &cmd]);
        }
    }

    // Unfocused lazygit tab — one keystroke away, out of the way.
    if let Some(tab) = json(&[
        "herdr",
        "tab",
        "create",
        "--workspace",
        ws,
        "--cwd",
        &target_str,
        "--label",
        "lazygit",
        "--no-focus",
    ]) && let Some(git_pane) = tab["result"]["root_pane"]["pane_id"].as_str()
    {
        out(&["herdr", "pane", "run", git_pane, "lazygit"]);
    }

    focus_workspace(ws);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_native_repo_identity_and_worktrunk_switch_results() {
        let herdr = serde_json::json!({
            "result": { "source": {
                "repo_key": "/src/herdr-deck/.git",
                "repo_name": "herdr-deck"
            } }
        });
        assert_eq!(herdr_repo_name(&herdr).as_deref(), Some("herdr-deck"));
        let misleading_bare = serde_json::json!({
            "result": { "source": {
                "repo_key": "/src/foresight/.bare",
                "repo_name": "test",
                "repo_root": "/src/foresight/.bare"
            } }
        });
        assert_eq!(
            herdr_repo_name(&misleading_bare).as_deref(),
            Some("foresight")
        );
        assert_eq!(
            project_name_from_repo_key("/src/foresight.git").as_deref(),
            Some("foresight")
        );

        let switched = serde_json::json!({
            "action": "created",
            "branch": "fix-preview",
            "path": "/src/herdr-deck.wt/fix-preview"
        });
        assert_eq!(
            parse_switch_result(&switched),
            Some(SwitchResult {
                path: PathBuf::from("/src/herdr-deck.wt/fix-preview"),
                branch: "fix-preview".into(),
            })
        );
        for shortcut in ["^", "-", "@", "pr:123", "mr:45", "https://host/pr/7"] {
            assert!(worktrunk_is_shortcut(shortcut), "{shortcut}");
        }
        assert!(!worktrunk_is_shortcut("feature/native-metadata"));
    }

    #[test]
    fn cleanup_closes_dedicated_workspaces_but_only_matching_mixed_panes() {
        let panes = serde_json::json!({
            "result": { "panes": [
                { "workspace_id": "dedicated", "pane_id": "d1", "cwd": "/repo.wt/feature" },
                { "workspace_id": "dedicated", "pane_id": "d2", "cwd": "/repo.wt/feature/src" },
                { "workspace_id": "mixed", "pane_id": "m1", "cwd": "/repo" },
                { "workspace_id": "mixed", "pane_id": "m2", "cwd": "/repo.wt/feature" },
                { "workspace_id": "mixed", "pane_id": "own", "cwd": "/repo.wt/feature" },
                { "workspace_id": "other", "pane_id": "o1", "cwd": "/repo.wt/feature-two" }
            ] }
        });
        assert_eq!(
            pane_cleanup_targets(&panes, Path::new("/repo.wt/feature"), "own"),
            RemovalCleanup {
                workspaces: vec!["dedicated".into()],
                panes: vec!["m2".into()],
            }
        );
    }

    #[test]
    fn cleanable_rows_are_integrated_non_main_and_explicitly_clean() {
        let rows = serde_json::json!([
            {
                "branch": "main", "path": "/repo", "is_main": true,
                "main_state": "is_main", "working_tree": {}
            },
            {
                "branch": "landed", "path": "/repo.wt/landed", "is_main": false,
                "main_state": "integrated",
                "working_tree": { "staged": false, "modified": false, "untracked": false }
            },
            {
                "branch": "dirty-but-landed", "path": "/repo.wt/dirty", "is_main": false,
                "main_state": "empty",
                "working_tree": { "modified": true }
            },
            {
                "branch": "in-progress", "path": "/repo.wt/wip", "is_main": false,
                "main_state": "ahead", "working_tree": {}
            }
        ]);
        assert_eq!(
            integrated_rows(&rows, "repo"),
            vec![
                IntegratedWt {
                    path: PathBuf::from("/repo.wt/landed"),
                    project: "repo".into(),
                    branch: "landed".into(),
                    clean: true,
                },
                IntegratedWt {
                    path: PathBuf::from("/repo.wt/dirty"),
                    project: "repo".into(),
                    branch: "dirty-but-landed".into(),
                    clean: false,
                },
            ]
        );
    }

    #[test]
    fn names_projects_from_normal_and_bare_worktree_roots() {
        assert_eq!(
            project_name_from_worktrees(
                "worktree /src/herdr-deck\nHEAD abc\nbranch refs/heads/main\n"
            )
            .as_deref(),
            Some("herdr-deck")
        );
        assert_eq!(
            project_name_from_worktrees("worktree /src/herdr-deck/.bare\nbare\n").as_deref(),
            Some("herdr-deck")
        );
        assert_eq!(
            project_name_from_worktrees("worktree /src/herdr-deck/bare\nbare\n").as_deref(),
            Some("herdr-deck")
        );
        assert_eq!(
            project_name_from_worktrees("worktree /src/herdr-deck.git\nbare\n").as_deref(),
            Some("herdr-deck")
        );
    }

    #[test]
    fn dangerous_command_and_toggle() {
        // Flag agents append; cursor uses its cursor-agent binary.
        assert_eq!(
            agent_command("codex", true),
            "codex --dangerously-bypass-approvals-and-sandbox"
        );
        assert_eq!(agent_command("codex", false), "codex");
        assert_eq!(agent_command("cursor", true), "cursor-agent --force");
        // opencode has no flag: prefix the env instead.
        assert_eq!(
            agent_command("opencode", true),
            "OPENCODE_PERMISSION='{\"*\":\"allow\"}' opencode"
        );
        // Agents with no known mechanism and unknown agents: no-op, and the
        // toggle is greyed.
        assert_eq!(agent_command("pi", true), "pi");
        assert!(!dangerous_toggleable("pi"));
        assert!(dangerous_toggleable("claude"));
        assert!(dangerous_toggleable("codex"));
        assert!(dangerous_toggleable("opencode"));
    }

    #[test]
    fn builds_safe_session_resume_commands() {
        let mut session = Session {
            agent: SessionAgent::Codex,
            id: "abc-123".into(),
            cwd: PathBuf::from("/tmp/project"),
            title: "test".into(),
            updated: 0,
            file: None,
            native_picker: false,
        };
        assert_eq!(
            session_command(&session),
            "codex 'resume' 'abc-123' '--dangerously-bypass-approvals-and-sandbox'"
        );
        assert_eq!(
            codex_launch_args(Some(&session), false),
            vec![
                "resume",
                "abc-123",
                "--dangerously-bypass-approvals-and-sandbox"
            ]
        );
        assert_eq!(
            codex_launch_args(None, true),
            vec!["--dangerously-bypass-approvals-and-sandbox"]
        );
        assert!(codex_launch_args(None, false).is_empty());

        session.agent = SessionAgent::Claude;
        assert_eq!(
            claude_launch_args(Some(&session), true),
            vec!["--resume", "abc-123", "--dangerously-skip-permissions"]
        );
        assert_eq!(
            claude_launch_args(Some(&session), false),
            vec!["--resume", "abc-123"]
        );
        assert_eq!(
            claude_launch_args(None, true),
            vec!["--dangerously-skip-permissions"]
        );
        assert!(claude_launch_args(None, false).is_empty());

        session.agent = SessionAgent::Pi;
        session.file = Some(PathBuf::from("/tmp/project's session.jsonl"));
        assert_eq!(
            session_command(&session),
            "pi --session '/tmp/project'\"'\"'s session.jsonl'"
        );
    }
}
