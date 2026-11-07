//! Layout rendering for voidrecon UI

use crate::app::state::{App, InputMode, NotifLevel};
use crate::ui::modals::{render_help_modal, render_model_picker_modal};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};

pub fn render_ui(f: &mut Frame, app: &mut App) {
    let area = f.size();
    app.terminal_size = (area.width, area.height);
    
    // Main layout
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),    // Header
            Constraint::Min(0),       // Main content
            Constraint::Length(1),    // Footer
        ])
        .split(area);
    
    render_header(f, app, chunks[0]);
    render_main(f, app, chunks[1]);
    render_footer(f, app, chunks[2]);
    
    // Overlays
    if app.ui_state.show_help {
        render_help_modal(f, app);
    }
    if app.ui_state.show_model_picker {
        render_model_picker_modal(f, app);
    }
    if app.ui_state.show_session_picker {
        crate::ui::modals::render_session_picker_modal(f, app);
    } else if app.ui_state.show_api_key_prompt {
        crate::ui::modals::render_api_key_modal(f, app);
    }
    
    // Notifications
    render_notifications(f, app);
}

fn render_header(f: &mut Frame, app: &App, area: Rect) {
    let model_display = if app.config.models_cache.iter().any(|m| m.id == app.config.active_model) {
        let model = app.config.models_cache.iter().find(|m| m.id == app.config.active_model).unwrap();
        let free_badge = if model.id.ends_with(":free") || (model.pricing.prompt == "0" && model.pricing.completion == "0") {
            " [FREE]".to_string()
        } else {
            String::new()
        };
        format!("{}{}", model.name, free_badge)
    } else {
        app.config.active_model.clone()
    };
    
    let tab_name = if app.active_tab == 0 {
        "Workspace & LLM"
    } else if let Some(tab) = app.tabs.get(app.active_tab - 1) {
        if tab.kind == crate::pty::tab::TerminalKind::Listener {
            &tab.label
        } else {
            "Shell"
        }
    } else {
        "Shell"
    };
    
    let spinner_frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let spinner = spinner_frames[(app.ui_state.tick_count / 2) % spinner_frames.len()];

    let llm_indicator = if let Some((_, cmd, start)) = &app.running_command {
        let elapsed = start.elapsed().as_secs();
        Span::styled(format!(" [RUNNING {} ({}s): {}] ", spinner, elapsed, cmd), Style::default().bg(Color::Yellow).fg(Color::Black).add_modifier(Modifier::BOLD))
    } else {
        match app.llm_request_state {
            crate::llm::types::LlmRequestState::Streaming => {
                Span::styled(format!(" [LLM {}] ", spinner), Style::default().bg(Color::Yellow).fg(Color::Black).add_modifier(Modifier::BOLD))
            }
            crate::llm::types::LlmRequestState::ProcessingToolCalls => {
                Span::styled(format!(" [Tools {}] ", spinner), Style::default().bg(Color::Cyan).fg(Color::Black).add_modifier(Modifier::BOLD))
            }
            crate::llm::types::LlmRequestState::Completed => {
                Span::styled(" [LLM: Ready] ", Style::default().bg(Color::Green).fg(Color::Black))
            }
            crate::llm::types::LlmRequestState::Failed => {
                Span::styled(" [LLM: Error] ", Style::default().bg(Color::Red).fg(Color::White))
            }
            crate::llm::types::LlmRequestState::Idle => {
                Span::styled(" [LLM] ", Style::default().bg(Color::DarkGray).fg(Color::White))
            }
        }
    };

    let header = Line::from(vec![
        Span::styled(format!(" session: {} ", app.session_meta.name), Style::default().bg(Color::Blue).fg(Color::White)),
        Span::styled(" │ ", Style::default().fg(Color::DarkGray)),
        Span::styled(format!(" provider: {} ", app.config.provider.as_str()), Style::default().bg(Color::Cyan).fg(Color::Black)),
        Span::styled(" │ ", Style::default().fg(Color::DarkGray)),
        Span::styled(format!(" model: {} ", model_display), Style::default().bg(Color::Green).fg(Color::Black)),
        Span::styled(" │ ", Style::default().fg(Color::DarkGray)),
        Span::styled(format!(" [{}] ", tab_name), Style::default().bg(Color::Yellow).fg(Color::Black)),
        Span::styled(" │ ", Style::default().fg(Color::DarkGray)),
        llm_indicator,
    ]);
    
    f.render_widget(Paragraph::new(header), area);
}

