use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process;

use anyhow::{Context, Result};
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
                if let Some(jsonl) = find_jsonl(sid) {
                    return Some((jsonl, info));
                }
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

/// Find the current session: try PID chain first, then time fallback.
pub fn find_session() -> Option<(PathBuf, Value)> {
    find_by_pid_chain().or_else(find_by_time)
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

pub fn save_events(path: &Path, events: &[Value]) -> Result<()> {
    let tmp_path = PathBuf::from(format!("{}.tmp", path.display()));
    let mut file = fs::File::create(&tmp_path)
        .with_context(|| format!("Failed to create temp file {:?}", tmp_path))?;
    for ev in events {
        writeln!(file, "{}", serde_json::to_string(ev)?)?;
    }
    file.flush()?;
    fs::rename(&tmp_path, path)
        .with_context(|| format!("Failed to rename {:?} to {:?}", tmp_path, path))?;
    Ok(())
}

pub fn append_event(path: &Path, event: &Value) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{}", serde_json::to_string(event)?)?;
    file.flush()?;
    Ok(())
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

// ── CWD state (deprecated, kept for legacy ls/cd/pwd) ────────────────

fn cwd_state_path() -> PathBuf {
    home_dir().join(".claude/ctx-cwd.json")
}

pub fn load_cwd(session_id: &str) -> Vec<usize> {
    let content = match fs::read_to_string(cwd_state_path()) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let data: Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    if data.get("session_id").and_then(|v| v.as_str()) != Some(session_id) {
        return Vec::new();
    }
    data.get("cwd")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_u64()).map(|v| v as usize).collect())
        .unwrap_or_default()
}

pub fn save_cwd(session_id: &str, cwd: &[usize]) {
    if let Ok(content) = serde_json::to_string(&serde_json::json!({
        "session_id": session_id,
        "cwd": cwd,
    })) {
        let _ = fs::write(cwd_state_path(), &content);
    }
}

pub fn guard_session(info: &Value) -> Result<()> {
    if let Some(pid) = info.get("pid").and_then(|v| v.as_i64()) {
        if session_pid_alive(info) {
            anyhow::bail!("✗ Claude is still running (PID {pid}). Stop it first, then retry.");
        }
    }
    Ok(())
}
