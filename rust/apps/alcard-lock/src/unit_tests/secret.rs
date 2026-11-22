use super::*;

#[test]
fn unicode_edits_and_clear_wipe_removed_bytes_without_reallocation() {
    let mut value = Secret::default();
    let allocation = value.bytes.as_ptr();
    for ch in "4 5 6 7 8 Яñ".chars() {
        assert!(value.insert(ch));
    }
    let old_length = value.len;
    value.backspace();
    assert_eq!(value.as_str(), "4 5 6 7 8 Я");
    assert!(value.bytes[value.len..old_length]
        .iter()
        .all(|byte| *byte == 0));
    value.clear();
    assert!(value.bytes.iter().all(|byte| *byte == 0));
    assert_eq!(value.count(), 0);
    assert_eq!(value.bytes.as_ptr(), allocation);
}

#[test]
fn both_character_and_byte_limits_reject_extra_or_control_input() {
    let mut value = Secret::default();
    for _ in 0..MAX_CHARS {
        assert!(value.insert('🦀'));
    }
    assert_eq!(value.len, MAX_BYTES);
    assert!(!value.insert('a'));
    assert!(!value.insert('\n'));
    assert!(Secret::from_bytes(b"a\0b").is_none());
    assert!(Secret::from_bytes(&[0xff]).is_none());
    assert!(Secret::from_bytes(&[b'a'; MAX_CHARS + 1]).is_none());
}