fn render_main(f: &mut Frame, app: &mut App, area: Rect) {
    if app.ui_state.sidebar_collapsed {
        render_main_full(f, app, area);
    } else {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(40), Constraint::Min(0)])
            .split(area);
        
        render_sidebar(f, app, chunks[0]);
        render_main_full(f, app, chunks[1]);
    }
}

fn render_sidebar(f: &mut Frame, app: &mut App, area: Rect) {
    let mut items = Vec::new();
    
    for node in &app.sidebar_nodes {
        let is_active = app.active_target_id == Some(node.target.id);
        let style = if is_active {
            Style::default().bg(Color::Blue).fg(Color::White).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        
        let expander = if node.expanded { "▼ " } else { "▶ " };
        items.push(ListItem::new(Line::from(vec![
            Span::styled(expander, Style::default().fg(Color::Yellow)),
            Span::styled(&node.target.ip_host, style),
        ])));
        
        if node.expanded {
            // Ports
            for port in &node.ports {
                items.push(ListItem::new(Line::from(vec![
                    Span::styled("  ├─ ", Style::default().fg(Color::DarkGray)),
                    Span::styled(format!("{}/{}: {} {}", port.port, port.proto, port.service, port.version), Style::default().fg(Color::Cyan)),
                ])));
            }
            
            // Findings by severity
            for (severity, findings) in &node.findings_by_severity {
                let severity_color = match severity.as_str() {
                    "critical" => Color::Red,
                    "high" => Color::LightRed,
                    "medium" => Color::Yellow,
                    "low" => Color::Green,
                    "info" => Color::Blue,
                    _ => Color::White,
                };
                
                items.push(ListItem::new(Line::from(vec![
                    Span::styled("  ├─ ", Style::default().fg(Color::DarkGray)),
                    Span::styled(format!("{} ({})", severity.to_uppercase(), findings.len()), Style::default().fg(severity_color).add_modifier(Modifier::BOLD)),
                ])));
                
                for finding in findings {
                    items.push(ListItem::new(Line::from(vec![
                        Span::styled("  │  ├─ ", Style::default().fg(Color::DarkGray)),
                        Span::styled(&finding.title, Style::default().fg(severity_color)),
                        Span::styled(format!(" — {}", finding.description), Style::default().fg(Color::DarkGray)),
                    ])));
                }
            }
        }
    }
    
    let items_len = items.len();
    let border_color = if app.ui_state.focus == crate::app::state::WindowFocus::Sidebar {
        Color::Cyan
    } else {
        Color::DarkGray
    };
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::RIGHT)
                .border_style(Style::default().fg(border_color))
                .title(" Targets "),
        )
        .highlight_style(Style::default().bg(Color::Blue).fg(Color::White).add_modifier(Modifier::BOLD));
    
    let mut state = ListState::default();
    state.select(Some(app.ui_state.sidebar_selected.min(items_len.saturating_sub(1))));
    
    f.render_stateful_widget(list, area, &mut state);
}

fn render_main_full(f: &mut Frame, app: &mut App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Tab bar
            Constraint::Min(0),    // Active tab viewport
        ])
        .split(area);

    render_tab_bar(f, app, chunks[0]);

    if app.active_tab == 0 {
        render_workspace_tab(f, app, chunks[1]);
    } else {
        render_shell_tab(f, app, chunks[1]);
    }
}

