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
    }
    if app.ui_state.show_api_key_prompt {
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
    
    let header = Line::from(vec![
        Span::styled(format!(" session: {} ", app.session_meta.name), Style::default().bg(Color::Blue).fg(Color::White)),
        Span::styled(" │ ", Style::default().fg(Color::DarkGray)),
        Span::styled(format!(" provider: {} ", app.config.provider.as_str()), Style::default().bg(Color::Cyan).fg(Color::Black)),
        Span::styled(" │ ", Style::default().fg(Color::DarkGray)),
        Span::styled(format!(" model: {} ", model_display), Style::default().bg(Color::Green).fg(Color::Black)),
        Span::styled(" │ ", Style::default().fg(Color::DarkGray)),
        Span::styled(format!(" [{}] ", tab_name), Style::default().bg(Color::Yellow).fg(Color::Black)),
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
            if let Some(ports_json) = &node.target.ports_json {
                if let Ok(ports) = serde_json::from_str::<Vec<crate::db::models::PortInfo>>(ports_json) {
                    for port in ports {
                        items.push(ListItem::new(Line::from(vec![
                            Span::styled("  ├─ ", Style::default().fg(Color::DarkGray)),
                            Span::styled(format!("{}/{}: {} {}", port.port, port.proto, port.service, port.version), Style::default().fg(Color::Cyan)),
                        ])));
                    }
                }
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
    let list = List::new(items)
        .block(Block::default().borders(Borders::RIGHT).title(" Targets "))
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
    let approval_height = if app.pending_exploit_approval.is_some() { 4 } else { 0 };
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
        let content = vec![
            Line::from(vec![
                Span::styled(" [EXPLOIT APPROVAL REQUIRED] ", Style::default().bg(Color::Red).fg(Color::White).add_modifier(Modifier::BOLD)),
                Span::raw(" LLM requests reverse shell listener:"),
            ]),
            Line::from(vec![
                Span::styled(format!(" Target: {} (ID {}) │ Interface: {}:{} │ Shell: {}", approval.target_ip, approval.target_id, approval.lhost, approval.lport, approval.shell_type), Style::default().fg(Color::Yellow)),
            ]),
            Line::from(vec![
                Span::styled(" [y] Approve & Spawn Listener ", Style::default().bg(Color::Green).fg(Color::Black).add_modifier(Modifier::BOLD)),
                Span::raw("   "),
                Span::styled(" [n/Esc] Reject ", Style::default().bg(Color::DarkGray).fg(Color::White)),
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

fn render_chat_log(f: &mut Frame, app: &mut App, area: Rect) {
    let mut lines = Vec::new();
    
    for msg in &app.chat_log {
        match msg.role.as_str() {
            "user" => {
                lines.push(Line::from(vec![
                    Span::styled("┌─ ", Style::default().fg(Color::Green)),
                    Span::styled("You ($) ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                    Span::styled("──────────────────────────────────────────────────", Style::default().fg(Color::DarkGray)),
                ]));
                for line in msg.content.lines() {
                    lines.push(Line::from(vec![
                        Span::styled("│ ", Style::default().fg(Color::Green)),
                        Span::styled(line, Style::default().fg(Color::White)),
                    ]));
                }
                lines.push(Line::from(""));
            }
            "output" => {
                for line in msg.content.lines() {
                    lines.push(Line::from(vec![
                        Span::styled("  ", Style::default().fg(Color::DarkGray)),
                        Span::styled(line, Style::default().fg(Color::Gray)),
                    ]));
                }
                lines.push(Line::from(""));
            }
            "assistant" => {
                lines.push(Line::from(vec![
                    Span::styled("┌─ ", Style::default().fg(Color::Cyan)),
                    Span::styled("🤖 AI / LLM ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                    Span::styled(format!("[{}] ", app.config.active_model), Style::default().fg(Color::Yellow)),
                    Span::styled("──────────────────────────────────────────", Style::default().fg(Color::DarkGray)),
                ]));
                for line in msg.content.lines() {
                    lines.push(Line::from(vec![
                        Span::styled("│ ", Style::default().fg(Color::Cyan)),
                        Span::styled(line, Style::default().fg(Color::White)),
                    ]));
                }
                lines.push(Line::from(vec![
                    Span::styled("└────────────────────────────────────────────────────────────", Style::default().fg(Color::DarkGray)),
                ]));
                lines.push(Line::from(""));
            }
            "tool" => {
                lines.push(Line::from(vec![
                    Span::styled("# Tool Result: ", Style::default().fg(Color::Yellow).add_modifier(Modifier::ITALIC)),
                    Span::styled(&msg.content, Style::default().fg(Color::DarkGray)),
                ]));
                lines.push(Line::from(""));
            }
            _ => {
                for line in msg.content.lines() {
                    lines.push(Line::from(vec![
                        Span::styled("! ", Style::default().fg(Color::Red)),
                        Span::styled(line, Style::default().fg(Color::White)),
                    ]));
                }
                lines.push(Line::from(""));
            }
        }
    }
    
    // Add streaming LLM response
    if matches!(app.llm_request_state, crate::llm::types::LlmRequestState::Streaming) && !app.llm_response_buffer.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("┌─ 🤖 AI / LLM [Streaming...] ─────────────────────────────────", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        ]));
        for line in app.llm_response_buffer.lines() {
            lines.push(Line::from(vec![
                Span::styled("│ ", Style::default().fg(Color::Yellow)),
                Span::styled(line, Style::default().fg(Color::White)),
            ]));
        }
        lines.push(Line::from(vec![
            Span::styled("│ ▌", Style::default().fg(Color::Yellow)),
        ]));
        lines.push(Line::from(""));
    }

    let total_lines = lines.len();
    let inner_height = area.height.saturating_sub(2) as usize;
    let max_scroll = total_lines.saturating_sub(inner_height);

    if app.ui_state.log_at_bottom {
        app.ui_state.log_scroll = max_scroll;
    } else {
        app.ui_state.log_scroll = app.ui_state.log_scroll.min(max_scroll);
    }

    let title = if app.ui_state.log_scroll < max_scroll {
        format!(" Workspace & LLM [Scrolled: line {}/{} - PgDn/Ctrl+End to bottom] ", app.ui_state.log_scroll, max_scroll)
    } else {
        " Workspace & LLM ".to_string()
    };
    
    let log = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: false })
        .scroll((app.ui_state.log_scroll as u16, 0));
    
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
    let (mode_indicator, mode_style) = match app.input_mode {
        InputMode::Shell => ("> ", Style::default().fg(Color::Green)),
        InputMode::LlmQuery => ("? ", Style::default().fg(Color::Cyan)),
        InputMode::EditCommand => ("e ", Style::default().fg(Color::Yellow)),
    };
    
    let input = Paragraph::new(Line::from(vec![
        Span::styled(mode_indicator, mode_style),
        Span::raw(&app.input_buffer),
    ]))
    .style(Style::default().fg(Color::White))
    .block(Block::default().borders(Borders::NONE));
    
    f.render_widget(input, area);
    
    // Set cursor position: accurately calculated and bounded
    let cursor_offset = app.ui_state.input_cursor.min(app.input_buffer.len());
    let cursor_x = (area.x + mode_indicator.len() as u16 + cursor_offset as u16)
        .min(area.x + area.width.saturating_sub(1));
    f.set_cursor(cursor_x, area.y);
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
    
    let footer = Line::from(vec![
        Span::styled(" F1/Alt+H/:help:Help ", Style::default().bg(Color::DarkGray).fg(Color::White)),
        Span::styled(" F2:Models ", Style::default().bg(Color::DarkGray).fg(Color::White)),
        Span::styled(" F3:New Shell ", Style::default().bg(Color::DarkGray).fg(Color::White)),
        Span::styled(" Tab:Switch Tab ", Style::default().bg(Color::DarkGray).fg(Color::White)),
        Span::styled(" Esc:Workspace ", Style::default().bg(Color::DarkGray).fg(Color::White)),
        Span::styled(format!(" [{}] ", mode_text), Style::default().bg(Color::Blue).fg(Color::White)),
    ]);
    
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