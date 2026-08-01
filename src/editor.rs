//! Persistent Neovim daemons, Herdr restart restoration, and link dispatch.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const RECORD_VERSION: u32 = 1;
const HEALTH_TIMEOUT: Duration = Duration::from_secs(10);
const HEALTH_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct EditorRecord {
    version: u32,
    herdr_socket: String,
    workspace_id: String,
    tab_id: String,
    editor_pane_id: String,
    cwd: PathBuf,
    nvim_socket: PathBuf,
    #[serde(default)]
    server_token: Option<String>,
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
        .map_err(|_| "HERDR_SOCKET_PATH is required for a persistent deck editor".into())
}

#[cfg(unix)]
fn server_token(socket: &str) -> Option<String> {
    use std::os::unix::fs::MetadataExt;

    let metadata = fs::metadata(socket).ok()?;
    Some(format!("{}:{}", metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn server_token(socket: &str) -> Option<String> {
    let modified = fs::metadata(socket).ok()?.modified().ok()?;
    Some(format!("{modified:?}"))
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

fn daemon_healthy(socket: &Path) -> bool {
    nvim_remote_expr(socket, "1+1").as_deref() == Some("2")
}

fn spawn_daemon(record: &EditorRecord) -> Result<(), String> {
    protect_dir(&runtime_dir())?;
    if record.nvim_socket.exists() {
        fs::remove_file(&record.nvim_socket)
            .map_err(|error| format!("could not remove stale editor socket: {error}"))?;
    }

    #[cfg(unix)]
    use std::os::unix::process::CommandExt;

    let mut command = Command::new(nvim_bin());
    command
        .arg("--headless")
        .arg("--listen")
        .arg(&record.nvim_socket)
        .current_dir(&record.cwd)
        .env("HERDR_ENV", "1")
        .env("HERDR_SOCKET_PATH", &record.herdr_socket)
        .env("HERDR_WORKSPACE_ID", &record.workspace_id)
        .env("HERDR_TAB_ID", &record.tab_id)
        .env("HERDR_PANE_ID", &record.editor_pane_id)
        .env("HERDR_BIN_PATH", herdr_bin())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(agent) = record.agent.as_deref() {
        let args = serde_json::to_string(&record.launch_args)
            .map_err(|error| format!("could not encode editor-agent arguments: {error}"))?;
        command
            .env("HERDR_NVIM_AGENT", agent)
            .env("HERDR_NVIM_AGENT_ARGS_JSON", args);
    } else {
        command
            .env_remove("HERDR_NVIM_AGENT")
            .env_remove("HERDR_NVIM_AGENT_ARGS_JSON");
    }

    #[cfg(unix)]
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    command
        .spawn()
        .map_err(|error| format!("could not start persistent Neovim: {error}"))?;
    let deadline = Instant::now() + HEALTH_TIMEOUT;
    while Instant::now() < deadline {
        if daemon_healthy(&record.nvim_socket) {
            return Ok(());
        }
        thread::sleep(HEALTH_INTERVAL);
    }
    Err(format!(
        "persistent Neovim did not listen on {} within {} seconds",
        record.nvim_socket.display(),
        HEALTH_TIMEOUT.as_secs()
    ))
}

fn ensure_daemon(record: &EditorRecord) -> Result<bool, String> {
    if daemon_healthy(&record.nvim_socket) {
        return Ok(true);
    }
    spawn_daemon(record)?;
    Ok(false)
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn remote_ui_command(socket: &Path) -> String {
    format!(
        "exec {} --server {} --remote-ui",
        shell_quote(&nvim_bin()),
        shell_quote(&socket.to_string_lossy())
    )
}

pub fn prepare_editor(
    workspace: &str,
    tab: &str,
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
        tab_id: tab.into(),
        editor_pane_id: pane.into(),
        cwd: cwd.into(),
        nvim_socket: socket_path_for(&socket, workspace),
        server_token: server_token(&socket),
        agent: agent
            .filter(|agent| matches!(*agent, "claude" | "codex"))
            .map(String::from),
        launch_args: launch_args.to_vec(),
    };
    let survived = ensure_daemon(&record)?;
    if let Err(error) = write_record(&record) {
        if !survived {
            stop_daemon(&record);
        }
        return Err(error);
    }
    Ok(remote_ui_command(&record.nvim_socket))
}

fn herdr_json(args: &[&str]) -> Option<Value> {
    let output = Command::new(herdr_bin()).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice(&output.stdout).ok()
}

fn pane_exists(pane: &str) -> bool {
    herdr_json(&["pane", "get", pane]).is_some()
}

fn foreground_pane() -> Option<String> {
    let value = herdr_json(&["pane", "current", "--current"])?;
    value
        .pointer("/result/pane/pane_id")
        .and_then(Value::as_str)
        .map(String::from)
}

fn pane_hosts_remote_ui(record: &EditorRecord) -> bool {
    let Some(value) = herdr_json(&["pane", "process-info", "--pane", &record.editor_pane_id])
    else {
        return false;
    };
    let socket = record.nvim_socket.to_string_lossy();
    value
        .pointer("/result/process_info/foreground_processes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|process| {
            process
                .get("argv")
                .and_then(Value::as_array)
                .is_some_and(|argv| {
                    argv.iter()
                        .filter_map(Value::as_str)
                        .any(|arg| arg == "--remote-ui")
                        && argv
                            .iter()
                            .filter_map(Value::as_str)
                            .any(|arg| arg == socket)
                })
        })
}

fn vim_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn lua_with_json(expression: &str, payload: &Value) -> String {
    let json = serde_json::to_string(payload).unwrap_or_else(|_| "{}".into());
    format!(
        "luaeval({}, json_decode({}))",
        vim_single_quote(expression),
        vim_single_quote(&json)
    )
}

fn refresh_daemon_env(record: &EditorRecord) -> bool {
    let payload = serde_json::json!({
        "HERDR_ENV": "1",
        "HERDR_SOCKET_PATH": record.herdr_socket,
        "HERDR_WORKSPACE_ID": record.workspace_id,
        "HERDR_TAB_ID": record.tab_id,
        "HERDR_PANE_ID": record.editor_pane_id,
        "HERDR_BIN_PATH": herdr_bin(),
    });
    let expression =
        "(function() for key, value in pairs(_A) do vim.env[key] = value end return 1 end)()";
    nvim_remote_expr(&record.nvim_socket, &lua_with_json(expression, &payload)).as_deref()
        == Some("1")
}

fn provider_pane(record: &EditorRecord, agent: &str) -> Option<String> {
    let payload = serde_json::json!({ "agent": agent });
    let expression = "(function() local ok, mod = pcall(require, 'herdr-agents'); if not ok or type(mod.pane) ~= 'function' then return '' end return mod.pane(_A.agent) or '' end)()";
    nvim_remote_expr(&record.nvim_socket, &lua_with_json(expression, &payload))
        .filter(|value| !value.is_empty())
}

fn agent_session(pane: &str, agent: &str) -> Option<String> {
    let value = herdr_json(&["pane", "get", pane])?;
    let session = value.pointer("/result/pane/agent_session")?;
    (session.get("agent").and_then(Value::as_str) == Some(agent)
        && session.get("kind").and_then(Value::as_str) == Some("id"))
    .then(|| {
        session
            .get("value")
            .and_then(Value::as_str)
            .map(String::from)
    })
    .flatten()
}

fn resume_args(agent: &str, original: &[String], session: Option<&str>) -> Vec<String> {
    let Some(session) = session else {
        return original.to_vec();
    };
    match agent {
        "claude" => {
            let mut kept = Vec::new();
            let mut index = 0;
            while index < original.len() {
                if original[index] == "--resume" {
                    index += 2;
                } else {
                    kept.push(original[index].clone());
                    index += 1;
                }
            }
            let mut args = vec!["--resume".into(), session.into()];
            args.extend(kept);
            args
        }
        "codex" => {
            let kept = if original.first().map(String::as_str) == Some("resume") {
                original.iter().skip(2).cloned().collect::<Vec<_>>()
            } else {
                original.to_vec()
            };
            let mut args = vec!["resume".into(), session.into()];
            args.extend(kept);
            args
        }
        _ => original.to_vec(),
    }
}

fn reconnect_agent(record: &EditorRecord) -> Result<(), String> {
    let Some(agent) = record.agent.as_deref() else {
        return Ok(());
    };
    let old_pane = provider_pane(record, agent);
    let session = old_pane
        .as_deref()
        .and_then(|pane| agent_session(pane, agent));
    let args = resume_args(agent, &record.launch_args, session.as_deref());
    let payload = serde_json::json!({ "agent": agent, "args": args });
    let expression = "(function() local ok, mod = pcall(require, 'herdr-agents'); if not ok or type(mod.reconnect) ~= 'function' then return 0 end return mod.reconnect(_A.agent, _A.args) and 1 or 0 end)()";
    let result = nvim_remote_expr(&record.nvim_socket, &lua_with_json(expression, &payload));
    if result.as_deref() != Some("1") {
        return Err(format!(
            "could not reconnect {agent} through the deck editor"
        ));
    }
    if session.is_none() {
        eprintln!(
            "herdr-deck: {agent} had no native session reference; started a new conversation"
        );
    }
    Ok(())
}

fn attach_remote_ui(record: &EditorRecord) -> Result<(), String> {
    let command = remote_ui_command(&record.nvim_socket);
    let status = Command::new(herdr_bin())
        .args(["pane", "run", &record.editor_pane_id, &command])
        .status()
        .map_err(|error| format!("could not attach restored editor pane: {error}"))?;
    if !status.success() {
        return Err(format!(
            "Herdr rejected the editor restore for {}",
            record.editor_pane_id
        ));
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if pane_hosts_remote_ui(record) {
            return Ok(());
        }
        thread::sleep(HEALTH_INTERVAL);
    }
    Err(format!(
        "editor pane {} did not attach to its Neovim daemon",
        record.editor_pane_id
    ))
}

pub fn restore_editors() -> Result<(), String> {
    let socket = herdr_socket()?;
    let current_server_token = server_token(&socket);
    let mut errors = Vec::new();
    for mut record in read_records()
        .into_iter()
        .filter(|record| record.herdr_socket == socket)
    {
        if !pane_exists(&record.editor_pane_id) {
            stop_daemon(&record);
            remove_record(&record);
            continue;
        }
        let server_restarted = current_server_token.is_some()
            && record.server_token.as_ref() != current_server_token.as_ref();
        if pane_hosts_remote_ui(&record) {
            let _ = refresh_daemon_env(&record);
            if server_restarted && let Err(error) = reconnect_agent(&record) {
                errors.push(error);
            }
            record.server_token = current_server_token.clone();
            if let Err(error) = write_record(&record) {
                errors.push(error);
            }
            continue;
        }
        let focused = foreground_pane();
        let survived = match ensure_daemon(&record) {
            Ok(survived) => survived,
            Err(error) => {
                errors.push(error);
                continue;
            }
        };
        if !refresh_daemon_env(&record) {
            errors.push(format!(
                "could not refresh Herdr identity in editor {}",
                record.editor_pane_id
            ));
            continue;
        }
        if let Err(error) = attach_remote_ui(&record) {
            errors.push(error);
            continue;
        }
        if survived {
            if let Err(error) = reconnect_agent(&record) {
                errors.push(error);
            }
        } else {
            eprintln!(
                "herdr-deck: Neovim state for {} was unavailable; started a fresh editor",
                record.workspace_id
            );
        }
        record.server_token = current_server_token.clone();
        if let Err(error) = write_record(&record) {
            errors.push(error);
        }
        if let Some(pane) = focused {
            let _ = Command::new(herdr_bin())
                .args(["pane", "focus", &pane])
                .status();
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

fn stop_daemon(record: &EditorRecord) {
    if daemon_healthy(&record.nvim_socket) {
        let _ = Command::new(nvim_bin())
            .arg("--server")
            .arg(&record.nvim_socket)
            .arg("--remote-send")
            .arg("<Cmd>qa!<CR>")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        thread::sleep(Duration::from_millis(100));
    }
    let _ = fs::remove_file(&record.nvim_socket);
}

pub fn stop_workspace(workspace: &str) {
    let socket = env::var("HERDR_SOCKET_PATH").unwrap_or_default();
    for record in read_records().into_iter().filter(|record| {
        record.workspace_id == workspace && (socket.is_empty() || record.herdr_socket == socket)
    }) {
        stop_daemon(&record);
        remove_record(&record);
    }
}

pub fn cleanup_event() -> Result<(), String> {
    #[derive(Clone, Copy)]
    enum Target<'a> {
        Workspace(&'a str),
        Pane(&'a str),
    }

    let socket = herdr_socket()?;
    let event_kind =
        env::var("HERDR_PLUGIN_EVENT").map_err(|_| "HERDR_PLUGIN_EVENT is missing".to_string())?;
    let event = env::var("HERDR_PLUGIN_EVENT_JSON")
        .map_err(|_| "HERDR_PLUGIN_EVENT_JSON is missing".to_string())?;
    let event: Value = serde_json::from_str(&event)
        .map_err(|error| format!("could not decode Herdr cleanup event: {error}"))?;
    let target = match event_kind.as_str() {
        "workspace.closed" => Target::Workspace(
            event
                .pointer("/data/workspace_id")
                .and_then(Value::as_str)
                .ok_or_else(|| "workspace.closed event is missing data.workspace_id".to_string())?,
        ),
        "pane.closed" => Target::Pane(
            event
                .pointer("/data/pane_id")
                .and_then(Value::as_str)
                .ok_or_else(|| "pane.closed event is missing data.pane_id".to_string())?,
        ),
        _ => return Err(format!("unexpected editor cleanup event: {event_kind}")),
    };
    for record in read_records()
        .into_iter()
        .filter(|record| record.herdr_socket == socket)
    {
        let matches = match target {
            Target::Workspace(workspace) => workspace == record.workspace_id,
            Target::Pane(pane) => pane == record.editor_pane_id,
        };
        if matches {
            stop_daemon(&record);
            remove_record(&record);
        }
    }
    Ok(())
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
    if !daemon_healthy(&record.nvim_socket) {
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
        let other = socket_path_for("/tmp/session-b/herdr.sock", "workspace:1");
        assert_eq!(first, second);
        assert_ne!(first, other);
        assert!(
            first
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with("-workspace-1.sock"))
        );
    }

    #[test]
    fn remote_ui_command_quotes_binary_and_socket() {
        unsafe { env::set_var("HERDR_DECK_NVIM_BIN", "/tmp/my nvim") };
        assert_eq!(
            remote_ui_command(Path::new("/tmp/a socket.sock")),
            "exec '/tmp/my nvim' --server '/tmp/a socket.sock' --remote-ui"
        );
        unsafe { env::remove_var("HERDR_DECK_NVIM_BIN") };
    }

    #[test]
    fn resume_arguments_replace_stale_session_selectors() {
        assert_eq!(
            resume_args(
                "claude",
                &["--resume".into(), "old".into(), "--danger".into()],
                Some("new")
            ),
            vec!["--resume", "new", "--danger"]
        );
        assert_eq!(
            resume_args(
                "codex",
                &["resume".into(), "old".into(), "--danger".into()],
                Some("new")
            ),
            vec!["resume", "new", "--danger"]
        );
    }

    #[test]
    fn lua_payload_uses_json_without_shell_interpolation() {
        let expression = lua_with_json("_A.value", &serde_json::json!({ "value": "it's $safe" }));
        assert!(expression.contains("it''s $safe"));
    }
}