fn render_tab_bar(f: &mut Frame, app: &App, area: Rect) {
    let mut spans = Vec::new();
    let total = 1 + app.tabs.len();
    for i in 0..total {
        let label = if i == 0 {
            "Tab 0: Workspace & LLM".to_string()
        } else {
            format!("Tab {}: {}", i, app.tabs[i - 1].label)
        };
        let is_active = i == app.active_tab;
        let style = if is_active {
            Style::default().bg(Color::Cyan).fg(Color::Black).add_modifier(Modifier::BOLD)
        } else {
            Style::default().bg(Color::DarkGray).fg(Color::White)
        };
        spans.push(Span::styled(format!(" [{}] ", label), style));
        spans.push(Span::raw(" "));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_workspace_tab(f: &mut Frame, app: &mut App, area: Rect) {
    let approval_height = if app.pending_exploit_approval.is_some() { 5 } else { 0 };
    let suggestion_height = if app.suggestion_visible { 5 } else { 0 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(8),                    // Chat/log area
            Constraint::Length(approval_height),   // Exploit approval panel
            Constraint::Length(suggestion_height), // Suggestion panel
            Constraint::Length(1),                 // Input line
        ])
        .split(area);
    
    render_chat_log(f, app, chunks[0]);
    
    if let Some(approval) = &app.pending_exploit_approval {
        let (prompt_title, prompt_detail, approve_label) = match &approval.kind {
            crate::app::state::ApprovalKind::ReverseShellListener { lhost, lport, shell_type } => (
                " LLM requests reverse shell listener:",
                format!(" Target: {} (ID {}) │ Interface: {}:{} │ Shell: {}", approval.target_ip, approval.target_id, lhost, lport, shell_type),
                " [y / Enter] Approve & Spawn Listener ",
            ),
            crate::app::state::ApprovalKind::ExecuteExploit { script_name, command_line } => (
                " LLM requests exploit script execution:",
                format!(" Target: {} (ID {}) │ Script: {} │ Command: {}", approval.target_ip, approval.target_id, script_name, command_line),
                " [y / Enter] Approve & Execute Exploit ",
            ),
            crate::app::state::ApprovalKind::SaveWorkspaceFile { folder, filename, description, .. } => (
                " LLM requests saving workspace file:",
                format!(" Target: {} (ID {}) │ File: workspace/{}/{} │ Desc: {}", approval.target_ip, approval.target_id, folder, filename, description),
                " [y / Enter] Approve & Save File ",
            ),
        };
        let content = vec![
            Line::from(vec![
                Span::styled(" [EXPLOIT APPROVAL REQUIRED] ", Style::default().bg(Color::Red).fg(Color::White).add_modifier(Modifier::BOLD)),
                Span::raw(prompt_title),
            ]),
            Line::from(vec![
                Span::styled(prompt_detail, Style::default().fg(Color::Yellow)),
            ]),
            Line::from(vec![
                Span::styled(approve_label, Style::default().bg(Color::Green).fg(Color::Black).add_modifier(Modifier::BOLD)),
                Span::raw("   "),
                Span::styled(" [n / Esc] Reject ", Style::default().bg(Color::DarkGray).fg(Color::White)),
            ]),
        ];
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Red))
            .title(" Action Approval ");
        f.render_widget(Paragraph::new(content).block(block), chunks[1]);
    }

    if app.suggestion_visible {
        render_suggestion_panel(f, app, chunks[2]);
    }
    
    render_input_line(f, app, chunks[3]);
}

pub fn strip_ansi_escapes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.next() {
                Some('[') => {
                    for next_c in chars.by_ref() {
                        if (0x40..=0x7E).contains(&(next_c as u32)) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(next_c) = chars.next() {
                        if next_c == '\x07' {
                            break;
                        }
                        if next_c == '\x1b' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                Some('(') | Some(')') => {
                    chars.next();
                }
                Some(other) => {
                    if !(0x40..=0x7E).contains(&(other as u32)) {
                        // ignore
                    }
                }
                None => {}
            }
        } else if c == '\t' {
            out.push_str("    ");
        } else if c == '\r' || c == '\n' || !c.is_control() {
            out.push(c);
        }
    }
    out
}

