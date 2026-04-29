use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::display::{get_content_text, preview};

#[derive(Debug)]
pub struct TreeNode {
    pub uuid: String,
    pub parent: Option<String>,
    pub role: String,
    pub preview: String,
    pub content_text: String,
    pub timestamp: String,
    pub children: Vec<usize>,
    pub model: String,
    pub has_tools: bool,
}

pub fn get_event_uuid(ev: &Value) -> String {
    ev.get("uuid")
        .and_then(|v| v.as_str())
        .or_else(|| {
            ev.get("message")
                .and_then(|m| m.get("id"))
                .and_then(|v| v.as_str())
        })
        .unwrap_or_default()
        .to_string()
}

fn find_shown_ancestor(
    uuid: &str,
    all_parents: &HashMap<String, Option<String>>,
    shown: &HashSet<String>,
) -> Option<String> {
    let mut current = uuid.to_string();
    for _ in 0..50 {
        match all_parents.get(&current) {
            Some(Some(parent)) => {
                if shown.contains(parent) {
                    return Some(parent.clone());
                }
                current = parent.clone();
            }
            _ => return None,
        }
    }
    None
}

/// Build tree of user/assistant events only (skips system/attachment etc.).
pub fn build_tree(events: &[Value]) -> (Vec<TreeNode>, HashMap<String, usize>, Vec<usize>) {
    let mut all_parents: HashMap<String, Option<String>> = HashMap::new();
    for ev in events {
        let uid = get_event_uuid(ev);
        if uid.is_empty() {
            continue;
        }
        let parent = ev
            .get("parentUuid")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        all_parents.insert(uid, parent);
    }

    let mut shown_uuids: HashSet<String> = HashSet::new();
    for ev in events {
        if matches!(ev.get("type").and_then(|v| v.as_str()), Some("user" | "assistant")) {
            let uid = get_event_uuid(ev);
            if !uid.is_empty() {
                shown_uuids.insert(uid);
            }
        }
    }

    let mut all_nodes: Vec<TreeNode> = Vec::new();
    let mut uuid_to_idx: HashMap<String, usize> = HashMap::new();

    for ev in events {
        let ev_type = match ev.get("type").and_then(|v| v.as_str()) {
            Some(t) if t == "user" || t == "assistant" => t,
            _ => continue,
        };

        let msg = match ev.get("message") {
            Some(Value::Object(_)) => ev.get("message").unwrap(),
            _ => continue,
        };

        let uid = get_event_uuid(ev);
        if uid.is_empty() {
            continue;
        }

        let role = msg
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or(ev_type)
            .to_string();

        let raw_parent = ev
            .get("parentUuid")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let adjusted_parent = match &raw_parent {
            Some(p) if shown_uuids.contains(p) => raw_parent,
            Some(p) => find_shown_ancestor(p, &all_parents, &shown_uuids),
            None => None,
        };

        let has_tools = msg.get("content").map_or(false, |c| {
            c.as_array().map_or(false, |arr| {
                arr.iter()
                    .any(|b| b.get("type").and_then(|v| v.as_str()) == Some("tool_use"))
            })
        });

        let model = msg
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let timestamp = ev
            .get("timestamp")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let pv = preview(msg);
        let ct = get_content_text(msg);

        let idx = all_nodes.len();
        uuid_to_idx.insert(uid.clone(), idx);
        all_nodes.push(TreeNode {
            uuid: uid,
            parent: adjusted_parent,
            role,
            preview: pv,
            content_text: ct,
            timestamp,
            children: Vec::new(),
            model,
            has_tools,
        });
    }

    let mut root_indices = Vec::new();
    for i in 0..all_nodes.len() {
        let parent_uuid = all_nodes[i].parent.clone();
        match parent_uuid {
            Some(ref puuid) => {
                if let Some(&parent_idx) = uuid_to_idx.get(puuid) {
                    all_nodes[parent_idx].children.push(i);
                } else {
                    root_indices.push(i);
                }
            }
            None => root_indices.push(i),
        }
    }

    (all_nodes, uuid_to_idx, root_indices)
}

