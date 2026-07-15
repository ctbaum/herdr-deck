//! Subprocess wrappers around herdr / wt / zoxide / fd / git, plus the
//! cockpit builder. Port of nic() and friends from zsh/.config/zsh/nic.zsh;
//! command sequences are kept verbatim so both frontends build identical
//! cockpits.

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
/// in first-appearance order (port of the _nic_pick_source sort).
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
    workspaces().into_iter().find(|w| w.label == label).map(|w| w.id)
}

pub fn focus_workspace(id: &str) {
    out(&["herdr", "workspace", "focus", id]);
}

pub fn close_workspace(id: &str) {
    out(&["herdr", "workspace", "close", id]);
}

// ---------------------------------------------------------------------------
// remote herdr servers

/// Remote herdr hosts: ssh aliases from $WT_COCKPIT_REMOTES (comma/space
/// separated). Running `herdr --remote` inside a pane would nest herdr in
/// herdr, so open_remote gives each remote its own terminal window instead.
pub fn remotes() -> Vec<String> {
    std::env::var("WT_COCKPIT_REMOTES")
        .unwrap_or_default()
        .split([',', ' '])
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

// ponytail: macOS + Ghostty hardcoded; make the spawn command a config knob
// when someone on another terminal or Linux wants this
pub fn open_remote(host: &str) {
    // `open` forwards the caller's env into the new app, and wt-cockpit runs
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
    let f = out(&["fd", "-H", "-d", "2", "-t", "d", "-E", ".Trash", ".", &home()])
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
    pub ahead: i64,
    pub behind: i64,
}

/// is_current row of `wt -C <path> list --format json` (port of _nic_wt_info).
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
        ahead: w["main"]["ahead"].as_i64().unwrap_or(0),
        behind: w["main"]["behind"].as_i64().unwrap_or(0),
    })
}

pub fn wt_remove(path: &Path, force: bool) -> bool {
    let p = path.to_string_lossy();
    let ok = if force {
        out(&["wt", "-C", &p, "remove", "-f", "-D"]).is_some()
    } else {
        out(&["wt", "-C", &p, "remove"]).is_some()
    };
    if ok {
        zoxide_purge(path);
    }
    ok
}

/// Drop a removed worktree's path and every zoxide entry beneath it. The `/`
/// boundary stops `foo` from purging `foo-2` (port of _nic_zoxide_purge).
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
// cockpit builder (port of nic())

/// How an agent enters yolo/dangerous mode when the launch toggle is on.
/// `Flag` is appended to the command; `Env` prefixes the pane command; `None`
/// means no toggle (agent is always-yolo, e.g. pi/mastracode, or claude which
/// gets skip-permissions via NIC_CLAUDE_ARGS — the launch toggle is greyed).
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
    Spec { id: "claude", bin: "claude", danger: Dangerous::None },
    Spec { id: "codex", bin: "codex", danger: Dangerous::Flag("--dangerously-bypass-approvals-and-sandbox") },
    Spec { id: "copilot", bin: "copilot", danger: Dangerous::Flag("--yolo") },
    Spec { id: "cursor", bin: "cursor-agent", danger: Dangerous::Flag("--force") },
    Spec { id: "devin", bin: "devin", danger: Dangerous::Flag("--permission-mode dangerous") },
    Spec { id: "droid", bin: "droid", danger: Dangerous::Flag("--skip-permissions-unsafe") },
    Spec { id: "kimi", bin: "kimi", danger: Dangerous::Flag("--yolo") },
    Spec { id: "opencode", bin: "opencode", danger: Dangerous::Env("OPENCODE_PERMISSION='{\"*\":\"allow\"}'") },
    Spec { id: "kilo", bin: "kilo", danger: Dangerous::Flag("--auto") },
    Spec { id: "hermes", bin: "hermes", danger: Dangerous::Flag("--yolo") },
    Spec { id: "qodercli", bin: "qodercli", danger: Dangerous::Flag("--yolo") },
    Spec { id: "pi", bin: "pi", danger: Dangerous::None },
    Spec { id: "omp", bin: "omp", danger: Dangerous::Flag("--yolo") },
    Spec { id: "mastracode", bin: "mastracode", danger: Dangerous::None },
];

