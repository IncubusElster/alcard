use super::*;
#[test]
fn notification_policy_never_takes_keyboard_focus_or_reserves_space() {
    assert_eq!(KEYBOARD, KeyboardInteractivity::None);
    assert_eq!(EXCLUSIVE_ZONE, -1);
}
#[test]
fn transparent_argb_banner_has_bounded_buffer_size() {
    let mut image = Pixmap::new(render::WIDTH as u32, HEIGHT).unwrap();
    render::paint_notification(&mut image, "notice", 0x123456, 0xffffff, 0xabcdef);
    assert_eq!(image.data().len(), 320 * 40 * 4);
    assert_eq!(image.pixel(0, 0).unwrap().alpha(), 0);
    assert_eq!(image.pixel(8, 4).unwrap().alpha(), 255);
}