/// Build a full tree including ALL event types (system, attachment, etc.).
/// Used by `list` so users can see the complete session structure.
pub fn build_full_tree(events: &[Value]) -> (Vec<TreeNode>, HashMap<String, usize>, Vec<usize>) {
    let mut all_parents: HashMap<String, Option<String>> = HashMap::new();
    for ev in events {
        let uid = get_event_uuid(ev);
        if uid.is_empty() {
            continue;
        }
        let parent = ev
            .get("parentUuid")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        all_parents.insert(uid, parent);
    }

    let mut all_nodes: Vec<TreeNode> = Vec::new();
    let mut uuid_to_idx: HashMap<String, usize> = HashMap::new();

    for ev in events {
        let ev_type = match ev.get("type").and_then(|v| v.as_str()) {
            Some(t) => t,
            None => continue,
        };

        let uid = get_event_uuid(ev);
        if uid.is_empty() {
            continue;
        }

        // Build a label for preview
        let pv = match ev_type {
            "user" | "assistant" => {
                if let Some(msg) = ev.get("message").and_then(|m| m.as_object()) {
                    let msg_val = Value::Object(msg.clone());
                    preview(&msg_val)
                } else {
                    ev_type.to_string()
                }
            }
            "system" => "[system]".to_string(),
            "attachment" => "[attachment]".to_string(),
            "permission-mode" => "[permission-mode]".to_string(),
            "last-prompt" => "[last-prompt]".to_string(),
            "queue-operation" => "[queue-operation]".to_string(),
            _ => format!("[{}]", ev_type),
        };

        let parent = ev
            .get("parentUuid")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let role = if ev_type == "user" || ev_type == "assistant" {
            ev.get("message")
                .and_then(|m| m.get("role"))
                .and_then(|v| v.as_str())
                .unwrap_or(ev_type)
                .to_string()
        } else {
            ev_type.to_string()
        };

        let has_tools = ev.get("message").map_or(false, |m| {
            m.get("content").map_or(false, |c| {
                c.as_array().map_or(false, |arr| {
                    arr.iter()
                        .any(|b| b.get("type").and_then(|v| v.as_str()) == Some("tool_use"))
                })
            })
        });

        let model = ev
            .get("message")
            .and_then(|m| m.get("model"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let timestamp = ev
            .get("timestamp")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // Only store content_text for user/assistant
        let ct = if ev_type == "user" || ev_type == "assistant" {
            if let Some(msg) = ev.get("message").and_then(|m| m.as_object()) {
                let msg_val = Value::Object(msg.clone());
                get_content_text(&msg_val)
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        let idx = all_nodes.len();
        uuid_to_idx.insert(uid.clone(), idx);
        all_nodes.push(TreeNode {
            uuid: uid,
            parent,
            role,
            preview: pv,
            content_text: ct,
            timestamp,
            children: Vec::new(),
            model,
            has_tools,
        });
    }

    let mut root_indices = Vec::new();
    for i in 0..all_nodes.len() {
        let parent_uuid = all_nodes[i].parent.clone();
        match parent_uuid {
            Some(ref puuid) => {
                if let Some(&parent_idx) = uuid_to_idx.get(puuid) {
                    all_nodes[parent_idx].children.push(i);
                } else {
                    root_indices.push(i);
                }
            }
            None => root_indices.push(i),
        }
    }

    (all_nodes, uuid_to_idx, root_indices)
}

// ── Navigation ───────────────────────────────────────────────────────

pub fn resolve_ls<'a>(
    all_nodes: &'a [TreeNode],
    root_indices: &'a [usize],
    cwd: &[usize],
) -> (Option<&'a TreeNode>, &'a [usize]) {
    let mut indices = root_indices;
    let mut node = None;

    for &segment in cwd {
        if segment == 0 || segment > indices.len() {
            return (None, &[]);
        }
        let idx = indices[segment - 1];
        let n = &all_nodes[idx];
        indices = &n.children;
        node = Some(n);
    }

    (node, indices)
}

/// Walk up from a node to the root, collecting 1-based child indices.
pub fn tree_path_from_idx(
    all_nodes: &[TreeNode],
    uuid_to_idx: &HashMap<String, usize>,
    mut idx: usize,
) -> Vec<usize> {
    let mut rev = Vec::new();
    loop {
        let node = &all_nodes[idx];
        match node.parent {
            Some(ref puid) => {
                if let Some(&pidx) = uuid_to_idx.get(puid) {
                    let pos = all_nodes[pidx]
                        .children
                        .iter()
                        .position(|&c| c == idx)
                        .unwrap_or(0);
                    rev.push(pos + 1);
                    idx = pidx;
                } else {
                    break;
                }
            }
            None => break,
        }
    }
    rev.reverse();
    rev
}

/// Build a flat list of user text turns with their tree node indices and paths.
pub fn user_turn_list<'a>(
    events: &[Value],
    all_nodes: &'a [TreeNode],
    uuid_to_idx: &HashMap<String, usize>,
) -> Vec<(usize, Vec<usize>, &'a TreeNode)> {
    events
        .iter()
        .filter(|e| is_user_turn(e))
        .filter_map(|ev| {
            let uid = get_event_uuid(ev);
            let idx = *uuid_to_idx.get(&uid)?;
            let path = tree_path_from_idx(all_nodes, uuid_to_idx, idx);
            Some((idx, path, &all_nodes[idx]))
        })
        .collect()
}

// ── Filter helpers ───────────────────────────────────────────────────

pub fn is_user_turn(ev: &Value) -> bool {
    if ev.get("type").and_then(|v| v.as_str()) != Some("user") {
        return false;
    }
    if ev.get("userType").and_then(|v| v.as_str()) == Some("ctx-tool") {
        return false;
    }
    if let Some(content) = ev.get("message").and_then(|m| m.get("content")) {
        if let Some(blocks) = content.as_array() {
            if blocks.iter().any(|b| {
                matches!(
                    b.get("type").and_then(|v| v.as_str()),
                    Some("tool_result" | "result")
                )
            }) {
                return false;
            }
        }
    }
    true
}

pub fn collect_uuids(all_nodes: &[TreeNode], idx: usize, out: &mut HashSet<String>) {
    out.insert(all_nodes[idx].uuid.clone());
    for &child_idx in &all_nodes[idx].children {
        collect_uuids(all_nodes, child_idx, out);
    }
}