fn spec(id: &str) -> Option<&'static Spec> {
    AGENTS.iter().find(|s| s.id == id)
}

fn agent_binary(id: &str) -> &str {
    spec(id).map(|s| s.bin).unwrap_or(id)
}

/// Whether the launch form's dangerous toggle does anything for this agent.
/// False for always-yolo agents (claude/pi/mastracode) and unknown ids.
pub fn dangerous_toggleable(id: &str) -> bool {
    matches!(spec(id).map(|s| &s.danger), Some(Dangerous::Flag(_) | Dangerous::Env(_)))
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

/// Resolve a branch name to its worktree path (port of _wt_path_for).
fn wt_path_for(dir: &Path, branch: &str) -> Option<PathBuf> {
    let d = dir.to_str()?;
    let porcelain = out(&["git", "-C", d, "worktree", "list", "--porcelain"])?;
    let target = format!("branch refs/heads/{branch}");
    let mut wp = "";
    for line in porcelain.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            wp = p.trim_start();
        } else if line == target {
            return Some(PathBuf::from(wp));
        } else if line.is_empty() {
            wp = "";
        }
    }
    None
}

fn in_git_repo(dir: &Path) -> bool {
    let Some(d) = dir.to_str() else { return false };
    out(&["git", "-C", d, "rev-parse", "--show-toplevel"]).is_some()
}