pub fn resolve_carriage_returns(text: &str) -> String {
    let mut result = Vec::new();
    for raw_line in text.split('\n') {
        let line = raw_line.trim_end_matches('\r');
        if !line.contains('\r') {
            result.push(line.to_string());
            continue;
        }

        let mut char_buf: Vec<char> = Vec::new();
        let mut cursor = 0;
        for c in line.chars() {
            if c == '\r' {
                cursor = 0;
            } else {
                if cursor < char_buf.len() {
                    char_buf[cursor] = c;
                } else {
                    char_buf.push(c);
                }
                cursor += 1;
            }
        }
        let resolved: String = char_buf.into_iter().collect();
        result.push(resolved);
    }
    result.join("\n")
}

pub fn sanitize_terminal_output(raw: &str) -> String {
    let stripped = strip_ansi_escapes(raw);
    resolve_carriage_returns(&stripped)
}

fn push_chunked_line(
    lines_out: &mut Vec<Line<'static>>,
    prefix: &str,
    prefix_style: Style,
    line: &str,
    text_style: Style,
) {
    if line.len() <= 200 {
        lines_out.push(Line::from(vec![
            Span::styled(prefix.to_string(), prefix_style),
            Span::styled(line.to_string(), text_style),
        ]));
        return;
    }

    let mut chunk_start = 0;
    let mut chunks_shown = 0;
    while chunk_start < line.len() {
        if chunks_shown >= 4 {
            lines_out.push(Line::from(vec![
                Span::styled(prefix.to_string(), prefix_style),
                Span::styled(
                    format!("... [long line truncated, total {} chars] ...", line.len()),
                    Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                ),
            ]));
            break;
        }

        let mut chunk_end = (chunk_start + 200).min(line.len());
        while chunk_end > chunk_start && !line.is_char_boundary(chunk_end) {
            chunk_end -= 1;
        }
        if chunk_end == chunk_start {
            break;
        }

        let piece = &line[chunk_start..chunk_end];
        let p = if chunks_shown == 0 { prefix } else { "  " };
        lines_out.push(Line::from(vec![
            Span::styled(p.to_string(), prefix_style),
            Span::styled(piece.to_string(), text_style),
        ]));

        chunk_start = chunk_end;
        chunks_shown += 1;
    }
}

