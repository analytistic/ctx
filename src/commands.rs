use std::collections::HashSet;
use std::fs;
use std::process;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::Value;
use uuid::Uuid;

use crate::cli::Command;
use crate::display::{self, child_line, fmt_time, node_model_suffix, node_tag, print_list, shorten};
use crate::session::{self, find_session, guard_session, load_cwd, save_cwd, session_pid_alive};
use crate::tree::{self, build_tree, get_event_uuid, is_user_turn, resolve_ls, user_turn_list};

// ── Dispatch ─────────────────────────────────────────────────────────

pub fn dispatch(command: Command) -> Result<()> {
    match command {
        Command::Ls => cmd_ls(),
        Command::Cd { path } => cmd_cd(path),
        Command::Pwd => cmd_pwd(),
        Command::List { depth, upstream, max_len } => cmd_list(depth, upstream, max_len),
        Command::Summary => cmd_summary(),
        Command::Info => cmd_info(),
        Command::Tail => cmd_tail(),
        Command::Insert { under, text } => cmd_insert(under, &text),
        Command::Rm { uuid } => cmd_rm(&uuid),
        Command::Export { file } => cmd_export(file.as_deref()),
    }
}

// ── list ─────────────────────────────────────────────────────────────

fn cmd_list(depth: usize, upstream: usize, max_len: usize) -> Result<()> {
    let (path, _info) = find_session().context("No active session.")?;
    let events = session::load_events(&path)?;
    let (all_nodes, _uuid_to_idx, root_indices) = tree::build_full_tree(&events);

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

// ── insert ───────────────────────────────────────────────────────────

fn cmd_insert(parent_uuid: Option<String>, text: &[String]) -> Result<()> {
    let text = text.join(" ");
    let (path, info) = find_session().context("No active session.")?;
    guard_session(&info)?;

    let timestamp = chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S.%6fZ")
        .to_string();
    let session_id = info.get("sessionId").and_then(|v| v.as_str());

    let mut note = serde_json::json!({
        "type": "user",
        "uuid": Uuid::new_v4().simple().to_string(),
        "timestamp": timestamp,
        "userType": "ctx-tool",
        "sessionId": session_id,
        "message": {
            "role": "user",
            "content": format!("[ctx note] {text}")
        }
    });

    if let Some(ref puid) = parent_uuid {
        note["parentUuid"] = serde_json::json!(puid);
    }

    session::append_event(&path, &note)?;

    match &parent_uuid {
        Some(pid) => println!(
            "✓ Inserted note under {}…: {}",
            &pid[..12.min(pid.len())],
            shorten(&text, 80)
        ),
        None => println!("✓ Inserted note as root: {}", shorten(&text, 80)),
    }
    Ok(())
}

// ── rm ───────────────────────────────────────────────────────────────

fn cmd_rm(uuid: &str) -> Result<()> {
    let (path, info) = find_session().context("No active session.")?;
    guard_session(&info)?;

    let events = session::load_events(&path)?;
    let (all_nodes, uuid_to_idx, _) = build_tree(&events);

    let target_idx = match uuid_to_idx.get(uuid) {
        Some(&idx) => idx,
        None => {
            eprintln!("UUID {uuid} not found.");
            process::exit(1);
        }
    };

    let mut to_remove = HashSet::new();
    tree::collect_uuids(&all_nodes, target_idx, &mut to_remove);

    let before = events.len();
    let filtered: Vec<Value> = events
        .into_iter()
        .filter(|ev| !to_remove.contains(&get_event_uuid(ev)))
        .collect();

    let removed = before - filtered.len();
    session::save_events(&path, &filtered)?;
    println!(
        "✗ Removed {removed} event(s) (subtree of {}…)",
        &uuid[..12.min(uuid.len())]
    );
    Ok(())
}

// ── export ───────────────────────────────────────────────────────────

fn cmd_export(file: Option<&str>) -> Result<()> {
    let (path, info) = find_session().context("No active session.")?;
    let events = session::load_events(&path)?;

    let sid = info
        .get("sessionId")
        .and_then(|v| v.as_str())
        .unwrap_or("x");
    let sid_prefix = &sid[..8.min(sid.len())];
    let default_path = format!("session-{sid_prefix}.json");
    let out_path = file.unwrap_or(&default_path);

    let out = fs::File::create(out_path)?;
    serde_json::to_writer_pretty(out, &events)?;
    println!("Exported {} events → {out_path}", events.len());
    Ok(())
}

// ── Filesystem-style navigation (deprecated, will be redesigned) ─────

fn cmd_ls() -> Result<()> {
    let (path, info) = find_session().context("No active session.")?;
    let session_id = info.get("sessionId").and_then(|v| v.as_str()).unwrap_or("");
    let events = session::load_events(&path)?;
    let (all_nodes, uuid_to_idx, root_indices) = build_tree(&events);
    let cwd = load_cwd(session_id);

    let (node, children_indices) = {
        let (n, ci) = resolve_ls(&all_nodes, &root_indices, &cwd);
        if !cwd.is_empty() && n.is_none() {
            save_cwd(session_id, &[]);
            (None, &root_indices[..])
        } else {
            (n, &ci[..])
        }
    };

    match node {
        None => {
            let (tin, tout) = count_tokens(&events);
            let alive = if session_pid_alive(&info) { " ● running" } else { "" };
            let sid = info.get("sessionId").and_then(|v| v.as_str()).unwrap_or("?");
            println!("Session {}…  {tin} in · {tout} out{alive}", &sid[..12.min(sid.len())]);
            println!("{}", "─".repeat(50));

            let turns = user_turn_list(&events, &all_nodes, &uuid_to_idx);
            if turns.is_empty() {
                println!("(no messages)");
            } else {
                for (i, (_, _, node)) in turns.iter().enumerate() {
                    let ts = if node.timestamp.is_empty() { String::new() } else { fmt_time(&node.timestamp) };
                    let p = node.preview.chars().take(80).collect::<String>();
                    println!(" {}│ user {} {}", i + 1, ts, p);
                }
            }
        }
        Some(n) => {
            println!("{}{} {}", node_tag(n), node_model_suffix(n), fmt_time(&n.timestamp));
            let text = if n.content_text.is_empty() { &n.preview } else { &n.content_text };
            if !text.is_empty() {
                println!("{text}");
            }
            println!("{}", "─".repeat(50));

            if children_indices.is_empty() {
                println!("(no children)");
            } else {
                for (i, &ci) in children_indices.iter().enumerate() {
                    println!("{}", child_line(i + 1, &all_nodes[ci]));
                }
            }
        }
    }

    Ok(())
}

fn cmd_cd(path_args: Vec<String>) -> Result<()> {
    let (_path, info) = find_session().context("No active session.")?;
    let session_id = info.get("sessionId").and_then(|v| v.as_str()).unwrap_or("");
    let path_str = path_args.join(" ");

    let current_cwd = load_cwd(session_id);
    let at_root = current_cwd.is_empty();
    let mut new_cwd = current_cwd;

    if path_str == "/" || path_str.is_empty() {
        new_cwd.clear();
    } else {
        for segment in path_str.split('/') {
            let seg = segment.trim();
            match seg {
                "" | "." => continue,
                ".." => {
                    new_cwd.pop();
                }
                num => {
                    let n: usize = match num.parse() {
                        Ok(n) if n > 0 => n,
                        _ => {
                            eprintln!("Invalid path: {num} is not a valid index");
                            process::exit(1);
                        }
                    };
                    new_cwd.push(n);
                }
            }
        }
    }

    if at_root && new_cwd.len() == 1 && !path_str.contains('/') {
        let events = session::load_events(&_path)?;
        let (all_nodes, uuid_to_idx, _) = build_tree(&events);
        let turns = user_turn_list(&events, &all_nodes, &uuid_to_idx);
        let turn_n = new_cwd[0];
        if turn_n > 0 && turn_n <= turns.len() {
            new_cwd = turns[turn_n - 1].1.clone();
        } else {
            eprintln!("No such turn (there are {} user messages)", turns.len());
            process::exit(1);
        }
    }

    let events = session::load_events(&_path)?;
    let (all_nodes, _uuid_to_idx, root_indices) = build_tree(&events);
    let (node, _) = resolve_ls(&all_nodes, &root_indices, &new_cwd);
    if !new_cwd.is_empty() && node.is_none() {
        eprintln!("No such node");
        process::exit(1);
    }

    save_cwd(session_id, &new_cwd);
    Ok(())
}

fn cmd_pwd() -> Result<()> {
    let (_path, info) = find_session().context("No active session.")?;
    let session_id = info.get("sessionId").and_then(|v| v.as_str()).unwrap_or("");
    let cwd = load_cwd(session_id);

    if cwd.is_empty() {
        println!("/");
    } else {
        println!(
            "/{}",
            cwd.iter()
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join("/")
        );
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
