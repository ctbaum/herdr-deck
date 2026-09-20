//! Deck editor launch, post-restart recovery, and link dispatch.
//!
//! Neovim runs directly in the editor pane with `--listen` so the plugin can
//! dispatch clicked file links to it. After a Herdr server restart the panes
//! come back as bare shells; the startup hook simply starts a fresh editor in
//! each recorded deck pane. No editor state survives a restart on purpose:
//! swapfiles and session plugins are Neovim's job. Editor-managed agents use
//! native session references to reconnect to the replacement editor.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const RECORD_VERSION: u32 = 2;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct EditorRecord {
    version: u32,
    herdr_socket: String,
    workspace_id: String,
    editor_pane_id: String,
    cwd: PathBuf,
    nvim_socket: PathBuf,
    agent: Option<String>,
    #[serde(default)]
    launch_args: Vec<String>,
}

fn safe_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

fn session_key(socket: &str) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in socket.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn herdr_socket() -> Result<String, String> {
    env::var("HERDR_SOCKET_PATH")
        .map_err(|_| "HERDR_SOCKET_PATH is required for the deck editor".into())
}

fn herdr_bin() -> String {
    env::var("HERDR_BIN_PATH")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "herdr".into())
}

fn nvim_bin() -> String {
    env::var("HERDR_DECK_NVIM_BIN")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "nvim".into())
}

fn runtime_dir() -> PathBuf {
    env::var_os("HERDR_DECK_RUNTIME_DIR")
        .map(PathBuf::from)
        .or_else(|| env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from))
        .unwrap_or_else(|| {
            let user = env::var("USER").unwrap_or_else(|_| "user".into());
            env::temp_dir().join(format!("herdr-deck-{}", safe_component(&user)))
        })
        .join("herdr-deck")
}

fn state_dir() -> PathBuf {
    env::var_os("HERDR_PLUGIN_STATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| runtime_dir().join("state"))
        .join("editors")
}

fn protect_dir(dir: &Path) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|error| {
        format!(
            "could not create editor directory {}: {error}",
            dir.display()
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(|error| {
            format!(
                "could not protect editor directory {}: {error}",
                dir.display()
            )
        })?;
    }
    Ok(())
}

fn socket_path_for(socket: &str, workspace: &str) -> PathBuf {
    runtime_dir().join(format!(
        "{}-{}.sock",
        session_key(socket),
        safe_component(workspace)
    ))
}

fn record_path(record: &EditorRecord) -> PathBuf {
    state_dir().join(format!(
        "{}-{}.json",
        session_key(&record.herdr_socket),
        safe_component(&record.workspace_id)
    ))
}

fn write_record(record: &EditorRecord) -> Result<(), String> {
    let dir = state_dir();
    protect_dir(&dir)?;
    let path = record_path(record);
    let temporary = path.with_extension(format!("json.tmp-{}", std::process::id()));
    let bytes = serde_json::to_vec_pretty(record)
        .map_err(|error| format!("could not encode editor state: {error}"))?;
    fs::write(&temporary, bytes)
        .map_err(|error| format!("could not write editor state: {error}"))?;
    fs::rename(&temporary, &path)
        .map_err(|error| format!("could not publish editor state: {error}"))?;
    Ok(())
}

fn read_records() -> Vec<EditorRecord> {
    let Ok(entries) = fs::read_dir(state_dir()) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("json"))
        .filter_map(|path| fs::read(path).ok())
        .filter_map(|bytes| serde_json::from_slice::<EditorRecord>(&bytes).ok())
        .filter(|record| record.version == RECORD_VERSION)
        .collect()
}

fn remove_record(record: &EditorRecord) {
    let _ = fs::remove_file(record_path(record));
}