pub fn format_chat_message(msg: &crate::llm::ChatMessage, active_model: &str) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let sanitized_content = sanitize_terminal_output(&msg.content);
    match msg.role.as_str() {
        "user" => {
            let is_llm = sanitized_content.starts_with('?');
            let header = if is_llm { "You (LLM Query) " } else { "You ($) " };
            let color = if is_llm { Color::Cyan } else { Color::Green };
            lines.push(Line::from(vec![
                Span::styled("┌─ ", Style::default().fg(color)),
                Span::styled(header, Style::default().fg(color).add_modifier(Modifier::BOLD)),
                Span::styled("──────────────────────────────────────────────────", Style::default().fg(Color::DarkGray)),
            ]));
            for line in sanitized_content.lines() {
                push_chunked_line(&mut lines, "│ ", Style::default().fg(color), line, Style::default().fg(Color::White));
            }
            lines.push(Line::from(""));
        }
        "output" => {
            let all: Vec<&str> = sanitized_content.lines().collect();
            let total = all.len();
            let max_lines = 100;
            let show_count = total.min(max_lines);
            for &line in &all[..show_count] {
                push_chunked_line(&mut lines, "  ", Style::default().fg(Color::DarkGray), line, Style::default().fg(Color::Gray));
            }
            if total > max_lines {
                lines.push(Line::from(vec![
                    Span::styled("  ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        format!("... [truncated {} lines; full output stored in workspace/outputs/] ...", total - max_lines),
                        Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                    ),
                ]));
            }
            lines.push(Line::from(""));
        }
        "assistant" => {
            lines.push(Line::from(vec![
                Span::styled("┌─ ", Style::default().fg(Color::Cyan)),
                Span::styled("🤖 AI / LLM ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                Span::styled(format!("[{}] ", active_model), Style::default().fg(Color::Yellow)),
                Span::styled("──────────────────────────────────────────", Style::default().fg(Color::DarkGray)),
            ]));
            for line in sanitized_content.lines() {
                push_chunked_line(&mut lines, "│ ", Style::default().fg(Color::Cyan), line, Style::default().fg(Color::White));
            }
            lines.push(Line::from(vec![
                Span::styled("└────────────────────────────────────────────────────────────", Style::default().fg(Color::DarkGray)),
            ]));
            lines.push(Line::from(""));
        }
        "tool" => {
            let all: Vec<&str> = sanitized_content.lines().collect();
            let total = all.len();
            let max_lines = 50;
            let show_count = total.min(max_lines);
            let mut first = true;
            for &line in &all[..show_count] {
                if first {
                    push_chunked_line(&mut lines, "# Tool Result: ", Style::default().fg(Color::Yellow).add_modifier(Modifier::ITALIC), line, Style::default().fg(Color::DarkGray));
                    first = false;
                } else {
                    push_chunked_line(&mut lines, "  ", Style::default().fg(Color::DarkGray), line, Style::default().fg(Color::DarkGray));
                }
            }
            if first {
                lines.push(Line::from(vec![
                    Span::styled("# Tool Result: ", Style::default().fg(Color::Yellow).add_modifier(Modifier::ITALIC)),
                    Span::styled("(empty)".to_string(), Style::default().fg(Color::DarkGray)),
                ]));
            }
            if total > max_lines {
                lines.push(Line::from(vec![
                    Span::styled("  ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        format!("... [tool result truncated {} lines] ...", total - max_lines),
                        Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                    ),
                ]));
            }
            lines.push(Line::from(""));
        }
        _ => {
            lines.push(Line::from(vec![
                Span::styled("┌─ ⚠️ Notice / System ──────────────────────────────────────────", Style::default().fg(Color::Red)),
            ]));
            for line in sanitized_content.lines() {
                push_chunked_line(&mut lines, "│ ", Style::default().fg(Color::Red), line, Style::default().fg(Color::White));
            }
            lines.push(Line::from(""));
        }
    }
    lines
}

fn calculate_line_wrapped_rows(line_text: &str, max_width: usize) -> usize {
    if max_width == 0 {
        return 1;
    }
    use unicode_width::UnicodeWidthStr;
    let total_width = line_text.width();
    if total_width == 0 {
        return 1;
    }
    if total_width <= max_width {
        return 1;
    }

    let mut rows = 1;
    let mut current_row_width = 0;

    for word in line_text.split(' ') {
        let word_width = word.width();
        if word_width == 0 {
            // space
            if current_row_width + 1 <= max_width {
                current_row_width += 1;
            } else {
                rows += 1;
                current_row_width = 1;
            }
            continue;
        }

        if current_row_width == 0 {
            if word_width <= max_width {
                current_row_width = word_width;
            } else {
                let extra_rows = (word_width.saturating_sub(1)) / max_width;
                rows += extra_rows;
                current_row_width = word_width % max_width;
                if current_row_width == 0 {
                    current_row_width = max_width;
                }
            }
        } else if current_row_width + 1 + word_width <= max_width {
            current_row_width += 1 + word_width;
        } else {
            rows += 1;
            if word_width <= max_width {
                current_row_width = word_width;
            } else {
                let extra_rows = (word_width.saturating_sub(1)) / max_width;
                rows += extra_rows;
                current_row_width = word_width % max_width;
                if current_row_width == 0 {
                    current_row_width = max_width;
                }
            }
        }
    }

    rows
}

