use voidrecon::app::App;

#[tokio::test]
async fn app_boots() {
    let app = App::new().await.expect("App::new failed");
    assert!(!app.ui_state.should_quit);
}