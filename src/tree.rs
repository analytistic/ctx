use std::collections::HashMap;

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
