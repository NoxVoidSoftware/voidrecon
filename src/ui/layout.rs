//! Layout rendering for voidrecon UI

use crate::app::state::{App, InputMode, NotifLevel};
use crate::pty::widget::VtScreenWidget;
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
    let suggestion_height = if app.suggestion_visible { 5 } else { 0 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(10),          // Chat/log area
            Constraint::Length(suggestion_height), // Suggestion panel
            Constraint::Length(1),        // Input line
        ])
        .split(area);
    
    render_chat_log(f, app, chunks[0]);
    
    if app.suggestion_visible {
        render_suggestion_panel(f, app, chunks[1]);
    }
    
    render_input_line(f, app, chunks[2]);
}

fn render_chat_log(f: &mut Frame, app: &mut App, area: Rect) {
    let mut lines = Vec::new();
    
    for msg in &app.chat_log {
        let prefix = match msg.role.as_str() {
            "user" => "$ ".to_string(),
            "assistant" => "> ".to_string(),
            "tool" => "# ".to_string(),
            "system" => "! ".to_string(),
            _ => "  ".to_string(),
        };
        
        let prefix_style = match msg.role.as_str() {
            "user" => Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            "assistant" => Style::default().fg(Color::Blue),
            "tool" => Style::default().fg(Color::Yellow),
            "system" => Style::default().fg(Color::Red),
            _ => Style::default().fg(Color::White),
        };
        
        for line in msg.content.lines() {
            lines.push(Line::from(vec![
                Span::styled(prefix.clone(), prefix_style),
                Span::raw(line),
            ]));
        }
    }
    
    // Add streaming LLM response
    if matches!(app.llm_request_state, crate::llm::types::LlmRequestState::Streaming) && !app.llm_response_buffer.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("▌ ", Style::default().fg(Color::Cyan)),
            Span::raw(&app.llm_response_buffer),
        ]));
    }
    
    let log = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" Workspace & LLM "))
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
    
    // Set cursor position
    f.set_cursor(
        area.x + mode_indicator.len() as u16 + app.ui_state.input_cursor as u16,
        area.y,
    );
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
        Span::styled(" F1/:help:Help ", Style::default().bg(Color::DarkGray).fg(Color::White)),
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
    let mut y_offset = 0;
    for (idx, notif) in app.notifications.iter().enumerate() {
        if idx >= 3 { break; }
        
        let color = match notif.level {
            NotifLevel::Info => Color::Blue,
            NotifLevel::Warning => Color::Yellow,
            NotifLevel::Error => Color::Red,
        };
        
        let area = Rect {
            x: size.width.saturating_sub(60),
            y: size.height.saturating_sub(4 + y_offset),
            width: 58,
            height: 1,
        };
        
        let notif_widget = Paragraph::new(notif.message.clone())
            .style(Style::default().fg(color).bg(Color::Black))
            .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(color)));
        
        f.render_widget(Clear, area);
        f.render_widget(notif_widget, area);
        
        y_offset += 2;
    }
}