/// Build a cockpit workspace: nvim + agent + full-width terminal + lazygit
/// tab. `agent` is a detected agent name, or None for a plain cockpit.
/// Returns Ok(()) once the workspace is focused (caller should quit).
pub fn launch_cockpit(
    dir: &Path,
    agent: Option<&str>,
    branch: &str,
    dangerous: bool,
) -> Result<(), String> {
    let mut target = dir.to_path_buf();

    // Resolve / create the worktree via wt — wt stays the worktree owner.
    if !branch.is_empty() {
        if !in_git_repo(dir) {
            return Err("not a git repository (branch given)".into());
        }
        let d = dir.to_str().unwrap_or(".");
        if wt_path_for(dir, branch).is_none() {
            let refname = format!("refs/heads/{branch}");
            let exists =
                out(&["git", "-C", d, "rev-parse", "--verify", "--quiet", &refname]).is_some();
            let ok = if exists {
                out(&["wt", "-C", d, "switch", "--no-cd", branch]).is_some()
            } else {
                out(&["wt", "-C", d, "switch", "--no-cd", "-c", branch]).is_some()
            };
            if !ok {
                return Err(format!("wt switch failed for '{branch}'"));
            }
        }
        target = wt_path_for(dir, branch)
            .ok_or_else(|| format!("could not resolve worktree path for '{branch}'"))?;
    }
    let target_str = target.to_string_lossy().into_owned();

    // Label: project/branch when known, else project, else basename.
    let mut project = String::new();
    let mut br = String::new();
    if in_git_repo(&target) {
        if let Some(porcelain) =
            out(&["git", "-C", &target_str, "worktree", "list", "--porcelain"])
        {
            if let Some(root) = porcelain.lines().next().and_then(|l| l.strip_prefix("worktree "))
            {
                project = Path::new(root.trim())
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
            }
        }
        br = if branch.is_empty() {
            out(&["git", "-C", &target_str, "symbolic-ref", "--short", "HEAD"])
                .map(|s| s.trim().to_string())
                .unwrap_or_default()
        } else {
            branch.to_string()
        };
    }
    let basename = target
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| target_str.clone());
    let label = match (project.is_empty(), br.is_empty()) {
        (false, false) => format!("{project}/{br}"),
        (false, true) => project,
        (true, _) => basename,
    };

    // A cockpit for this label already exists: focus it instead of rebuilding.
    if let Some(existing) = ws_id_for_label(&label) {
        focus_workspace(&existing);
        return Ok(());
    }

    // Env via `workspace create --env` (a `pane run` prefix would be echoed
    // onto the pane). Claude runs skip-permissions by default, like nic.
    // ponytail: claude riding inside nvim (via the claudecode herdr provider
    // reading NIC_AI) is the one setup-specific assumption left — make it a
    // config knob if anyone else wants a different in-editor agent
    let mut create: Vec<&str> = vec![
        "herdr", "workspace", "create", "--cwd", &target_str, "--label", &label, "--no-focus",
    ];
    if agent == Some("claude") {
        create.extend([
            "--env",
            "NIC_AI=claude",
            "--env",
            "NIC_CLAUDE_ARGS=--dangerously-skip-permissions",
        ]);
    }
    let created = json(&create).ok_or("herdr workspace create failed")?;
    let root_pane = &created["result"]["root_pane"];
    let ws = root_pane["workspace_id"].as_str().ok_or("no workspace_id in create result")?;
    let root = root_pane["pane_id"].as_str().ok_or("no pane_id in create result")?;
    let root_tab = root_pane["tab_id"].as_str().unwrap_or("");

    out(&["herdr", "tab", "rename", root_tab, "nic"]);

    // Full-width terminal on the bottom row.
    out(&[
        "herdr", "pane", "split", root, "--direction", "down", "--ratio", "0.8", "--cwd",
        &target_str, "--no-focus",
    ]);

    match agent {
        // Claude is spawned by nvim's claudecode herdr provider (env above);
        // None gets the same layout minus the env, so nvim stays plain.
        Some("claude") | None => {
            out(&["herdr", "pane", "run", root, "nvim"]);
        }
        // Every other agent gets its own pane on the top-right. Dangerous is
        // agent-specific (AGENTS table): append a flag, or prefix an env.
        Some(a) => {
            let cmd = agent_command(a, dangerous);
            let split = json(&[
                "herdr", "pane", "split", root, "--direction", "right", "--ratio", "0.7",
                "--cwd", &target_str, "--no-focus",
            ])
            .ok_or("pane split for agent failed")?;
            let agent_pane =
                split["result"]["pane"]["pane_id"].as_str().ok_or("no pane_id in split result")?;
            out(&["herdr", "pane", "run", root, "nvim"]);
            out(&["herdr", "pane", "run", agent_pane, &cmd]);
        }
    }

    // Unfocused lazygit tab — one keystroke away, out of the way.
    if let Some(tab) = json(&[
        "herdr", "tab", "create", "--workspace", ws, "--cwd", &target_str, "--label", "lazygit",
        "--no-focus",
    ]) {
        if let Some(git_pane) = tab["result"]["root_pane"]["pane_id"].as_str() {
            out(&["herdr", "pane", "run", git_pane, "lazygit"]);
        }
    }

    focus_workspace(ws);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dangerous_command_and_toggle() {
        // Flag agents append; cursor uses its cursor-agent binary.
        assert_eq!(agent_command("codex", true), "codex --dangerously-bypass-approvals-and-sandbox");
        assert_eq!(agent_command("codex", false), "codex");
        assert_eq!(agent_command("cursor", true), "cursor-agent --force");
        // opencode has no flag: prefix the env instead.
        assert_eq!(agent_command("opencode", true), "OPENCODE_PERMISSION='{\"*\":\"allow\"}' opencode");
        // Always-yolo / unknown agents: no-op, and toggle is greyed.
        assert_eq!(agent_command("pi", true), "pi");
        assert!(!dangerous_toggleable("pi"));
        assert!(!dangerous_toggleable("claude"));
        assert!(dangerous_toggleable("codex"));
        assert!(dangerous_toggleable("opencode"));
    }
}
