use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use ratatui::{prelude::*, widgets::*};
use serde_json::Value;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::display;
use crate::session;
use crate::tree::{build_full_tree, get_event_uuid, TreeNode};

// ── Public entry points ────────────────────────────────────────────

pub fn run_tui() -> Result<()> {
    let (path, _info) = session::find_session().context("No active session.")?;
    run_tui_with_path(path)
}

pub fn run_tui_with_path(path: PathBuf) -> Result<()> {
    let events = session::load_events(&path)?;
    let (all_nodes, _uuid_to_idx, root_indices) = build_full_tree(&events);

    enable_raw_mode()?;
    io::stdout().execute(EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;

    let mut app = App::new(all_nodes, root_indices, events, path);
    let res = app.run(&mut terminal);

    io::stdout().execute(LeaveAlternateScreen)?;
    disable_raw_mode()?;
    res
}

// ── App state ───────────────────────────────────────────────────────

struct App {
    all_nodes: Vec<TreeNode>,
    root_indices: Vec<usize>,
    events: Vec<Value>,
    session_path: PathBuf,
    expanded: std::collections::HashSet<usize>,
    visible: Vec<LineInfo>,
    selected: usize,
    mode: Mode,
    detail: String,
    detail_idx: usize,
}

#[derive(PartialEq)]
enum Mode {
    Tree,
    Detail,
}

struct LineInfo {
    idx: usize,
    depth: usize,
    has_children: bool,
    open: bool,
}

impl App {
    fn new(
        all_nodes: Vec<TreeNode>,
        root_indices: Vec<usize>,
        events: Vec<Value>,
        session_path: PathBuf,
    ) -> Self {
        let mut app = Self {
            all_nodes,
            root_indices,
            events,
            session_path,
            expanded: std::collections::HashSet::default(),
            visible: Vec::new(),
            selected: 0,
            mode: Mode::Tree,
            detail: String::new(),
            detail_idx: 0,
        };
        app.expand_initial();
        app.build_visible();
        app
    }

    fn expand_initial(&mut self) {
        for &r in &self.root_indices {
            self.expanded.insert(r);
        }
        if let Some(last_idx) = find_last_node(&self.all_nodes, &self.events) {
            let mut path = path_to_root(&self.all_nodes, last_idx);
            if path.len() > 1 {
                path.pop();
            }
            for &n in &path {
                self.expanded.insert(n);
            }
        }
    }

    fn build_visible(&mut self) {
        self.visible.clear();
        for &r in &self.root_indices {
            walk_node(&self.all_nodes, r, 0, &self.expanded, &mut self.visible);
        }
        self.selected = self.selected.min(self.visible.len().saturating_sub(1));
    }

    fn toggle_expand(&mut self, idx: usize) {
        if self.expanded.contains(&idx) {
            self.expanded.remove(&idx);
        } else {
            self.expanded.insert(idx);
        }
        self.build_visible();
    }

    fn show_detail(&mut self, node_idx: usize) {
        self.detail = format_node_detail(&self.all_nodes[node_idx], &self.events);
        self.detail_idx = node_idx;
        self.mode = Mode::Detail;
    }

    fn run(&mut self, terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
        loop {
            terminal.draw(|frame| self.render(frame))?;

            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => match self.mode {
                    Mode::Tree => match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                        KeyCode::Up => {
                            self.selected = self.selected.saturating_sub(1);
                        }
                        KeyCode::Down => {
                            self.selected =
                                (self.selected + 1).min(self.visible.len().saturating_sub(1));
                        }
                        KeyCode::Right | KeyCode::Char(' ') => {
                            let li = &self.visible[self.selected];
                            if li.has_children {
                                self.toggle_expand(li.idx);
                            }
                        }
                        KeyCode::Left => {
                            let li = &self.visible[self.selected];
                            if li.open {
                                self.expanded.remove(&li.idx);
                                self.build_visible();
                            }
                        }
                        KeyCode::Enter => {
                            let li = &self.visible[self.selected];
                            self.show_detail(li.idx);
                        }
                        _ => {}
                    },
                    Mode::Detail => match key.code {
                        KeyCode::Enter => {
                            self.mode = Mode::Tree;
                        }
                        KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                        KeyCode::Char('e') => {
                            if let Err(e) = self.edit_event() {
                                self.detail = format!("Edit error: {e}");
                            }
                        }
                        _ => {}
                    },
                },
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    }

    fn edit_event(&mut self) -> Result<()> {
        let node = &self.all_nodes[self.detail_idx];
        let ev_idx = self.events.iter().position(|e| get_event_uuid(e) == node.uuid);
        let ev_idx = match ev_idx {
            Some(i) => i,
            None => return Ok(()),
        };

        let json = serde_json::to_string_pretty(&self.events[ev_idx])?;

        let tmp = env::temp_dir().join(format!("ctx_event_{}.json", node.uuid));
        fs::write(&tmp, &json)?;

        let editor = env::var("EDITOR").unwrap_or_else(|_| "vim".to_string());
        let status = std::process::Command::new(&editor)
            .arg(&tmp)
            .status()
            .context("Failed to launch editor")?;

        if !status.success() {
            let _ = fs::remove_file(&tmp);
            return Err(anyhow::anyhow!("Editor exited with error"));
        }

        let new_json = fs::read_to_string(&tmp)?;
        let new_ev: Value =
            serde_json::from_str(&new_json).context("Invalid JSON after edit")?;

        let _ = fs::remove_file(&tmp);

        self.events[ev_idx] = new_ev;
        self.save_events()?;

        let (all_nodes, _, root_indices) = build_full_tree(&self.events);
        self.all_nodes = all_nodes;
        self.root_indices = root_indices;
        self.detail_idx = 0;
        self.build_visible();
        self.mode = Mode::Tree;

        Ok(())
    }

    fn save_events(&self) -> Result<()> {
        let mut file = fs::File::create(&self.session_path)?;
        for ev in &self.events {
            writeln!(file, "{}", serde_json::to_string(ev)?)?;
        }
        Ok(())
    }

    fn render(&self, frame: &mut Frame) {
        match self.mode {
            Mode::Tree => self.render_tree(frame, frame.size()),
            Mode::Detail => self.render_detail(frame, frame.size()),
        }
    }

    fn render_tree(&self, frame: &mut Frame, area: Rect) {
        let visible_height = (area.height as usize).saturating_sub(2);
        let scroll = if self.selected > visible_height / 2 {
            let max_scroll = self.visible.len().saturating_sub(visible_height);
            (self.selected - visible_height / 2).min(max_scroll)
        } else {
            0
        };
        let items: Vec<ListItem> = self
            .visible
            .iter()
            .enumerate()
            .skip(scroll)
            .take(visible_height)
            .map(|(i, li)| {
                let node = &self.all_nodes[li.idx];
                const MAX_INDENT_DEPTH: usize = 10;
                let indent = if li.depth > MAX_INDENT_DEPTH {
                    "  ".repeat(MAX_INDENT_DEPTH) + ".. "
                } else {
                    "  ".repeat(li.depth)
                };
                let glyph = if li.has_children {
                    if li.open { "▾ " } else { "▸ " }
                } else {
                    "  "
                };
                let tag = role_tag(node);
                let ts = display::fmt_time(&node.timestamp);
                let preview_raw = if node.preview.is_empty() {
                    &node.content_text
                } else {
                    &node.preview
                };
                let content_width = (area.width as usize).saturating_sub(2);
                let prefix = format!("{indent}{glyph}{tag} {ts} ");
                let prefix_width = prefix.width();
                let max_preview = content_width.saturating_sub(prefix_width).max(10);
                let preview = truncate_width(preview_raw, max_preview);
                let text = format!("{prefix}{preview}");

                let style = if i == self.selected {
                    Style::default().bg(Color::DarkGray).fg(Color::Yellow)
                } else if node.role == "user" {
                    Style::default().fg(Color::Green)
                } else if node.role == "system"
                    || node.role == "attachment"
                    || node.role == "permission-mode"
                {
                    Style::default().fg(Color::DarkGray)
                } else {
                    Style::default()
                };

                ListItem::new(text).style(style)
            })
            .collect();

        let list = List::new(items)
            .block(Block::default().title(" Session Tree ").borders(Borders::ALL))
            .highlight_style(Style::default().add_modifier(Modifier::BOLD));

        frame.render_widget(list, area);
    }

    fn render_detail(&self, frame: &mut Frame, area: Rect) {
        let para = Paragraph::new(self.detail.as_str())
            .block(
                Block::default()
                    .title(" JSON Detail (e: edit, Enter: back, Esc/q: quit) ")
                    .borders(Borders::ALL),
            )
            .wrap(Wrap { trim: false });
        frame.render_widget(para, area);
    }
}

// ── Tree helpers ────────────────────────────────────────────────────

fn walk_node(
    nodes: &[TreeNode],
    idx: usize,
    depth: usize,
    expanded: &std::collections::HashSet<usize>,
    out: &mut Vec<LineInfo>,
) {
    let n = &nodes[idx];
    let open = expanded.contains(&idx);
    out.push(LineInfo {
        idx,
        depth,
        has_children: !n.children.is_empty(),
        open,
    });
    if open {
        for &c in &n.children {
            walk_node(nodes, c, depth + 1, expanded, out);
        }
    }
}

fn path_to_root(nodes: &[TreeNode], mut idx: usize) -> Vec<usize> {
    let mut path = Vec::new();
    loop {
        path.push(idx);
        match nodes[idx].parent {
            Some(ref puid) => {
                if let Some(pidx) = nodes.iter().position(|n| n.uuid == *puid) {
                    idx = pidx;
                } else {
                    break;
                }
            }
            None => break,
        }
    }
    path.reverse();
    path
}

fn find_last_node(nodes: &[TreeNode], events: &[Value]) -> Option<usize> {
    for ev in events.iter().rev() {
        let uid = get_event_uuid(ev);
        if !uid.is_empty() {
            if let Some(idx) = nodes.iter().position(|n| n.uuid == uid) {
                return Some(idx);
            }
        }
    }
    None
}

/// Truncate a string to fit `max_width` columns in the terminal.
fn truncate_width(s: &str, max_width: usize) -> String {
    if s.width() <= max_width {
        return s.replace('\n', " ").replace('\r', "");
    }
    let mut out = String::with_capacity(max_width);
    let mut w = 0usize;
    for ch in s.chars() {
        let cw = ch.width().unwrap_or(0);
        if w + cw > max_width.saturating_sub(3) {
            out.push_str("...");
            break;
        }
        out.push(ch);
        w += cw;
    }
    out.replace('\n', " ").replace('\r', "")
}

fn role_tag(node: &TreeNode) -> &str {
    if node.role == "user" {
        "user"
    } else if node.role == "assistant" {
        "Claude"
    } else {
        &node.role
    }
}

// ── Detail formatting ───────────────────────────────────────────────

fn format_node_detail(node: &TreeNode, events: &[Value]) -> String {
    let uuid = &node.uuid;
    let ev = events.iter().find(|e| get_event_uuid(e) == *uuid);
    match ev {
        Some(ev) => {
            let mut s = String::new();
            s.push_str(&format!(
                "{}  {}  [type: {}]\n",
                role_tag(node),
                display::fmt_time(&node.timestamp),
                ev.get("type").and_then(|v| v.as_str()).unwrap_or("?"),
            ));
            s.push_str(&"─".repeat(40));
            s.push('\n');
            if let Ok(pretty) = serde_json::to_string_pretty(ev) {
                s.push_str(&pretty);
            }
            s
        }
        None => format!("(no event data for {})", node.uuid),
    }
}
