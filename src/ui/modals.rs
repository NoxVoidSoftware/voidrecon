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
        Line::from("   F1          - Toggle this help"),
        Line::from("   F2          - Open model picker"),
        Line::from("   F3          - New shell tab"),
        Line::from("   Tab         - Cycle tabs"),
        Line::from("   Ctrl+L      - Enter LLM query mode"),
        Line::from("   Ctrl+Q      - Quit"),
        Line::from("   [           - Toggle sidebar"),
        Line::from("   Esc         - Return to Shell mode"),
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

pub fn render_model_picker_modal(f: &mut Frame, app: &mut App) {
    let area = centered_rect(80, 80, f.size());
    f.render_widget(Clear, area);
    
    let models: Vec<&crate::config::ModelInfo> = if app.ui_state.model_picker_free_only {
        app.config.get_free_models()
    } else {
        app.config.search_models(&app.ui_state.model_picker_search)
    };
    
    let mut rows = Vec::new();
    for (idx, model) in models.iter().enumerate() {
        let price = if model.id.ends_with(":free") || (model.pricing.prompt == "0" && model.pricing.completion == "0") {
            "FREE".to_string()
        } else {
            format!("${}/${}", model.pricing.prompt, model.pricing.completion)
        };
        
        let ctx_len = model.context_length.map(|c| c.to_string()).unwrap_or("-".to_string());
        
        let style = if idx == app.ui_state.model_picker_selected {
            Style::default().bg(Color::Blue).fg(Color::White)
        } else {
            Style::default()
        };
        
        rows.push(Row::new(vec![
            model.name.clone(),
            ctx_len,
            price,
        ]).style(style));
    }
    
    let table = Table::new(rows, [
        Constraint::Percentage(50),
        Constraint::Percentage(15),
        Constraint::Percentage(35),
    ])
    .header(Row::new(vec!["Model Name", "Context", "Price"]).style(Style::default().add_modifier(Modifier::BOLD)))
    .block(Block::default().borders(Borders::ALL).title(" Model Picker (F2/Esc to close, Enter to select) "))
    .highlight_style(Style::default().bg(Color::Blue).fg(Color::White));
    
    let mut state = TableState::default();
    state.select(Some(app.ui_state.model_picker_selected.min(models.len().saturating_sub(1))));
    
    f.render_stateful_widget(table, area, &mut state);
    
    // Search input at top
    let search_area = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width - 2,
        height: 1,
    };
    let free_indicator = if app.ui_state.model_picker_free_only { " [FREE ONLY]" } else { "" };
    let search_text = format!("Search: {}{}", app.ui_state.model_picker_search, free_indicator);
    let search = Paragraph::new(search_text).style(Style::default().fg(Color::Yellow));
    f.render_widget(search, search_area);
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