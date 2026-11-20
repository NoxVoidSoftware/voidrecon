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
