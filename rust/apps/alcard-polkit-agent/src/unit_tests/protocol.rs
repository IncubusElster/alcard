use super::*;
#[test]
fn secret_edits_wipe_removed_utf8_and_accept_all_softkey_digits() {
    let mut s = Secret::default();
    for c in "45678Яñ".chars() {
        assert!(s.insert(c));
    }
    assert_eq!(s.count(), 7);
    let old_len = s.len;
    s.backspace();
    assert_eq!(s.as_str(), "45678Я");
    assert!(s.bytes[s.len..old_len].iter().all(|b| *b == 0));
    s.clear();
    assert_eq!(s.count(), 0);
    assert!(s.bytes.iter().all(|b| *b == 0));
}
#[test]
fn secret_limits_do_not_reallocate_or_accept_control_characters() {
    let mut s = Secret::default();
    let ptr = s.bytes.as_ptr();
    for _ in 0..128 {
        assert!(s.insert('🦀'));
    }
    assert!(!s.insert('a'));
    assert!(!s.insert('\n'));
    assert_eq!(s.len, 512);
    assert_eq!(s.bytes.as_ptr(), ptr);
}
#[test]
fn public_text_is_bounded_and_cannot_embed_bidi_or_terminal_controls() {
    assert_eq!(plain("a\u{202e}b\n\u{1b}c", 20), "abc");
    assert_eq!(plain(&"x".repeat(5000), 192).len(), 192);
}