fn render_chat_log(f: &mut Frame, app: &mut App, area: Rect) {
    let inner_width = area.width.saturating_sub(2) as usize;
    let inner_height = area.height.saturating_sub(2) as usize;

    let estimate_visual_rows = |lines_slice: &[Line]| -> usize {
        if inner_width == 0 {
            return lines_slice.len();
        }
        lines_slice
            .iter()
            .map(|l| {
                let line_text: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
                calculate_line_wrapped_rows(&line_text, inner_width)
            })
            .sum()
    };

    let mut display_lines = app.cached_chat_lines.clone();
    
    // Add streaming LLM response
    if matches!(app.llm_request_state, crate::llm::types::LlmRequestState::Streaming | crate::llm::types::LlmRequestState::ProcessingToolCalls) {
        let spinner_frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        let spinner = spinner_frames[(app.ui_state.tick_count / 2) % spinner_frames.len()];

        let title = if app.llm_request_state == crate::llm::types::LlmRequestState::ProcessingToolCalls {
            format!("┌─ 🤖 AI / LLM [Executing Tools... {}] (Ctrl+C: Cancel) ────────", spinner)
        } else {
            format!("┌─ 🤖 AI / LLM [Thinking / Streaming... {}] (Ctrl+C: Cancel) ───", spinner)
        };
        display_lines.push(Line::from(vec![
            Span::styled(title, Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        ]));
        if app.llm_response_buffer.is_empty() {
            display_lines.push(Line::from(vec![
                Span::styled("│ ", Style::default().fg(Color::Yellow)),
                Span::styled(format!("Connecting to provider & generating response... {}", spinner), Style::default().fg(Color::Yellow)),
            ]));
        } else {
            for line in app.llm_response_buffer.lines() {
                display_lines.push(Line::from(vec![
                    Span::styled("│ ", Style::default().fg(Color::Yellow)),
                    Span::styled(line.to_string(), Style::default().fg(Color::White)),
                ]));
            }
            display_lines.push(Line::from(vec![
                Span::styled(format!("│ {}", spinner), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            ]));
        }
        display_lines.push(Line::from(""));
    }

    if let Some((_, cmd, start)) = &app.running_command {
        let elapsed = start.elapsed().as_secs();
        let spinner_frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        let spinner = spinner_frames[(app.ui_state.tick_count / 2) % spinner_frames.len()];
        display_lines.push(Line::from(vec![
            Span::styled(format!(" {} Executing: {} ({}s)...", spinner, cmd, elapsed), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        ]));
        display_lines.push(Line::from(""));
    }

    let total_visual_rows = estimate_visual_rows(&display_lines);
    let max_scroll = total_visual_rows.saturating_sub(inner_height);

    let (scroll_offset, title) = if app.ui_state.log_at_bottom {
        app.ui_state.log_scroll = max_scroll;
        (max_scroll, " Workspace & LLM ".to_string())
    } else {
        if app.ui_state.log_scroll >= max_scroll {
            app.ui_state.log_scroll = max_scroll;
            app.ui_state.log_at_bottom = true;
            (max_scroll, " Workspace & LLM ".to_string())
        } else {
            let scroll = app.ui_state.log_scroll;
            let title = format!(
                " Workspace & LLM [Scrolled: line {}/{} - PgDn/Ctrl+End to bottom] ",
                scroll, max_scroll
            );
            (scroll, title)
        }
    };

    let (border_color, focus_badge) = match app.ui_state.focus {
        crate::app::state::WindowFocus::ChatLog => (Color::Cyan, " [FOCUSED - Scroll with Wheel/Keys] "),
        _ => (Color::DarkGray, ""),
    };
    let title = format!("{}{}", title, focus_badge);

    let log = Paragraph::new(display_lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border_color))
                .title(title),
        )
        .wrap(Wrap { trim: false })
        .scroll((scroll_offset as u16, 0));

    f.render_widget(log, area);
}

fn render_suggestion_panel(f: &mut Frame, app: &App, area: Rect) {
    let suggestion = match &app.suggestion {
        Some(s) => s,
        None => return,
    };
    
    let command = suggestion.command.as_deref().unwrap_or("");
    let explanation = &suggestion.explanation;
    
    let content = vec![
        Line::from(vec![
            Span::styled("$ ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            Span::raw(command),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(explanation, Style::default().fg(Color::Gray).add_modifier(Modifier::ITALIC)),
        ]),
    ];
    
    let panel = Paragraph::new(content)
        .block(Block::default().borders(Borders::ALL).title(" LLM Suggestion "))
        .style(Style::default().fg(Color::White));
    
    f.render_widget(panel, area);
    
    // Render action hints
    let hints = Line::from(vec![
        Span::styled(" [r]un ", Style::default().bg(Color::Green).fg(Color::Black)),
        Span::styled(" [e]dit ", Style::default().bg(Color::Yellow).fg(Color::Black)),
        Span::styled(" [s]kip ", Style::default().bg(Color::Red).fg(Color::White)),
    ]);
    let hints_area = Rect {
        x: area.x,
        y: area.y + area.height.saturating_sub(1),
        width: area.width,
        height: 1,
    };
    f.render_widget(Paragraph::new(hints), hints_area);
}

fn render_input_line(f: &mut Frame, app: &App, area: Rect) {
    if app.pending_exploit_approval.is_some() {
        let line = Line::from(vec![
            Span::styled(" [ACTION APPROVAL] ", Style::default().bg(Color::Red).fg(Color::White).add_modifier(Modifier::BOLD)),
            Span::styled(" Press ", Style::default().fg(Color::White)),
            Span::styled(" [Y / Enter] ", Style::default().bg(Color::Green).fg(Color::Black).add_modifier(Modifier::BOLD)),
            Span::styled(" to Approve action or ", Style::default().fg(Color::White)),
            Span::styled(" [N / Esc] ", Style::default().bg(Color::DarkGray).fg(Color::White).add_modifier(Modifier::BOLD)),
            Span::styled(" to Reject", Style::default().fg(Color::White)),
        ]);
        f.render_widget(Paragraph::new(line), area);
        return;
    }

    let (mode_indicator, mode_style) = match app.input_mode {
        InputMode::Shell => {
            if app.ui_state.focus == crate::app::state::WindowFocus::Input {
                ("> ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
            } else {
                ("> ", Style::default().fg(Color::DarkGray))
            }
        }
        InputMode::LlmQuery => (
            "? ",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        InputMode::EditCommand => (
            "e ",
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ),
    };
    
    let input = Paragraph::new(Line::from(vec![
        Span::styled(mode_indicator, mode_style),
        Span::raw(&app.input_buffer),
    ]))
    .style(Style::default().fg(Color::White))
    .block(Block::default().borders(Borders::NONE));
    
    f.render_widget(input, area);
    
    // Set cursor position: accurately calculated and bounded when input is focused
    if app.ui_state.focus == crate::app::state::WindowFocus::Input {
        let cursor_offset = app.ui_state.input_cursor.min(app.input_buffer.len());
        let cursor_x = (area.x + mode_indicator.len() as u16 + cursor_offset as u16)
            .min(area.x + area.width.saturating_sub(1));
        f.set_cursor(cursor_x, area.y);
    }
}

fn render_shell_tab(f: &mut Frame, app: &mut App, area: Rect) {
    if app.active_tab > 0 && app.active_tab - 1 < app.tabs.len() {
        let tab = &app.tabs[app.active_tab - 1];
        let widget = crate::ui::widgets::VtScreenWidget { parser: &tab.parser };
        f.render_widget(widget, area);
    } else {
        let msg = Paragraph::new("No active shell tab. Press F3 to create a new shell tab.")
            .block(Block::default().borders(Borders::ALL).title(" Shell "));
        f.render_widget(msg, area);
    }
}

fn render_footer(f: &mut Frame, app: &App, area: Rect) {
    let mode_text = match app.input_mode {
        InputMode::Shell => "SHELL",
        InputMode::LlmQuery => "LLM QUERY",
        InputMode::EditCommand => "EDIT CMD",
    };
    
    let mut footer_spans = vec![
        Span::styled(" F1/Alt+H/:help:Help ", Style::default().bg(Color::DarkGray).fg(Color::White)),
        Span::styled(" F2:Models ", Style::default().bg(Color::DarkGray).fg(Color::White)),
        Span::styled(" F3:New Shell ", Style::default().bg(Color::DarkGray).fg(Color::White)),
        Span::styled(" Tab:Switch Tab ", Style::default().bg(Color::DarkGray).fg(Color::White)),
        Span::styled(" Esc:Workspace ", Style::default().bg(Color::DarkGray).fg(Color::White)),
    ];
    if app.pending_exploit_approval.is_some() {
        footer_spans.push(Span::styled(" [Y: Approve | N: Reject] ", Style::default().bg(Color::Red).fg(Color::White).add_modifier(Modifier::BOLD)));
    }
    if matches!(app.llm_request_state, crate::llm::types::LlmRequestState::Streaming | crate::llm::types::LlmRequestState::ProcessingToolCalls) {
        footer_spans.push(Span::styled(" Ctrl+C: Cancel LLM ", Style::default().bg(Color::Red).fg(Color::White).add_modifier(Modifier::BOLD)));
    }
    footer_spans.push(Span::styled(format!(" [{}] ", mode_text), Style::default().bg(Color::Blue).fg(Color::White)));

    let footer = Line::from(footer_spans);
    
    f.render_widget(Paragraph::new(footer).alignment(Alignment::Center), area);
}

fn render_notifications(f: &mut Frame, app: &App) {
    let size = f.size();
    if size.width < 30 || size.height < 8 {
        return;
    }
    let notif_width = 56.min(size.width.saturating_sub(4));
    let mut y_offset: u16 = 0;
    
    for (idx, notif) in app.notifications.iter().enumerate() {
        if idx >= 3 { break; }
        
        let color = match notif.level {
            NotifLevel::Info => Color::Cyan,
            NotifLevel::Warning => Color::Yellow,
            NotifLevel::Error => Color::Red,
        };

        let tag = match notif.level {
            NotifLevel::Info => " [Info] ",
            NotifLevel::Warning => " [Notice] ",
            NotifLevel::Error => " [Error] ",
        };

        let card_height: u16 = 3;
        if size.height <= 3 + y_offset + card_height {
            break;
        }

        let area = Rect {
            x: size.width.saturating_sub(notif_width + 2),
            y: size.height.saturating_sub(3 + y_offset + card_height),
            width: notif_width,
            height: card_height,
        };
        
        let notif_widget = Paragraph::new(format!(" {}", notif.message))
            .style(Style::default().fg(Color::White).bg(Color::Black))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(color))
                    .title(Span::styled(tag, Style::default().fg(color).add_modifier(Modifier::BOLD)))
            );
        
        f.render_widget(Clear, area);
        f.render_widget(notif_widget, area);
        
        y_offset += card_height;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_terminal_output_carriage_return() {
        let raw = " 0     0    0\r53 1.25M   53\r100 1.25M  100";
        let cleaned = sanitize_terminal_output(raw);
        assert_eq!(cleaned, "100 1.25M  100");
    }

    #[test]
    fn test_sanitize_terminal_output_ansi_and_tabs() {
        let raw = "\x1b[31mError:\x1b[0m\tCheck failed\r\nDone";
        let cleaned = sanitize_terminal_output(raw);
        assert_eq!(cleaned, "Error:    Check failed\nDone");
    }
}