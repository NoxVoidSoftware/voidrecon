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
        Line::from("   F1 / Alt+H / Ctrl+H - Toggle this help (or type :help)"),
        Line::from("   F2          - Open model picker (or type :models)"),
        Line::from("   F3          - New shell tab (or type :newtab)"),
        Line::from("   F4 / Alt+M  - Toggle Mouse Mode (OFF: native select & copy; ON: scrolling)"),
        Line::from("   Tab         - Complete command or path (shell tabs use shell completion)"),
        Line::from("   Shift+Tab   - Cycle backward through workspace panes and shell tabs"),
        Line::from("   Esc         - Return to Workspace (Tab 0) / dismiss"),
        Line::from("   Ctrl+L      - Enter LLM query mode"),
        Line::from("   Ctrl+C      - Abort running process or LLM query"),
        Line::from("   Ctrl+Q      - Quit (or type :quit)"),
        Line::from("   [           - Toggle sidebar"),
        Line::from(""),
        Line::from(vec![
            Span::styled(" Mouse & Text Selection:", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        ]),
        Line::from("   Mouse Mode is OFF by default, allowing native click & drag to select and copy text."),
        Line::from("   Press F4 or Alt+M (or :mouse on) to enable mouse wheel scrolling & tab clicks."),
        Line::from("   Tip: In most terminals, you can also hold Shift while dragging to select text anytime."),
        Line::from(""),
        Line::from(vec![
            Span::styled(" Special Commands (in Shell mode):", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        ]),
        Line::from("   :targets    - List all targets"),
        Line::from("   :findings   - Show findings for active target"),
        Line::from("   :findings N - Show findings for target N"),
        Line::from("   :note <txt> - Add a note (info finding)"),
        Line::from("   :models     - Open model picker"),
        Line::from("   :provider   - LLM setup & provider switcher (openrouter/nvidia/openai/ollama/custom)"),
        Line::from("   :sessions   - Switch or create assessment sessions"),
        Line::from("   :mouse      - Toggle mouse capture on/off (:mouse on, :mouse off)"),
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
    let area = centered_rect(76, 52, f.size());
    f.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" LLM Provider & Multi-Key Configuration (voidrecon) ");

    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // Provider tabs
            Constraint::Length(3), // Key status & instructions
            Constraint::Length(3), // Input box
            Constraint::Length(2), // Hints
        ])
        .split(inner);

    let providers = crate::config::ALL_PROVIDERS;
    let selected_idx = app.ui_state.provider_picker_selected.min(providers.len().saturating_sub(1));
    let selected_provider = providers[selected_idx];

    // 1. Provider tabs with configuration checkmarks
    let mut tab_spans = vec![Span::styled(" Provider: ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))];
    for (i, p) in providers.iter().enumerate() {
        let is_selected = i == selected_idx;
        let is_configured = app.config.has_api_key(*p);
        let is_active = *p == app.config.provider;
        let tag = if is_configured { " ✓" } else { "" };
        let active_mark = if is_active { "*" } else { "" };
        let label = format!("{}{}{}", active_mark, p.as_str(), tag);

        if is_selected {
            tab_spans.push(Span::styled(
                format!(" [{}] ", label),
                Style::default().bg(Color::Blue).fg(Color::White).add_modifier(Modifier::BOLD),
            ));
        } else if is_configured {
            tab_spans.push(Span::styled(
                format!("  {}  ", label),
                Style::default().fg(Color::Green),
            ));
        } else {
            tab_spans.push(Span::styled(
                format!("  {}  ", label),
                Style::default().fg(Color::DarkGray),
            ));
        }
    }
    f.render_widget(Paragraph::new(Line::from(tab_spans)), chunks[0]);

    // 2. Info / Key status description
    let has_key = app.config.has_api_key(selected_provider);
    let current_key = app.config.get_api_key(selected_provider);
    let endpoint = selected_provider.default_base_url();

    let (status_line, action_line) = if selected_provider.requires_api_key() {
        if has_key {
            (
                Line::from(vec![
                    Span::styled(" Key status: ", Style::default().fg(Color::White)),
                    Span::styled(
                        format!("✓ KEY ALREADY SAVED [{}]", crate::config::mask_key(current_key)),
                        Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!("  (Endpoint: {})", endpoint), Style::default().fg(Color::DarkGray)),
                ]),
                Line::from(Span::styled(
                    " Press [Enter] to activate this provider using the saved key, or type below to OVERWRITE it.",
                    Style::default().fg(Color::Cyan),
                )),
            )
        } else {
            (
                Line::from(vec![
                    Span::styled(" Key status: ", Style::default().fg(Color::White)),
                    Span::styled("✗ NO KEY SAVED FOR THIS PROVIDER", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                    Span::styled(format!("  (Endpoint: {})", endpoint), Style::default().fg(Color::DarkGray)),
                ]),
                Line::from(Span::styled(
                    " Enter your API key below and press [Enter] to save and connect.",
                    Style::default().fg(Color::DarkGray),
                )),
            )
        }
    } else {
        (
            Line::from(vec![
                Span::styled(" Key status: ", Style::default().fg(Color::White)),
                Span::styled("✓ READY (Local service - no key needed)", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                Span::styled(format!("  (Endpoint: {})", endpoint), Style::default().fg(Color::DarkGray)),
            ]),
            Line::from(Span::styled(
                " Press [Enter] to activate this provider.",
                Style::default().fg(Color::Cyan),
            )),
        )
    };

    let desc_para = Paragraph::new(vec![status_line, action_line]);
    f.render_widget(desc_para, chunks[1]);

    // 3. Input box
    let title = if selected_provider.requires_api_key() {
        if has_key {
            format!(" {} API Key [Configured - Type to overwrite] ", selected_provider.as_str())
        } else {
            format!(" {} API Key ", selected_provider.as_str())
        }
    } else {
        format!(" {} API Key (Not required) ", selected_provider.as_str())
    };

    let input_text = if app.ui_state.api_key_input.is_empty() {
        if has_key {
            Span::styled(
                format!(" [Saved: {}] (Press Enter to keep and activate)", crate::config::mask_key(current_key)),
                Style::default().fg(Color::Green).add_modifier(Modifier::ITALIC),
            )
        } else if !selected_provider.requires_api_key() {
            Span::styled(
                " (Not required - press Enter to activate) ",
                Style::default().fg(Color::Green).add_modifier(Modifier::ITALIC),
            )
        } else {
            Span::styled(
                format!(" (Type {} key here...) ", selected_provider.as_str()),
                Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
            )
        }
    } else {
        Span::styled(format!(" Key: {}█", app.ui_state.api_key_input), Style::default().fg(Color::White))
    };

    let input = Paragraph::new(Line::from(input_text))
        .style(Style::default().bg(Color::DarkGray))
        .block(Block::default().borders(Borders::ALL).title(title));
    f.render_widget(input, chunks[2]);

    // 4. Footer navigation hints
    let footer = Paragraph::new(Line::from(vec![
        Span::styled(" [Tab / ← / →] ", Style::default().bg(Color::Blue).fg(Color::White)),
        Span::raw(" Switch Provider    "),
        Span::styled(" [Enter] ", Style::default().bg(Color::Green).fg(Color::Black).add_modifier(Modifier::BOLD)),
        Span::raw(" Save / Overwrite & Activate    "),
        Span::styled(" [Esc] ", Style::default().bg(Color::DarkGray).fg(Color::White)),
        Span::raw(" Close"),
    ]));
    f.render_widget(footer, chunks[3]);
}

pub fn render_model_picker_modal(f: &mut Frame, app: &mut App) {
    let area = centered_rect(82, 82, f.size());
    f.render_widget(Clear, area);

    let viewing_provider = app.ui_state.model_picker_provider;
    let title = format!(
        " Model Picker: {} (Tab: Switch Provider | F4: Free | F5: Refresh | Enter: Select | Esc: Cancel) ",
        viewing_provider.as_str()
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title);
    let inner_area = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Provider switch bar [Tab]
            Constraint::Length(1), // Search bar
            Constraint::Length(1), // Divider
            Constraint::Min(0),    // Models table
        ])
        .split(inner_area);

    // Top: Connected Providers tab bar
    let configured = app.config.configured_providers();
    let mut prov_spans = vec![
        Span::styled(" Provider [Tab]: ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
    ];
    for p in &configured {
        let is_viewing = *p == viewing_provider;
        let is_active_in_config = *p == app.config.provider;
        let is_conf = app.config.has_api_key(*p);
        let tag = if is_conf { " ✓" } else { "" };
        let active_badge = if is_active_in_config { "*" } else { "" };
        let text = format!(" {}{}{} ", active_badge, p.as_str(), tag);
        if is_viewing {
            prov_spans.push(Span::styled(
                format!(" [{}] ", text.trim()),
                Style::default().bg(Color::Blue).fg(Color::White).add_modifier(Modifier::BOLD),
            ));
        } else if is_conf {
            prov_spans.push(Span::styled(
                format!(" {} ", text.trim()),
                Style::default().fg(Color::Green),
            ));
        } else {
            prov_spans.push(Span::styled(
                format!(" {} ", text.trim()),
                Style::default().fg(Color::DarkGray),
            ));
        }
    }
    f.render_widget(Paragraph::new(Line::from(prov_spans)), chunks[0]);
    
    let models: Vec<&crate::config::ModelInfo> = if app.ui_state.model_picker_free_only {
        app.config.get_free_models_for_provider(viewing_provider)
    } else {
        app.config.search_models_for_provider(viewing_provider, &app.ui_state.model_picker_search)
    };
    
    // Search input
    let free_indicator = if app.ui_state.model_picker_free_only {
        " [FREE ONLY: ON (F4/Ctrl+F)]"
    } else {
        " [FREE ONLY: OFF (F4/Ctrl+F)]"
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
    f.render_widget(Paragraph::new(search_line), chunks[1]);

    // Separator line
    f.render_widget(Block::default().borders(Borders::BOTTOM).border_style(Style::default().fg(Color::DarkGray)), chunks[2]);
    
    let mut rows = Vec::new();
    for (idx, model) in models.iter().enumerate() {
        let is_selected = idx == app.ui_state.model_picker_selected;
        let is_active = model.id == app.config.active_model && viewing_provider == app.config.provider;
        
        let price = if model.id.ends_with(":free") || (model.pricing.prompt == "0" && model.pricing.completion == "0") {
            "FREE".to_string()
        } else {
            format!("${}/${}", model.pricing.prompt, model.pricing.completion)
        };
        
        let ctx_len = match model.context_length {
            Some(len) if len >= 1_000_000 => format!("{}M", len / 1_000_000),
            Some(len) if len >= 1_000 => format!("{}k", len / 1_000),
            Some(len) if len > 0 => format!("{}", len),
            _ => "N/A".to_string(),
        };
        
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
    
    if rows.is_empty() {
        rows.push(
            Row::new(vec![
                format!("[No models cached for {} - press F5/Ctrl+R to fetch]", viewing_provider.as_str()),
                "-".to_string(),
                "-".to_string(),
            ])
            .style(Style::default().fg(Color::Yellow)),
        );
    }

    let table = Table::new(rows, [
        Constraint::Percentage(55),
        Constraint::Length(12),
        Constraint::Min(15),
    ])
    .header(Row::new(vec!["Model Name", "Context", "Price"]).style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)))
    .highlight_style(Style::default().bg(Color::Blue).fg(Color::White).add_modifier(Modifier::BOLD));
    
    let mut state = TableState::default();
    if !models.is_empty() {
        state.select(Some(app.ui_state.model_picker_selected.min(models.len().saturating_sub(1))));
    }
    
    f.render_stateful_widget(table, chunks[3], &mut state);
}

pub fn render_session_picker_modal(f: &mut Frame, app: &mut App) {
    if app.ui_state.session_naming_mode {
        let area = centered_rect(65, 36, f.size());
        f.render_widget(Clear, area);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(" 📝 Create New Assessment Session ");
        let inner_area = block.inner(area);
        f.render_widget(block, area);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2), // Prompt instruction
                Constraint::Length(3), // Input box
                Constraint::Length(2), // Hint / path preview
                Constraint::Min(0),    // Space
                Constraint::Length(1), // Footer controls
            ])
            .split(inner_area);

        let prompt = Paragraph::new(Line::from(vec![
            Span::styled("Enter a name for the new session (alphanumeric, '-', '_'):", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        ]));
        f.render_widget(prompt, chunks[0]);

        let cursor_char = if app.ui_state.tick_count % 10 < 5 { "█" } else { " " };
        let input_line = if app.ui_state.session_name_input.is_empty() {
            Line::from(vec![
                Span::styled("  > ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                Span::styled("e.g. corp-network-audit", Style::default().fg(Color::DarkGray)),
                Span::styled(cursor_char, Style::default().fg(Color::Cyan)),
            ])
        } else {
            Line::from(vec![
                Span::styled("  > ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                Span::styled(&app.ui_state.session_name_input, Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                Span::styled(cursor_char, Style::default().fg(Color::Cyan)),
            ])
        };

        let input_box = Paragraph::new(input_line).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        );
        f.render_widget(input_box, chunks[1]);

        let default_suggestion = format!("{}_scan", chrono::Local::now().format("%Y-%m-%d"));
        let hint_line = if app.ui_state.session_name_input.is_empty() {
            Line::from(vec![
                Span::styled("  ℹ Leave empty and press Enter for default: ", Style::default().fg(Color::DarkGray)),
                Span::styled(default_suggestion, Style::default().fg(Color::Green)),
            ])
        } else {
            let clean: String = app.ui_state.session_name_input.chars().map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' }
            }).collect();
            Line::from(vec![
                Span::styled("  📁 Will create: ", Style::default().fg(Color::DarkGray)),
                Span::styled(format!("~/.config/voidrecon/sessions/{}/", clean), Style::default().fg(Color::Green)),
            ])
        };
        f.render_widget(Paragraph::new(hint_line), chunks[2]);

        let footer = Paragraph::new(Line::from(vec![
            Span::styled(" [Enter] ", Style::default().bg(Color::Green).fg(Color::Black).add_modifier(Modifier::BOLD)),
            Span::raw(" Create & Enter Session    "),
            Span::styled(" [Esc] ", Style::default().bg(Color::DarkGray).fg(Color::White)),
            Span::raw(" Back to Sessions List"),
        ]));
        f.render_widget(footer, chunks[4]);
        return;
    }

    if let Some(target_session) = &app.ui_state.session_delete_confirm {
        let area = centered_rect(58, 25, f.size());
        f.render_widget(Clear, area);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Red))
            .title(" ⚠️ Confirm Delete Session ");
        let inner_area = block.inner(area);
        f.render_widget(block, area);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // Question
                Constraint::Length(1), // Session name
                Constraint::Length(2), // Warning
                Constraint::Min(0),    // Space
                Constraint::Length(1), // Buttons
            ])
            .split(inner_area);

        let question = Paragraph::new(Line::from(vec![
            Span::styled("Are you sure you want to delete session:", Style::default().fg(Color::White)),
        ]));
        f.render_widget(question, chunks[0]);

        let name_p = Paragraph::new(Line::from(vec![
            Span::styled(format!("  \"{}\"", target_session), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        ]));
        f.render_widget(name_p, chunks[1]);

        let warn = Paragraph::new(Line::from(vec![
            Span::styled("  All scan logs, database targets, findings and artifacts will be permanently erased.", Style::default().fg(Color::DarkGray)),
        ]));
        f.render_widget(warn, chunks[2]);

        let buttons = Paragraph::new(Line::from(vec![
            Span::styled(" [y / Enter] Yes, Delete ", Style::default().bg(Color::Red).fg(Color::White).add_modifier(Modifier::BOLD)),
            Span::raw("     "),
            Span::styled(" [n / Esc] No, Cancel ", Style::default().bg(Color::DarkGray).fg(Color::White)),
        ]));
        f.render_widget(buttons, chunks[4]);
        return;
    }

    let area = centered_rect(75, 60, f.size());
    f.render_widget(Clear, area);

    let title = if app.ui_state.is_initial_session_select {
        " 🎯 Select or Create Assessment Session (Enter: Select | D: Delete | Esc: Quit) "
    } else {
        " 🎯 Assessment Sessions (Enter: Select | D: Delete | Esc: Close) "
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .title(title);
    let inner_area = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // Top description
            Constraint::Min(0),    // Table
            Constraint::Length(1), // Footer hint
        ])
        .split(inner_area);

    let intro = if app.ui_state.is_initial_session_select {
        Paragraph::new(Line::from(vec![
            Span::styled("Welcome to VoidRecon! ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Span::styled("Choose an existing assessment session or create a new one to enter:", Style::default().fg(Color::White)),
        ]))
    } else {
        Paragraph::new(Line::from(vec![
            Span::styled("Current Session: ", Style::default().fg(Color::Gray)),
            Span::styled(&app.session_meta.name, Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
        ]))
    };
    f.render_widget(intro, chunks[0]);

    // Rows: First is [+ Create New Session], followed by existing sessions
    let mut rows = Vec::new();
    let is_create_selected = app.ui_state.session_picker_selected == 0;
    rows.push(
        Row::new(vec![
            "[+ Create New Session (Enter custom name)]".to_string(),
            "-".to_string(),
            "-".to_string(),
        ])
        .style(if is_create_selected {
            Style::default().bg(Color::Blue).fg(Color::White).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Cyan)
        }),
    );

    for (idx, s) in app.ui_state.session_list.iter().enumerate() {
        let is_selected = idx + 1 == app.ui_state.session_picker_selected;
        let is_current = !app.ui_state.is_initial_session_select && s.name == app.session_meta.name;
        let prefix = if is_current { "* " } else { "  " };

        let style = if is_selected {
            Style::default().bg(Color::Blue).fg(Color::White).add_modifier(Modifier::BOLD)
        } else if is_current {
            Style::default().fg(Color::Green)
        } else {
            Style::default()
        };

        rows.push(
            Row::new(vec![
                format!("{}{}", prefix, s.name),
                s.created_at.format("%Y-%m-%d %H:%M").to_string(),
                s.last_activity.format("%Y-%m-%d %H:%M").to_string(),
            ])
            .style(style),
        );
    }

    let table = Table::new(rows, [
        Constraint::Percentage(50),
        Constraint::Percentage(25),
        Constraint::Percentage(25),
    ])
    .header(
        Row::new(vec!["Session Name", "Created", "Last Activity"])
            .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
    )
    .highlight_style(Style::default().bg(Color::Blue).fg(Color::White).add_modifier(Modifier::BOLD));

    let mut state = TableState::default();
    let total_rows = 1 + app.ui_state.session_list.len();
    state.select(Some(app.ui_state.session_picker_selected.min(total_rows.saturating_sub(1))));

    f.render_stateful_widget(table, chunks[1], &mut state);

    let esc_label = if app.ui_state.is_initial_session_select {
        " Quit"
    } else {
        " Cancel"
    };

    let footer = Paragraph::new(Line::from(vec![
        Span::styled(" [↑/↓] ", Style::default().bg(Color::DarkGray).fg(Color::White)),
        Span::raw(" Navigate   "),
        Span::styled(" [Enter] ", Style::default().bg(Color::Green).fg(Color::Black)),
        Span::raw(" Select / Create   "),
        Span::styled(" [D] ", Style::default().bg(Color::Red).fg(Color::White)),
        Span::raw(" Delete Session   "),
        Span::styled(" [Esc] ", Style::default().bg(Color::DarkGray).fg(Color::White)),
        Span::raw(esc_label),
    ]));
    f.render_widget(footer, chunks[2]);
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