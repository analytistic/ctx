use std::sync::LazyLock;

use chrono::{DateTime, Local};
use colored::Colorize;
use regex::Regex;
use serde_json::Value;

use crate::tree::{get_event_uuid, TreeNode};

// ── Tag cleaning ─────────────────────────────────────────────────────

static REMOVE_CAVEAT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)<local-command-caveat>[^<]*</local-command-caveat>").unwrap()
});
static REMOVE_SYSTEM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)<system-reminder>[^<]*</system-reminder>").unwrap()
});
static REMOVE_THINKING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)<thinking>[^<]*</thinking>").unwrap()
});
static BLOCK_TAGS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)</?(?:local-command-caveat|system-reminder|thinking)[^>]*>").unwrap()
});
static WRAPPER_TAGS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)</?(?:bash-input|bash-stdout|bash-stderr|tool_result|result)[^>]*>").unwrap()
});

pub fn clean_system_tags(text: &str) -> String {
    let text = REMOVE_CAVEAT.replace_all(text, "");
    let text = REMOVE_SYSTEM.replace_all(&text, "");
    let text = REMOVE_THINKING.replace_all(&text, "");
    let text = BLOCK_TAGS.replace_all(&text, "");
    let text = WRAPPER_TAGS.replace_all(&text, "");
    text.trim().to_string()
}

// ── Formatting helpers ───────────────────────────────────────────────

pub fn shorten_mid(text: &str, n: usize) -> String {
    let text: String = text.replace('\n', " ").replace('\r', "");
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= n {
        text
    } else if n <= 3 {
        let take = n.min(chars.len());
        chars.into_iter().take(take).collect()
    } else {
        let half = (n - 3) / 2;
        let front: String = chars.iter().take(half).collect();
        let back: String = chars.iter().skip(chars.len() - half).take(half).collect();
        format!("{}...{}", front, back)
    }
}

fn extract_tool_result_text(b: &Value) -> Option<String> {
    let content = b.get("content")?;
    match content {
        Value::String(s) => {
            let cleaned = clean_system_tags(s);
            if cleaned.is_empty() { None } else { Some(cleaned) }
        }
        Value::Array(arr) => {
            let parts: Vec<String> = arr.iter().filter_map(|block| {
                if block.get("type").and_then(|v| v.as_str()) == Some("text") {
                    let text = block.get("text").and_then(|v| v.as_str())?;
                    let cleaned = clean_system_tags(text);
                    if cleaned.is_empty() { None } else { Some(cleaned) }
                } else {
                    None
                }
            }).collect();
            if parts.is_empty() { None } else { Some(parts.join(" ")) }
        }
        _ => None,
    }
}

