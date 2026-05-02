use std::fs;
use std::path::{Path, PathBuf};
use std::process;

use anyhow::Result;
use serde_json::Value;

fn home_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| {
        eprintln!("HOME environment variable not set");
        process::exit(1);
    });
    PathBuf::from(home)
}

fn sessions_dir() -> PathBuf {
    home_dir().join(".claude/sessions")
}

fn projects_dir() -> PathBuf {
    home_dir().join(".claude/projects")
}

/// Infer the JSONL path from session metadata when the file doesn't exist yet.
/// Handles the race where `!ctx` runs before Claude Code has flushed the
/// first events to disk (JSONL created lazily on first persist).
fn infer_jsonl_path(sid: &str, info: &Value) -> PathBuf {
    // Transform cwd into the project directory name used by Claude Code:
    //   /Users/alex/projects/ctx  →  -Users-alex-projects-ctx
    if let Some(cwd) = info.get("cwd").and_then(|v| v.as_str()) {
        let proj_name = format!("-{}", cwd.trim_start_matches('/').replace('/', "-"));
        let proj_dir = projects_dir().join(&proj_name);
        if proj_dir.exists() {
            return proj_dir.join(format!("{sid}.jsonl"));
        }
    }
    sessions_dir().join(format!("{sid}.jsonl"))
}

/// Walk up the process tree to find the Claude Code session.
/// `ctx` is typically spawned as: claude -> zsh -> ctx
/// We check each ancestor PID against `~/.claude/sessions/{pid}.json`.
fn find_by_pid_chain() -> Option<(PathBuf, Value)> {
    let sdir = sessions_dir();
    let mut pid: Option<u32> = Some(process::id() as u32);

    for _ in 0..20 {
        let p = pid?;
        let sess_file = sdir.join(format!("{p}.json"));
        if let Ok(content) = fs::read_to_string(&sess_file) {
            if let Ok(info) = serde_json::from_str::<Value>(&content) {
                let sid = info.get("sessionId")?.as_str()?;
                let jsonl = find_jsonl(sid).unwrap_or_else(|| {
                    // JSONL may not exist yet (Claude Code creates it lazily
                    // on first event persist). Construct the expected path
                    // from cwd + sessionId instead of giving up.
                    infer_jsonl_path(sid, &info)
                });
                return Some((jsonl, info));
            }
        }

        pid = parent_pid(p);
    }
    None
}

/// Get parent PID via `ps`.
fn parent_pid(pid: u32) -> Option<u32> {
    let output = process::Command::new("ps")
        .args(["-o", "ppid=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&output.stdout);
    s.trim().parse().ok()
}

/// Find JSONL by session ID — check projects dir first, then sessions dir.
fn find_jsonl(sid: &str) -> Option<PathBuf> {
    let pdir = projects_dir();
    if let Ok(proj_dir) = fs::read_dir(&pdir) {
        for entry in proj_dir.filter_map(|e| e.ok()) {
            let jsonl = entry.path().join(format!("{sid}.jsonl"));
            if jsonl.exists() {
                return Some(jsonl);
            }
        }
    }
    let fallback = sessions_dir().join(format!("{sid}.jsonl"));
    if fallback.exists() {
        return Some(fallback);
    }
    None
}

/// Fallback: sort by time (works when ctx is run directly in shell, not via !ctx).
fn find_by_time() -> Option<(PathBuf, Value)> {
    let sdir = sessions_dir();
    let pdir = projects_dir();

    let mut entries: Vec<_> = fs::read_dir(&sdir).ok()?.filter_map(|e| e.ok()).collect();
    entries.sort_by(|a, b| {
        b.metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
            .cmp(
                &a.metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH),
            )
    });

    for entry in entries {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let info: Value = fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())?;
        let sid = info.get("sessionId")?.as_str()?;

        if let Ok(proj_dir) = fs::read_dir(&pdir) {
            for proj in proj_dir.filter_map(|e| e.ok()) {
                let jsonl = proj.path().join(format!("{sid}.jsonl"));
                if jsonl.exists() {
                    return Some((jsonl, info));
                }
            }
        }
        let jsonl = path.with_extension("jsonl");
        if jsonl.exists() {
            return Some((jsonl, info));
        }
    }
    None
}

/// Find the current session: try PID chain first, then alive-PID scan, then time fallback.
pub fn find_session() -> Option<(PathBuf, Value)> {
    find_by_pid_chain()
        .or_else(|| find_by_alive_pid())
        .or_else(find_by_time)
}

/// Find a session by explicit session ID.
pub fn find_session_by_id(sid: &str) -> Option<(PathBuf, Value)> {
    let jsonl = find_jsonl(sid)?;
    // Load the matching session metadata
    let sdir = sessions_dir();
    if let Ok(entries) = fs::read_dir(&sdir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(info) = serde_json::from_str::<Value>(&content) {
                    if info.get("sessionId").and_then(|v| v.as_str()) == Some(sid) {
                        return Some((jsonl, info));
                    }
                }
            }
        }
    }
    // JSONL found but no metadata — still return the path
    Some((jsonl, Value::Null))
}

/// Scan all session files for alive `claude` processes.
/// Returns the most recently modified session file whose PID is alive
/// and running as `claude`.
fn find_by_alive_pid() -> Option<(PathBuf, Value)> {
    let sdir = sessions_dir();
    let mut candidates: Vec<(PathBuf, Value, std::time::SystemTime)> = Vec::new();

    let entries = fs::read_dir(&sdir).ok()?;
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let info: Value = fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok())?;
        let pid = info.get("pid").and_then(|v| v.as_i64())? as u32;

        if !pid_is_alive_claude(pid) {
            continue;
        }

        let mtime = path.metadata().ok()?.modified().ok()?;
        candidates.push((path, info, mtime));
    }

    // Most recently modified first
    candidates.sort_by(|a, b| b.2.cmp(&a.2));
    let (_path, info, _mtime) = candidates.into_iter().next()?;

    let sid = info.get("sessionId")?.as_str()?;
    let jsonl = find_jsonl(sid).unwrap_or_else(|| infer_jsonl_path(sid, &info));
    Some((jsonl, info))
}

/// Check if a PID is alive and running the `claude` binary.
fn pid_is_alive_claude(pid: u32) -> bool {
    let alive = process::Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !alive {
        return false;
    }

    let output = match process::Command::new("ps")
        .args(["-o", "command=", "-p", &pid.to_string()])
        .output()
        .ok()
    {
        Some(o) => o,
        None => return false,
    };
    let cmdline = String::from_utf8_lossy(&output.stdout);
    cmdline.contains("claude")
}

// ── Event I/O ────────────────────────────────────────────────────────

pub fn load_events(path: &Path) -> Result<Vec<Value>> {
    let content = fs::read_to_string(path)?;
    Ok(content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect())
}


// ── PID checking ─────────────────────────────────────────────────────

pub fn session_pid_alive(info: &Value) -> bool {
    let pid = match info.get("pid").and_then(|v| v.as_i64()) {
        Some(pid) => pid,
        None => return false,
    };
    process::Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
