use ratatui::backend::TestBackend;
use ratatui::Terminal;
use voidrecon::app::App;
use voidrecon::llm::ChatMessage;

fn screen_rows(terminal: &Terminal<TestBackend>) -> Vec<String> {
    let buffer = terminal.backend().buffer();
    let width = buffer.area.width as usize;
    buffer
        .content
        .chunks(width)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect()
}

#[tokio::test]
async fn newest_chat_text_stays_visible_after_long_history() {
    let mut app = App::new().await.expect("App::new failed");
    app.ui_state.show_session_picker = false;
    app.ui_state.show_help = false;
    // Real chat content: column-aligned scan output, long unbroken tokens,
    // emoji and tabs, wrapping a varying number of times.
    for index in 0..300 {
        let text = format!(
            "🤖 step {}: PORT     STATE SERVICE  VERSION\n445/tcp  open  microsoft-ds    Windows Server 2019\t{}\nhash {} and url https://example.com/{}?q={}\n{}",
            index,
            "word  ".repeat(index % 31),
            "a1b2c3d4".repeat(5 + index % 17),
            "path/".repeat(index % 29),
            "x".repeat(index % 11),
            "- bullet with   several   spaces ".repeat(1 + index % 5)
        );
        app.llm.chat_log.push(ChatMessage::assistant(text.clone()));
        app.llm.chat_log.push(ChatMessage::output(text));
    }
    app.llm.chat_log.push(ChatMessage::assistant("FINAL-MARKER reply"));
    app.rebuild_cached_chat_lines();
    app.ui_state.log_at_bottom = true;

    let mut terminal = Terminal::new(TestBackend::new(100, 40)).expect("terminal");
    terminal.draw(|f| voidrecon::ui::layout::render_ui(f, &mut app)).expect("draw");
    let rows = screen_rows(&terminal);
    let marker_row = rows
        .iter()
        .position(|row| row.contains("FINAL-MARKER"))
        .expect("newest message is on screen");
    // Below the marker there must not be a large block of empty chat rows.
    let blank_below = rows[marker_row + 1..]
        .iter()
        .take_while(|row| row.trim_matches(|c: char| c == ' ' || c == '│').is_empty())
        .count();
    assert!(blank_below <= 3, "{} blank rows under the newest message:\n{}", blank_below, rows.join("\n"));
}
