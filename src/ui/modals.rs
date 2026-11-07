//! Modal dialogs for voidrecon UI

use crate::app::state::App;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Row, Table, TableState, Wrap},
    Frame,
};

pub fn render_help_modal(f: &mut Frame, _app: &App) {
    let area = centered_rect(60, 70, f.size());
    f.render_widget(Clear, area);
    
    let help_text = vec![
        Line::from(vec![
            Span::styled(" voidrecon Help ", Style::default().fg(Color::White).bg(Color::Blue).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(" Global Keys:", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        ]),
        Line::from("   F1 / Ctrl+H - Toggle this help (or type :help)"),
        Line::from("   F2          - Open model picker (or type :models)"),
        Line::from("   F3          - New shell tab (or type :newtab)"),
        Line::from("   Tab         - Cycle open tabs"),
        Line::from("   Esc         - Return to Workspace (Tab 0) / dismiss"),
        Line::from("   Ctrl+L      - Enter LLM query mode"),
        Line::from("   Ctrl+Q      - Quit (or type :quit)"),
        Line::from("   [           - Toggle sidebar"),
        Line::from(""),
        Line::from(vec![
            Span::styled(" Special Commands (in Shell mode):", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        ]),
        Line::from("   :targets    - List all targets"),
        Line::from("   :findings   - Show findings for active target"),
        Line::from("   :findings N - Show findings for target N"),
        Line::from("   :note <txt> - Add a note (info finding)"),
        Line::from("   :models     - Open model picker"),
        Line::from("   :newtab     - New shell tab"),
        Line::from("   :closeshell - Close current tab"),
        Line::from("   :exploit    - Enter exploit mode for active target"),
        Line::from("   :quit       - Quit application"),
        Line::from(""),
        Line::from(vec![
            Span::styled(" LLM Query Mode:", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        ]),
        Line::from("   Type ? followed by your question"),
        Line::from("   Or press Ctrl+L from anywhere"),
        Line::from("   Press Enter to send, Esc to cancel"),
        Line::from(""),
        Line::from(vec![
            Span::styled(" Suggestion Panel:", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        ]),
        Line::from("   [r] - Run suggested command"),
        Line::from("   [e] - Edit suggested command"),
        Line::from("   [s] - Skip/dismiss suggestion"),
        Line::from(""),
        Line::from(vec![
            Span::styled(" Scrolling:", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        ]),
        Line::from("   ↑/↓        - Scroll by line"),
        Line::from("   PgUp/PgDn  - Scroll by page"),
        Line::from(""),
        Line::from(vec![
            Span::styled(" Exploit Mode:", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        ]),
        Line::from("   ?give me a shell on <target>"),
        Line::from("   Opens reverse shell listener tab"),
        Line::from("   Model guides post-exploitation step by step"),
    ];
    
    let paragraph = Paragraph::new(help_text)
        .block(Block::default().borders(Borders::ALL).title(" Help "))
        .wrap(Wrap { trim: true });
    
    f.render_widget(paragraph, area);
}

pub fn render_api_key_modal(f: &mut Frame, app: &App) {
    let area = centered_rect(65, 35, f.size());
    f.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" OpenRouter API Key Setup (voidrecon) ");

    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // Intro
            Constraint::Length(3), // Input box
            Constraint::Length(2), // Hints
        ])
        .split(inner);

    let intro = Paragraph::new(vec![
        Line::from(vec![
            Span::styled("No OpenRouter API key configured in ~/.audit-tui/config.json", Style::default().fg(Color::Yellow)),
        ]),
        Line::from("Enter your API key to enable LLM assistance:"),
    ]);
    f.render_widget(intro, chunks[0]);

    let input_text = format!(" Key: {}█", app.ui_state.api_key_input);
    let input = Paragraph::new(input_text)
        .style(Style::default().fg(Color::White).bg(Color::DarkGray))
        .block(Block::default().borders(Borders::ALL).title(" API Key "));
    f.render_widget(input, chunks[1]);

    let footer = Paragraph::new(Line::from(vec![
        Span::styled(" [Enter] ", Style::default().bg(Color::Green).fg(Color::Black).add_modifier(Modifier::BOLD)),
        Span::raw(" Save & Continue    "),
        Span::styled(" [Esc] ", Style::default().bg(Color::DarkGray).fg(Color::White)),
        Span::raw(" Skip (LLM disabled)"),
    ]));
    f.render_widget(footer, chunks[2]);
}

pub fn render_model_picker_modal(f: &mut Frame, app: &mut App) {
    let area = centered_rect(80, 80, f.size());
    f.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Model Picker (F2/Esc: Close | Enter: Select | Tab: Toggle Free) ");
    let inner_area = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Search bar
            Constraint::Length(1), // Divider
            Constraint::Min(0),    // Models table
        ])
        .split(inner_area);
    
    let models: Vec<&crate::config::ModelInfo> = if app.ui_state.model_picker_free_only {
        app.config.get_free_models()
    } else {
        app.config.search_models(&app.ui_state.model_picker_search)
    };
    
    // Search input at top
    let free_indicator = if app.ui_state.model_picker_free_only {
        " [FREE ONLY: ON]"
    } else {
        " [FREE ONLY: OFF (Tab to toggle)]"
    };
    let search_line = Line::from(vec![
        Span::styled("Search: ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        Span::styled(&app.ui_state.model_picker_search, Style::default().fg(Color::White)),
        Span::styled("█", Style::default().fg(Color::Yellow)),
        Span::styled(
            free_indicator,
            if app.ui_state.model_picker_free_only {
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            },
        ),
    ]);
    f.render_widget(Paragraph::new(search_line), chunks[0]);

    // Separator line
    f.render_widget(Block::default().borders(Borders::BOTTOM).border_style(Style::default().fg(Color::DarkGray)), chunks[1]);
    
    let mut rows = Vec::new();
    for (idx, model) in models.iter().enumerate() {
        let is_selected = idx == app.ui_state.model_picker_selected;
        let is_active = model.id == app.config.active_model;
        
        let price = if model.id.ends_with(":free") || (model.pricing.prompt == "0" && model.pricing.completion == "0") {
            "FREE".to_string()
        } else {
            format!("${}/${}", model.pricing.prompt, model.pricing.completion)
        };
        
        let ctx_len = model.context_length.map(|c| c.to_string()).unwrap_or_else(|| "-".to_string());
        
        let prefix = if is_active { "* " } else { "  " };
        let name_display = format!("{}{}", prefix, model.name);

        let style = if is_selected {
            Style::default().bg(Color::Blue).fg(Color::White).add_modifier(Modifier::BOLD)
        } else if is_active {
            Style::default().fg(Color::Green)
        } else {
            Style::default()
        };
        
        rows.push(Row::new(vec![
            name_display,
            ctx_len,
            price,
        ]).style(style));
    }
    
    let table = Table::new(rows, [
        Constraint::Percentage(50),
        Constraint::Percentage(20),
        Constraint::Percentage(30),
    ])
    .header(Row::new(vec!["Model Name", "Context", "Price"]).style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)))
    .highlight_style(Style::default().bg(Color::Blue).fg(Color::White).add_modifier(Modifier::BOLD));
    
    let mut state = TableState::default();
    if !models.is_empty() {
        state.select(Some(app.ui_state.model_picker_selected.min(models.len().saturating_sub(1))));
    }
    
    f.render_stateful_widget(table, chunks[2], &mut state);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);
    
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}