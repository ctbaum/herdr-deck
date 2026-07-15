//! Local agent-session discovery and resume handoff. Session history is loaded
//! only while the Sessions source is active, so the normal deck search stays
//! small and fast.

use serde_json::Value;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Agent {
    Claude,
    Codex,
    Cursor,
    Pi,
}

impl Agent {
    pub fn id(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Cursor => "cursor",
            Self::Pi => "pi",
        }
    }
}

#[derive(Clone)]
pub struct Session {
    pub agent: Agent,
    pub id: String,
    pub cwd: PathBuf,
    pub title: String,
    pub updated: u64,
    pub file: Option<PathBuf>,
    /// Cursor exposes its saved chats through an interactive CLI picker, not a
    /// local enumerable store. This entry hands off to that native picker.
    pub native_picker: bool,
}

fn command_exists(bin: &str) -> bool {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path).any(|dir| dir.join(bin).is_file())
}

fn files(root: &Path, max_depth: usize) -> Vec<PathBuf> {
    fn visit(dir: &Path, depth: usize, max_depth: usize, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() && depth < max_depth {
                visit(&p, depth + 1, max_depth, out);
            } else if p.extension().is_some_and(|e| e == "jsonl") {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    visit(root, 0, max_depth, &mut out);
    out
}

fn modified(path: &Path) -> u64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn text_content(v: &Value) -> Option<String> {
    if let Some(s) = v.as_str() {
        return Some(s.to_string());
    }
    let parts: Vec<&str> = v
        .as_array()?
        .iter()
        .filter(|part| part["type"].as_str() == Some("text"))
        .filter_map(|part| part["text"].as_str())
        .collect();
    (!parts.is_empty()).then(|| parts.join(" "))
}

fn title(s: &str) -> String {
    let one_line = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= 180 {
        one_line
    } else {
        format!("{}…", one_line.chars().take(179).collect::<String>())
    }
}

fn claude(root: &Path) -> Vec<Session> {
    // Main Claude sessions sit one directory below `projects`; nested JSONL is
    // subagent/task state and would duplicate the parent conversation.
    files(root, 1)
        .into_iter()
        .filter_map(|path| {
            let reader = BufReader::new(File::open(&path).ok()?);
            for line in reader.lines().map_while(Result::ok) {
                let v: Value = serde_json::from_str(&line).ok()?;
                if v["type"].as_str() != Some("user") {
                    continue;
                }
                if v["isSidechain"].as_bool() == Some(true) {
                    return None;
                }
                let Some(prompt) = text_content(&v["message"]["content"]) else {
                    continue;
                };
                return Some(Session {
                    agent: Agent::Claude,
                    id: v["sessionId"]
                        .as_str()
                        .map(String::from)
                        .or_else(|| path.file_stem().map(|s| s.to_string_lossy().into_owned()))?,
                    cwd: PathBuf::from(v["cwd"].as_str().unwrap_or(".")),
                    title: title(&prompt),
                    updated: modified(&path),
                    file: Some(path),
                    native_picker: false,
                });
            }
            None
        })
        .collect()
}

fn codex(root: &Path) -> Vec<Session> {
    files(root, 4)
        .into_iter()
        .filter_map(|path| {
            let reader = BufReader::new(File::open(&path).ok()?);
            let mut id = None;
            let mut cwd = None;
            let mut prompt = None;
            for line in reader.lines().map_while(Result::ok) {
                let Ok(v) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                match v["type"].as_str() {
                    Some("session_meta") => {
                        id = v["payload"]["id"]
                            .as_str()
                            .or_else(|| v["payload"]["session_id"].as_str())
                            .map(String::from);
                        cwd = v["payload"]["cwd"].as_str().map(PathBuf::from);
                    }
                    Some("event_msg") if v["payload"]["type"].as_str() == Some("user_message") => {
                        prompt = v["payload"]["message"].as_str().map(String::from);
                    }
                    _ => {}
                }
                if id.is_some() && cwd.is_some() && prompt.is_some() {
                    break;
                }
            }
            Some(Session {
                agent: Agent::Codex,
                id: id?,
                cwd: cwd?,
                title: title(&prompt?),
                updated: modified(&path),
                file: Some(path),
                native_picker: false,
            })
        })
        .collect()
}

fn pi(root: &Path) -> Vec<Session> {
    files(root, 2)
        .into_iter()
        .filter_map(|path| {
            let reader = BufReader::new(File::open(&path).ok()?);
            let mut id = None;
            let mut cwd = None;
            let mut name = None;
            let mut prompt = None;
            for line in reader.lines().map_while(Result::ok) {
                let Ok(v) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                match v["type"].as_str() {
                    Some("session") => {
                        id = v["id"].as_str().map(String::from);
                        cwd = v["cwd"].as_str().map(PathBuf::from);
                    }
                    Some("session_info") => name = v["name"].as_str().map(String::from),
                    Some("message")
                        if v["message"]["role"].as_str() == Some("user") && prompt.is_none() =>
                    {
                        prompt = text_content(&v["message"]["content"]);
                    }
                    _ => {}
                }
            }
            Some(Session {
                agent: Agent::Pi,
                id: id?,
                cwd: cwd?,
                title: title(
                    name.as_deref()
                        .or(prompt.as_deref())
                        .unwrap_or("untitled session"),
                ),
                updated: modified(&path),
                file: Some(path),
                native_picker: false,
            })
        })
        .collect()
}

pub fn list() -> Vec<Session> {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let mut sessions = Vec::new();
    if command_exists("claude") {
        sessions.extend(claude(&home.join(".claude/projects")));
    }
    if command_exists("codex") {
        sessions.extend(codex(&home.join(".codex/sessions")));
    }
    if command_exists("pi") {
        sessions.extend(pi(&home.join(".pi/agent/sessions")));
    }
    if command_exists("cursor-agent") {
        sessions.push(Session {
            agent: Agent::Cursor,
            id: String::new(),
            cwd: std::env::current_dir().unwrap_or_else(|_| home.clone()),
            title: "open Cursor's session picker".into(),
            updated: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            file: None,
            native_picker: true,
        });
    }
    sessions.sort_by_key(|s| std::cmp::Reverse(s.updated));
    sessions
}

pub fn age(updated: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(updated);
    let seconds = now.saturating_sub(updated);
    match seconds {
        0..=59 => "now".into(),
        60..=3599 => format!("{}m ago", seconds / 60),
        3600..=86_399 => format!("{}h ago", seconds / 3600),
        _ => format!("{}d ago", seconds / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("herdr-deck-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn flattens_and_truncates_titles() {
        assert_eq!(title("one\n  two"), "one two");
        assert_eq!(title(&"x".repeat(200)).chars().count(), 180);
    }

    #[test]
    fn reads_text_content_arrays() {
        let v = serde_json::json!([
            {"type": "text", "text": "hello"},
            {"type": "image", "url": "ignored"},
            {"type": "text", "text": "world"}
        ]);
        assert_eq!(text_content(&v).as_deref(), Some("hello world"));
    }

    #[test]
    fn discovers_claude_and_codex_fixtures() {
        let root = fixture_dir("sessions");
        let claude_dir = root.join("claude/project");
        let codex_dir = root.join("codex/2026/07/15");
        fs::create_dir_all(&claude_dir).unwrap();
        fs::create_dir_all(&codex_dir).unwrap();
        fs::write(
            claude_dir.join("c1.jsonl"),
            r#"{"type":"user","sessionId":"c1","cwd":"/tmp/project","isSidechain":false,"message":{"content":"hello claude"}}
"#,
        )
        .unwrap();
        fs::write(
            codex_dir.join("rollout.jsonl"),
            r#"{"type":"session_meta","payload":{"id":"x1","cwd":"/tmp/project"}}
{"type":"event_msg","payload":{"type":"user_message","message":"hello codex"}}
"#,
        )
        .unwrap();

        let c = claude(&root.join("claude"));
        let x = codex(&root.join("codex"));
        assert_eq!((c.len(), c[0].title.as_str()), (1, "hello claude"));
        assert_eq!((x.len(), x[0].title.as_str()), (1, "hello codex"));
        fs::remove_dir_all(root).unwrap();
    }
}