fn nvim_remote_expr(socket: &Path, expression: &str) -> Option<String> {
    let output = Command::new(nvim_bin())
        .arg("--server")
        .arg(socket)
        .arg("--remote-expr")
        .arg(expression)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn nvim_responding(socket: &Path) -> bool {
    nvim_remote_expr(socket, "1+1").as_deref() == Some("2")
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn nvim_listen_command(socket: &Path, restore_wait: bool) -> String {
    let socket = shell_quote(&socket.to_string_lossy());
    let wait = if restore_wait {
        "HERDR_NVIM_AGENT_RECOVER_WAIT_MS='5000' "
    } else {
        ""
    };
    // ponytail: NVIM_LISTEN_ADDRESS is deprecated but is the only shell-native
    // way for a later plain `nvim` to reuse this socket; use a wrapper if removed.
    format!(
        "export NVIM_LISTEN_ADDRESS={socket}; {wait}{} --listen {socket}",
        shell_quote(&nvim_bin())
    )
}

fn remove_stale_socket(socket: &Path) -> Result<(), String> {
    if socket.exists() {
        fs::remove_file(socket)
            .map_err(|error| format!("could not remove stale editor socket: {error}"))?;
    }
    Ok(())
}

/// Record the deck editor for `workspace` and return the pane command that
/// starts it. The agent env travels on the workspace, not the command line.
pub fn prepare_editor(
    workspace: &str,
    pane: &str,
    cwd: &Path,
    agent: Option<&str>,
    launch_args: &[String],
) -> Result<String, String> {
    let socket = herdr_socket()?;
    let record = EditorRecord {
        version: RECORD_VERSION,
        herdr_socket: socket.clone(),
        workspace_id: workspace.into(),
        editor_pane_id: pane.into(),
        cwd: cwd.into(),
        nvim_socket: socket_path_for(&socket, workspace),
        agent: agent
            .filter(|agent| matches!(*agent, "claude" | "codex" | "pi"))
            .map(String::from),
        launch_args: launch_args.to_vec(),
    };
    protect_dir(&runtime_dir())?;
    remove_stale_socket(&record.nvim_socket)?;
    write_record(&record)?;
    Ok(nvim_listen_command(&record.nvim_socket, false))
}

fn herdr_json(args: &[&str]) -> Option<Value> {
    let output = Command::new(herdr_bin()).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice(&output.stdout).ok()
}

fn pane_workspace(pane: &str) -> Option<String> {
    let value = herdr_json(&["pane", "get", pane])?;
    value
        .pointer("/result/pane/workspace_id")
        .and_then(Value::as_str)
        .map(String::from)
}

/// Pane command that restarts the editor after a server restart. Workspace env
/// from `workspace create --env` is not assumed to survive the restart, so the
/// agent contract rides along as a command-line prefix. The recovery window
/// lets Herdr restore native agent sessions before Neovim inspects the tab.
fn restore_command(record: &EditorRecord) -> Result<String, String> {
    let Some(agent) = record.agent.as_deref() else {
        return Ok(nvim_listen_command(&record.nvim_socket, false));
    };
    let args = serde_json::to_string(&record.launch_args)
        .map_err(|error| format!("could not encode editor-agent arguments: {error}"))?;
    Ok(format!(
        "export HERDR_NVIM_AGENT={} HERDR_NVIM_AGENT_ARGS_JSON={} HERDR_NVIM_AGENT_RECOVER='1'; {}",
        shell_quote(agent),
        shell_quote(&args),
        nvim_listen_command(&record.nvim_socket, true)
    ))
}

/// Start a fresh editor in every recorded deck pane whose Neovim is gone.
/// Records for vanished or repurposed panes are dropped.
pub fn restore_editors() -> Result<(), String> {
    let socket = herdr_socket()?;
    let mut errors = Vec::new();
    for record in read_records()
        .into_iter()
        .filter(|record| record.herdr_socket == socket)
    {
        // Pane ids can be reused after close: only touch a pane that still
        // belongs to the recorded workspace.
        if pane_workspace(&record.editor_pane_id).as_deref() != Some(&record.workspace_id) {
            remove_record(&record);
            continue;
        }
        if nvim_responding(&record.nvim_socket) {
            continue;
        }
        let run = remove_stale_socket(&record.nvim_socket)
            .and_then(|()| restore_command(&record))
            .and_then(|command| {
                let status = Command::new(herdr_bin())
                    .args(["pane", "run", &record.editor_pane_id, &command])
                    .status()
                    .map_err(|error| format!("could not restart deck editor: {error}"))?;
                status.success().then_some(()).ok_or_else(|| {
                    format!(
                        "Herdr rejected the editor restart for {}",
                        record.editor_pane_id
                    )
                })
            });
        if let Err(error) = run {
            errors.push(error);
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

pub fn stop_workspace(workspace: &str) {
    let socket = env::var("HERDR_SOCKET_PATH").unwrap_or_default();
    for record in read_records().into_iter().filter(|record| {
        record.workspace_id == workspace && (socket.is_empty() || record.herdr_socket == socket)
    }) {
        remove_record(&record);
    }
}

fn percent_decode(value: &str) -> String {
    fn hex_value(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }

    let bytes = value.as_bytes();
    let mut result = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
        {
            result.push((high << 4) | low);
            index += 3;
            continue;
        }
        result.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&result).into_owned()
}

fn file_url_path(value: &str) -> Option<String> {
    let rest = value.strip_prefix("file://")?;
    let path = if rest.starts_with('/') {
        rest
    } else {
        &rest[rest.find('/')?..]
    };
    Some(percent_decode(path))
}

pub(crate) fn parse_clicked(value: &str) -> Option<(String, Option<u32>)> {
    let decoded = file_url_path(value).unwrap_or_else(|| value.to_string());
    let mut path = decoded.trim_end_matches(['.', ',']);
    let mut numbers = Vec::new();
    for _ in 0..2 {
        let Some((before, after)) = path.rsplit_once(':') else {
            break;
        };
        let Ok(number) = after.parse::<u32>() else {
            break;
        };
        numbers.push(number);
        path = before;
    }
    if path.is_empty() {
        return None;
    }
    Some((path.to_string(), numbers.last().copied()))
}

fn expand_tilde(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => PathBuf::from(env::var_os("HOME").unwrap_or_default()).join(rest),
        None => PathBuf::from(path),
    }
}

fn resolve_clicked(path: &str, cwd: &Path) -> Option<PathBuf> {
    let expanded = expand_tilde(path);
    let direct = if expanded.is_absolute() {
        expanded
    } else {
        cwd.join(expanded)
    };
    if direct.is_file() {
        return Some(direct);
    }
    if Path::new(path).is_absolute() || path.starts_with('~') {
        return None;
    }
    let output = Command::new("git")
        .args(["-C", &cwd.to_string_lossy(), "rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let root = String::from_utf8_lossy(&output.stdout);
    let candidate = Path::new(root.trim()).join(path);
    candidate.is_file().then_some(candidate)
}

fn pane_cwd(pane: &str) -> Option<PathBuf> {
    let value = herdr_json(&["pane", "get", pane])?;
    value
        .pointer("/result/pane/foreground_cwd")
        .or_else(|| value.pointer("/result/pane/cwd"))
        .and_then(Value::as_str)
        .map(PathBuf::from)
}

fn open_in_nvim(socket: &Path, path: &Path, line: Option<u32>) -> Result<(), String> {
    let status = Command::new(nvim_bin())
        .arg("--server")
        .arg(socket)
        .arg("--remote")
        .arg(path)
        .stdout(Stdio::null())
        .status()
        .map_err(|error| format!("could not contact deck editor: {error}"))?;
    if !status.success() {
        return Err("deck editor rejected the file-open request".into());
    }
    if let Some(line) = line {
        let status = Command::new(nvim_bin())
            .arg("--server")
            .arg(socket)
            .arg("--remote-expr")
            .arg(format!("cursor({line}, 1)"))
            .stdout(Stdio::null())
            .status()
            .map_err(|error| format!("could not move the deck editor cursor: {error}"))?;
        if !status.success() {
            return Err(format!("deck editor could not move to line {line}"));
        }
    }
    Ok(())
}

pub fn open_clicked_link() -> Result<(), String> {
    let Some(clicked) = env::var("HERDR_PLUGIN_CLICKED_URL").ok() else {
        return Ok(());
    };
    let Some(pane) = env::var("HERDR_PANE_ID").ok() else {
        return Ok(());
    };
    let Some(workspace) = env::var("HERDR_WORKSPACE_ID").ok() else {
        return Ok(());
    };
    let Some((path, line)) = parse_clicked(&clicked) else {
        return Err(format!("could not parse clicked file path: {clicked}"));
    };
    let Some(cwd) = pane_cwd(&pane) else {
        return Err(format!(
            "could not determine working directory for pane {pane}"
        ));
    };
    let Some(path) = resolve_clicked(&path, &cwd) else {
        return Err(format!("clicked file does not exist: {path}"));
    };
    let socket = herdr_socket()?;
    let Some(record) = read_records()
        .into_iter()
        .find(|record| record.workspace_id == workspace && record.herdr_socket == socket)
    else {
        return Err(format!(
            "no deck editor is recorded for workspace {workspace}"
        ));
    };
    if !nvim_responding(&record.nvim_socket) {
        return Err(format!(
            "deck editor for workspace {workspace} is not running"
        ));
    }
    open_in_nvim(&record.nvim_socket, &path, line)?;

    if record.editor_pane_id != pane {
        let status = Command::new(herdr_bin())
            .args(["pane", "focus", &record.editor_pane_id])
            .status()
            .map_err(|error| format!("could not focus deck editor pane: {error}"))?;
        if !status.success() {
            return Err(format!(
                "Herdr could not focus deck editor pane {}",
                record.editor_pane_id
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_paths_and_file_urls() {
        assert_eq!(
            parse_clicked("src/main.rs:42:7"),
            Some(("src/main.rs".into(), Some(42)))
        );
        assert_eq!(
            parse_clicked("file:///tmp/a%20file.rs:9"),
            Some(("/tmp/a file.rs".into(), Some(9)))
        );
        assert_eq!(
            parse_clicked("file:///tmp/%aé.rs"),
            Some(("/tmp/%aé.rs".into(), None))
        );
    }

    #[test]
    fn listener_paths_are_stable_session_scoped_and_sanitized() {
        let first = socket_path_for("/tmp/session-a/herdr.sock", "workspace:1");
        let second = socket_path_for("/tmp/session-a/herdr.sock", "workspace:1");
        let other_workspace = socket_path_for("/tmp/session-a/herdr.sock", "workspace:2");
        let other = socket_path_for("/tmp/session-b/herdr.sock", "workspace:1");
        assert_eq!(first, second);
        assert_ne!(first, other_workspace);
        assert_ne!(first, other);
        assert!(
            first
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with("-workspace-1.sock"))
        );
    }

    #[test]
    fn editor_commands_quote_binary_socket_and_agent_contract() {
        unsafe { env::set_var("HERDR_DECK_NVIM_BIN", "/tmp/my nvim") };
        assert_eq!(
            nvim_listen_command(Path::new("/tmp/a socket.sock"), false),
            "export NVIM_LISTEN_ADDRESS='/tmp/a socket.sock'; '/tmp/my nvim' --listen '/tmp/a socket.sock'"
        );
        let record = EditorRecord {
            version: RECORD_VERSION,
            herdr_socket: "/tmp/herdr.sock".into(),
            workspace_id: "w1".into(),
            editor_pane_id: "w1:p1".into(),
            cwd: "/repo".into(),
            nvim_socket: "/tmp/w1.sock".into(),
            agent: Some("claude".into()),
            launch_args: vec!["--resume".into(), "it's".into()],
        };
        assert_eq!(
            restore_command(&record).unwrap(),
            "export HERDR_NVIM_AGENT='claude' HERDR_NVIM_AGENT_ARGS_JSON='[\"--resume\",\"it'\"'\"'s\"]' HERDR_NVIM_AGENT_RECOVER='1'; export NVIM_LISTEN_ADDRESS='/tmp/w1.sock'; HERDR_NVIM_AGENT_RECOVER_WAIT_MS='5000' '/tmp/my nvim' --listen '/tmp/w1.sock'"
        );
        let pi_record = EditorRecord {
            agent: Some("pi".into()),
            launch_args: vec!["--session".into(), "/tmp/session with spaces.jsonl".into()],
            ..record
        };
        assert_eq!(
            restore_command(&pi_record).unwrap(),
            "export HERDR_NVIM_AGENT='pi' HERDR_NVIM_AGENT_ARGS_JSON='[\"--session\",\"/tmp/session with spaces.jsonl\"]' HERDR_NVIM_AGENT_RECOVER='1'; export NVIM_LISTEN_ADDRESS='/tmp/w1.sock'; HERDR_NVIM_AGENT_RECOVER_WAIT_MS='5000' '/tmp/my nvim' --listen '/tmp/w1.sock'"
        );

        unsafe { env::set_var("HERDR_DECK_NVIM_BIN", "/bin/echo") };
        let probe = format!(
            "unset HERDR_NVIM_AGENT_RECOVER_WAIT_MS; {}; printf '\\nwait=<%s> agent=<%s> recover=<%s> socket=<%s>\\n' \"${{HERDR_NVIM_AGENT_RECOVER_WAIT_MS-}}\" \"$HERDR_NVIM_AGENT\" \"$HERDR_NVIM_AGENT_RECOVER\" \"$NVIM_LISTEN_ADDRESS\"",
            restore_command(&pi_record).unwrap()
        );
        let output = Command::new("sh").args(["-c", &probe]).output().unwrap();
        assert!(output.status.success());
        let output = String::from_utf8(output.stdout).unwrap();
        assert!(output.contains("wait=<>"));
        assert!(output.contains("agent=<pi>"));
        assert!(output.contains("recover=<1>"));
        assert!(output.contains("socket=</tmp/w1.sock>"));
        unsafe { env::remove_var("HERDR_DECK_NVIM_BIN") };
    }
}
