use voidrecon::app::App;

#[tokio::test]
async fn tab_numbers_are_reused_after_close() {
    let mut app = App::new().await.expect("App::new failed");
    app.new_shell_tab().await.expect("first shell tab");
    app.new_shell_tab().await.expect("second shell tab");
    let numbers: Vec<usize> = app.tabs.iter().map(|tab| tab.number).collect();
    assert_eq!(numbers, vec![1, 2]);

    app.close_terminal_tab(1).expect("close tab 1");
    assert_eq!(app.tabs[0].number, 2, "remaining tab keeps its number");

    app.new_shell_tab().await.expect("third shell tab");
    let numbers: Vec<usize> = app.tabs.iter().map(|tab| tab.number).collect();
    assert_eq!(numbers, vec![2, 1], "freed number 1 is reused");

    // Internal ids stay unique so late events from a closed tab are ignored.
    let ids: Vec<usize> = app.tabs.iter().map(|tab| tab.id).collect();
    assert_eq!(ids, vec![2, 3]);
    assert_eq!(app.active_terminal_tab_id(), Some(1));
}

#[tokio::test]
async fn skipped_suggestion_is_reported_to_the_llm() {
    let mut app = App::new().await.expect("App::new failed");
    app.llm.suggestion = Some(voidrecon::llm::types::LlmSuggestion {
        command: Some("nmap -p- 10.10.10.10".to_string()),
        explanation: String::new(),
        interactive: false,
    });
    app.llm.suggestion_visible = true;

    app.skip_suggestion().await;

    assert!(!app.llm.suggestion_visible);
    assert!(app.llm.suggestion.is_none());
    let last = app.llm.chat_log.last().expect("skip note");
    assert_eq!(last.role, "user");
    assert!(last.content.contains("NOT executed"));
    assert!(last.content.contains("nmap -p- 10.10.10.10"));

    // Nothing pending: skipping again records nothing.
    let count = app.llm.chat_log.len();
    app.skip_suggestion().await;
    assert_eq!(app.llm.chat_log.len(), count);
}

#[tokio::test]
async fn long_running_command_is_reported_once() {
    let mut app = App::new().await.expect("App::new failed");
    // Keep the follow-up analysis from reaching a real LLM provider.
    app.config.api_key.clear();
    app.run_llm_command("sleep 30", false).await.expect("start command");
    let id = *app.tab_command_runs.keys().next().expect("tracked run");
    if let Some(run) = app.tab_command_runs.get_mut(&id) {
        run.started -= std::time::Duration::from_secs(20);
    }
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    app.handle_tick().await;
    let reports = |app: &App| {
        app.llm
            .chat_log
            .iter()
            .filter(|message| message.content.contains("still running after"))
            .count()
    };
    assert_eq!(reports(&app), 1);
    assert!(app.tab_command_runs[&id].reported_running);

    app.handle_tick().await;
    assert_eq!(reports(&app), 1, "reported only once");
}

#[tokio::test]
async fn yolo_runs_waiting_suggestion_and_respects_the_cap() {
    let mut app = App::new().await.expect("App::new failed");
    app.config.api_key.clear();
    let suggest = |app: &mut App| {
        app.llm.suggestion = Some(voidrecon::llm::types::LlmSuggestion {
            command: Some("echo yolo".to_string()),
            explanation: String::new(),
            interactive: false,
        });
        app.llm.suggestion_visible = true;
    };

    // Off: the suggestion waits for the user.
    suggest(&mut app);
    app.run_yolo_step().await.expect("step");
    assert!(app.llm.suggestion_visible);

    // Turning it on runs what is waiting.
    app.set_yolo("on").await.expect("yolo on");
    assert!(app.yolo.enabled);
    assert!(app.llm.suggestion.is_none());
    assert_eq!(app.tab_command_runs.len(), 1, "command started in its own tab");
    assert_eq!(app.yolo.auto_actions, 1);

    // At the cap it pauses and leaves the action to the user.
    app.yolo.auto_actions = voidrecon::app::yolo::MAX_AUTO_ACTIONS;
    suggest(&mut app);
    app.run_yolo_step().await.expect("step");
    assert!(app.llm.suggestion_visible);

    app.set_yolo("off").await.expect("yolo off");
    assert!(!app.yolo.enabled);
}
