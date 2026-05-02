use std::fs;
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::tty::IsTty;
use serde_json::Value;

use crate::cli::Command;
use crate::display::{self, fmt_time, print_list};
use crate::session::{self, find_session, session_pid_alive};
use crate::tree::{build_full_tree, is_user_turn};
use crate::tui;

// ── Dispatch ─────────────────────────────────────────────────────────

pub fn dispatch(command: Command, close_on_exit: bool) -> Result<()> {
    match command {
        Command::List { depth, upstream, max_len } => cmd_list(depth, upstream, max_len),
        Command::Summary => cmd_summary(),
        Command::Info => cmd_info(),
        Command::Tail => cmd_tail(),
        Command::Tui { session } => cmd_tui(close_on_exit, session),
    }
}

// ── list ─────────────────────────────────────────────────────────────

fn cmd_list(depth: usize, upstream: usize, max_len: usize) -> Result<()> {
    let (path, _info) = find_session().context("No active session.")?;
    let events = session::load_events(&path)?;
    let (all_nodes, _uuid_to_idx, root_indices) = build_full_tree(&events);

    print_list(&all_nodes, &root_indices, &events, depth, upstream, max_len);
    Ok(())
}

// ── summary ──────────────────────────────────────────────────────────

fn cmd_summary() -> Result<()> {
    let (path, info) = find_session().context("No active session.")?;
    let events = session::load_events(&path)?;

    let alive = if session_pid_alive(&info) {
        " ● running"
    } else {
        ""
    };
    let (tin, tout) = count_tokens(&events);
    let user_turns: Vec<&Value> = events.iter().filter(|e| is_user_turn(e)).collect();

    println!(
        "Session{alive} — {} user turns · {tin} in / {tout} out",
        user_turns.len()
    );
    println!();

    for (i, ev) in user_turns.iter().enumerate() {
        let text = display::preview(
            ev.get("message")
                .and_then(|m| m.as_object())
                .map(|m| Value::Object(m.clone()))
                .as_ref()
                .unwrap(),
        )
        .chars()
        .take(100)
        .collect::<String>();
        let ts = fmt_time(
            ev.get("timestamp")
                .and_then(|v| v.as_str())
                .unwrap_or(""),
        );
        println!("  [{}] {} {}", i + 1, ts, text);
    }
    Ok(())
}

// ── info ─────────────────────────────────────────────────────────────

fn cmd_info() -> Result<()> {
    let (path, info) = find_session().context("No active session.")?;
    let events = session::load_events(&path)?;

    let nu = events
        .iter()
        .filter(|e| e.get("type").and_then(|v| v.as_str()) == Some("user"))
        .count();
    let na = events
        .iter()
        .filter(|e| e.get("type").and_then(|v| v.as_str()) == Some("assistant"))
        .count();
    let (tin, tout) = count_tokens(&events);
    let alive = if session_pid_alive(&info) {
        "yes"
    } else {
        "no"
    };
    let fsize = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);

    println!(
        "Session ID:  {}",
        info.get("sessionId")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
    );
    println!("Path:        {}", path.display());
    println!(
        "Size:        {fsize} B ({fsize_kb:.0} KB)",
        fsize_kb = fsize as f64 / 1024.0
    );
    println!(
        "PID:         {}  (alive: {alive})",
        info.get("pid")
            .and_then(|v| v.as_i64())
            .map(|v| v.to_string())
            .unwrap_or_else(|| "?".to_string())
    );
    println!(
        "CWD:         {}",
        info.get("cwd").and_then(|v| v.as_str()).unwrap_or("?")
    );
    println!(
        "Started:     {}",
        info.get("procStart")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
    );
    println!(
        "Events:      {}  ({nu} user · {na} asst · {} other)",
        events.len(),
        events.len() - nu - na
    );
    println!(
        "Tokens:      {tin} in · {tout} out = {total} total",
        total = tin + tout
    );
    Ok(())
}

// ── tail ─────────────────────────────────────────────────────────────

fn cmd_tail() -> Result<()> {
    let (path, _info) = find_session().context("No active session.")?;
    let mut last_count = 0usize;

    loop {
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Error reading session: {e}");
                std::thread::sleep(Duration::from_secs(2));
                continue;
            }
        };

        let event_lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();

        if event_lines.len() > last_count {
            for line in &event_lines[last_count..] {
                if let Ok(ev) = serde_json::from_str::<Value>(line) {
                    let now = chrono::Local::now().format("%H:%M:%S").to_string();
                    match ev.get("type").and_then(|v| v.as_str()) {
                        Some(t) if t == "user" || t == "assistant" => {
                            let role = ev
                                .get("message")
                                .and_then(|m| m.get("role"))
                                .and_then(|v| v.as_str())
                                .unwrap_or(t);
                            let msg_obj = ev.get("message").and_then(|m| m.as_object()).cloned();
                            let p = msg_obj
                                .as_ref()
                                .map(|m| display::preview(&Value::Object(m.clone())))
                                .unwrap_or_default();
                            let p = p.chars().take(80).collect::<String>();
                            println!("[{now}] [{role}] {p}");
                        }
                        Some(t) => println!("[{now}] [{t}]"),
                        None => println!("[{now}] [unknown]"),
                    }
                }
            }
            last_count = event_lines.len();
        }

        std::thread::sleep(Duration::from_secs(1));
    }
}

// ── tui ──────────────────────────────────────────────────────────────

fn cmd_tui(close_on_exit: bool, session_id: Option<String>) -> Result<()> {
    if !std::io::stdout().is_tty() {
        // Running inside Claude Code — open a new Terminal window
        let (_path, info) = find_session().context("No active session.")?;
        let sid = session_id.unwrap_or_else(|| {
            info.get("sessionId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        });
        std::process::Command::new("osascript")
            .args(["-e", &format!(r#"tell app "Terminal" to do script "ctx --close-on-exit tui --session {}""#, sid)])
            .spawn()
            .context("Failed to open Terminal window. ctx tui requires macOS Terminal.app.")?;
        println!("Opened session tree in a new Terminal window.");
        return Ok(());
    }

    // Running in a real Terminal
    match session_id {
        Some(sid) => {
            let (path, _) = session::find_session_by_id(&sid).context("No active session.")?;
            tui::run_tui_with_path(path)?;
        }
        None => {
            tui::run_tui()?;
        }
    }

    if close_on_exit {
        let _ = std::process::Command::new("osascript")
            .args(["-e", r#"tell application "Terminal" to close front window"#])
            .spawn();
    }

    Ok(())
}

// ── Shared helpers ───────────────────────────────────────────────────

fn count_tokens(events: &[Value]) -> (u64, u64) {
    let mut tin = 0u64;
    let mut tout = 0u64;
    for ev in events {
        if ev.get("type").and_then(|v| v.as_str()) != Some("assistant") {
            continue;
        }
        let usage = match ev.get("message").and_then(|m| m.get("usage")) {
            Some(Value::Object(_)) => ev.get("message").unwrap().get("usage").unwrap(),
            _ => continue,
        };
        tin += usage
            .get("input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            + usage
                .get("cache_creation_input_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
            + usage
                .get("cache_read_input_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
        tout += usage
            .get("output_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
    }
    (tin, tout)
}