fn summarize_tool_input(b: &Value) -> String {
    match b.get("input") {
        Some(Value::Object(obj)) => {
            // For Bash, show the command
            if let Some(cmd) = obj.get("command").and_then(|v| v.as_str()) {
                return cmd.to_string();
            }
            // For Read, show file_path
            if let Some(path) = obj.get("file_path").and_then(|v| v.as_str()) {
                return path.to_string();
            }
            // For Write/Edit, show file_path
            if let Some(path) = obj.get("file_path").and_then(|v| v.as_str()) {
                return path.to_string();
            }
            // Otherwise serialize the whole input
            serde_json::to_string(&Value::Object(obj.clone())).unwrap_or_default()
        }
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

fn format_block_preview(b: &Value) -> Option<String> {
    let t = b.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match t {
        "text" => {
            let text = b.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let cleaned = clean_system_tags(text);
            if cleaned.is_empty() { None } else { Some(cleaned) }
        }
        "thinking" => {
            let text = b.get("plain_text")
                .or_else(|| b.get("thinking"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let cleaned = clean_system_tags(text);
            if cleaned.is_empty() { None }
            else { Some(format!("[thinking] {}", cleaned)) }
        }
        "tool_use" => {
            let name = b.get("name").and_then(|v| v.as_str()).unwrap_or("?");
            let input = summarize_tool_input(b);
            Some(format!("[{}] {}", name, input))
        }
        "tool_result" | "result" => extract_tool_result_text(b),
        _ => None,
    }
}

pub fn preview(msg: &Value) -> String {
    match msg.get("content") {
        Some(Value::String(s)) => clean_system_tags(s),
        Some(Value::Array(blocks)) => {
            let parts: Vec<String> = blocks.iter().filter_map(format_block_preview).collect();
            if parts.is_empty() { String::new() } else { parts.join(" ") }
        }
        _ => String::new(),
    }
}

pub fn get_content_text(msg: &Value) -> String {
    match msg.get("content") {
        Some(Value::String(s)) => clean_system_tags(s),
        Some(Value::Array(blocks)) => {
            let parts: Vec<String> = blocks
                .iter()
                .filter_map(|b| {
                    let t = b.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    match t {
                        "text" => {
                            let text = b.get("text").and_then(|v| v.as_str()).unwrap_or("");
                            let cleaned = clean_system_tags(text);
                            if cleaned.is_empty() { None } else { Some(cleaned) }
                        }
                        "tool_use" => {
                            let name = b.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                            let input = b.get("input")
                                .map(|v| v.to_string())
                                .filter(|s| s != "null")
                                .unwrap_or_default();
                            if input.is_empty() {
                                Some(format!("[{}]", name))
                            } else {
                                Some(format!("[{}({})]", name, input))
                            }
                        }
                        "tool_result" | "result" => extract_tool_result_text(b),
                        "thinking" => {
                            let text = b.get("plain_text")
                                .or_else(|| b.get("thinking"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            let cleaned = clean_system_tags(text);
                            if cleaned.is_empty() { None } else { Some(cleaned) }
                        }
                        _ => None,
                    }
                })
                .collect();
            parts.join(" ")
        }
        _ => String::new(),
    }
}

pub fn fmt_time(ts: &str) -> String {
    if ts.is_empty() {
        return String::new();
    }
    let normalized = ts.replace('Z', "+00:00");
    if let Ok(dt) = DateTime::parse_from_rfc3339(&normalized) {
        let local = dt.with_timezone(&Local);
        let now = Local::now();
        if local.date_naive() == now.date_naive() {
            return local.format("%H:%M:%S").to_string();
        } else {
            return local.format("%m/%d %H:%M").to_string();
        }
    }
    String::new()
}

// ── List display ─────────────────────────────────────────────────────

const HIDDEN_TYPES: &[&str] = &[
    "system",
    "attachment",
    "permission-mode",
    "last-prompt",
    "queue-operation",
    "file-history-snapshot",
];

/// Print the tree centered on the current position.
pub fn print_list(
    all_nodes: &[TreeNode],
    root_indices: &[usize],
    events: &[Value],
    depth: usize,
    upstream: usize,
    max_len: usize,
) {
    let current_idx = find_last_content_node(all_nodes, events);

    // Build full path from root to current
    let (root_of_current, root_path) = match current_idx {
        Some(cur) => {
            let full = path_to_root(all_nodes, cur);
            let root = *full.first().unwrap_or(&cur);
            (Some(root), full)
        }
        None => (None, Vec::new()),
    };

    let sid = events
        .first()
        .and_then(|e| e.get("sessionId"))
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    println!("Session {}…", &sid[..12.min(sid.len())]);
    println!();

    for (i, &root_idx) in root_indices.iter().enumerate() {
        if i > 0 { println!(); }
        let node = &all_nodes[root_idx];
        let line = node_line(node, max_len);
        println!("── {}", line);
        if node.children.is_empty() { continue; }

        if root_of_current == Some(root_idx) && root_path.len() > 1 {
            print_upstream_path(
                all_nodes, &root_path, current_idx, "   ", depth, max_len, upstream,
            );
        } else if depth > 0 {
            for &child_idx in node.children.iter() {
                print_collapsed(all_nodes, child_idx, "   ", depth, max_len);
            }
        }
    }
}

fn path_to_root(all_nodes: &[TreeNode], mut idx: usize) -> Vec<usize> {
    let mut path = Vec::new();
    loop {
        path.push(idx);
        match all_nodes[idx].parent {
            Some(ref puid) => {
                if let Some(pidx) = all_nodes.iter().position(|n| n.uuid == *puid) {
                    idx = pidx;
                } else { break; }
            }
            None => break,
        }
    }
    path.reverse();
    path
}

fn find_last_content_node(all_nodes: &[TreeNode], events: &[Value]) -> Option<usize> {
    for ev in events.iter().rev() {
        let uid = get_event_uuid(ev);
        if !uid.is_empty() {
            if let Some(idx) = all_nodes.iter().position(|n| n.uuid == uid) {
                return Some(idx);
            }
        }
    }
    None
}

/// Print a branch collapsed to `rem_depth` levels. Shows leaf tail at truncation.
fn print_collapsed(
    all_nodes: &[TreeNode],
    node_idx: usize,
    prefix: &str,
    rem_depth: usize,
    max_len: usize,
) {
    let node = &all_nodes[node_idx];

    let line = node_line(node, max_len);
    println!("{prefix}── {}", line);

    if rem_depth > 1 && !node.children.is_empty() {
        let child_prefix = format!("{}   ", prefix);
        for &child_idx in node.children.iter() {
            print_collapsed(all_nodes, child_idx, &child_prefix, rem_depth - 1, max_len);
        }
    } else if !node.children.is_empty() {
        let total = count_subtree(all_nodes, node_idx) - 1;
        if total > 0 {
            let last = leaf_node(all_nodes, node);
            println!("{prefix}   ┌── [{} levels]", total);
            println!("{prefix}   └── {}", node_line(last, max_len));
        }
    }
}

/// Walk along `root_path` from root toward current.
/// Shows first `depth` levels expanded, then `[N levels]` gap, then last `upstream+1` levels.
fn print_upstream_path(
    all_nodes: &[TreeNode],
    root_path: &[usize],
    current_idx: Option<usize>,
    prefix: &str,
    depth: usize,
    max_len: usize,
    upstream: usize,
) {
    if root_path.len() < 2 {
        return;
    }

    let show_count = (upstream + 1).min(root_path.len());
    let upstream_start = root_path.len() - show_count;

    // Siblings of root_path[1] under root
    let root = &all_nodes[root_path[0]];
    if let Some(pos) = root.children.iter().position(|&c| c == root_path[1]) {
        for &sib in root.children[..pos].iter() {
            print_collapsed(all_nodes, sib, prefix, depth, max_len);
        }
        for &sib in root.children[pos + 1..].iter() {
            print_collapsed(all_nodes, sib, prefix, depth, max_len);
        }
    }

    let mut cur_prefix = prefix.to_string();

    // Phase 1: depth expansion — show first `depth` tree levels along the path.
    // Each path node = one tree level, regardless of visibility.
    let mut i = 1usize;
    while i < root_path.len() && i <= depth {
        let node = &all_nodes[root_path[i]];

        if i + 1 < root_path.len() {
            let next_idx = root_path[i + 1];
            if let Some(pos) = node.children.iter().position(|&c| c == next_idx) {
                let sp = format!("{}   ", cur_prefix);
                for &sib in node.children[..pos].iter() {
                    print_collapsed(all_nodes, sib, &sp, depth, max_len);
                }
                println!("{cur_prefix}── {}", node_line(node, max_len));
                for &sib in node.children[pos + 1..].iter() {
                    print_collapsed(all_nodes, sib, &sp, depth, max_len);
                }
            }
        } else {
            // Depth covers everything — last node is current
            println!("{cur_prefix}── {}  ←", node_line(node, max_len));
            return;
        }

        i += 1;
        cur_prefix = format!("{}   ", cur_prefix);
    }

    // Phase 2: hidden gap between depth and upstream ranges
    if i < upstream_start {
        let hidden = upstream_start - i;
        let gap_last = &all_nodes[root_path[upstream_start - 1]];
        let sp = format!("{}   ", cur_prefix);
        println!("{sp}┌── [{} levels]", hidden);
        println!("{sp}└── {}", node_line(gap_last, max_len));
        cur_prefix = sp;
    }

    // Phase 3: upstream tail — root_path[max(i, upstream_start)..]
    let phase3_start = i.max(upstream_start);
    for i in phase3_start..root_path.len() {
        let node = &all_nodes[root_path[i]];
        let is_last = i == root_path.len() - 1;

        if !is_last {
            let next_idx = root_path[i + 1];
            if let Some(pos) = node.children.iter().position(|&c| c == next_idx) {
                let sp = format!("{}   ", cur_prefix);
                for &sib in node.children[..pos].iter() {
                    print_collapsed(all_nodes, sib, &sp, depth, max_len);
                }
                println!("{cur_prefix}── {}", node_line(node, max_len));
                for &sib in node.children[pos + 1..].iter() {
                    print_collapsed(all_nodes, sib, &sp, depth, max_len);
                }
            }
        } else {
            let is_current = Some(root_path[i]) == current_idx;
            let marker = if is_current { "  ←".yellow().to_string() } else { String::new() };
            println!("{cur_prefix}── {}{}", node_line(node, max_len), marker);
        }

        cur_prefix = format!("{}   ", cur_prefix);
    }
}

fn node_line(node: &TreeNode, max_len: usize) -> String {
    let ts = fmt_time(&node.timestamp);
    let tag = if node.role == "user" {
        "user".to_string()
    } else if HIDDEN_TYPES.contains(&node.role.as_str()) {
        node.role.clone()
    } else {
        "Claude".to_string()
    };
    let model = if node.role == "assistant" && !node.model.is_empty() {
        format!(" [{}]", node.model.rsplit('/').next().unwrap_or(&node.model))
    } else {
        String::new()
    };
    let content = if node.preview.is_empty() {
        &node.content_text
    } else {
        &node.preview
    };
    let text = if content.is_empty() {
        String::new()
    } else {
        shorten_mid(content, max_len)
    };
    let line = if ts.is_empty() {
        format!("{}{} {}", tag, model, text)
    } else {
        format!("{}{} {} {}", tag, model, ts, text)
    };
    if node.role == "user" {
        line.green().to_string()
    } else {
        line
    }
}

fn count_subtree(all_nodes: &[TreeNode], idx: usize) -> usize {
    let node = &all_nodes[idx];
    if node.children.is_empty() { return 1; }
    1 + node.children.iter().map(|&c| count_subtree(all_nodes, c)).sum::<usize>()
}

fn leaf_node<'a>(all_nodes: &'a [TreeNode], node: &'a TreeNode) -> &'a TreeNode {
    if node.children.is_empty() {
        node
    } else {
        let last = node.children[node.children.len() - 1];
        leaf_node(all_nodes, &all_nodes[last])
    }
}
