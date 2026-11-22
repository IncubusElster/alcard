use super::*;

#[test]
fn back_from_the_welcome_screen_exits() {
    let mut app = App::new_firstboot();
    app.handle(Intent::Soft(4));
    assert!(app.should_exit);
}
