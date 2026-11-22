use super::*;

#[test]
fn edits_english_russian_and_spanish_without_splitting_utf8() {
    let mut input = TextInput::new("AЯñ", 8);
    assert_eq!(input.cursor(), 3);
    assert!(input.left());
    assert!(input.insert("éё"));
    assert_eq!(input.value(), "AЯéёñ");
    assert_eq!(input.cursor(), 4);
    assert!(input.backspace());
    assert!(input.delete());
    assert_eq!(input.value(), "AЯé");
    assert!(input.home());
    assert!(!input.backspace());
    assert!(input.delete());
    assert_eq!(input.value(), "Яé");
    assert!(input.end());
    assert!(!input.delete());
    assert!(!input.right());
    assert!(input.clear());
    assert_eq!(input.cursor(), 0);
    assert!(!input.clear());
}

#[test]
fn text_limits_count_characters_and_reject_controls() {
    let mut input = TextInput::new("Я\nñ\té\0", 3);
    assert_eq!(input.value(), "Яñé");
    assert!(!input.insert("4"));
    input.left();
    input.delete();
    assert!(input.insert("\r\nümore"));
    assert_eq!(input.value(), "Яñü");
    assert_eq!(input.cursor(), 3);
    let full = TextInput::new(&"ё".repeat(MAX_TEXT_CHARS + 1), usize::MAX);
    assert_eq!(full.value().chars().count(), MAX_TEXT_CHARS);
    assert_eq!(full.cursor(), MAX_TEXT_CHARS);
    assert!(!TextInput::new("", 0).insert("a"));
    let mut empty = TextInput::new("", 8);
    assert!(!empty.insert(&"\0".repeat(MAX_TEXT_SCAN + 1)));
}

#[test]
fn viewport_keeps_a_multibyte_cursor_visible_at_both_ends() {
    let mut input = TextInput::new("ABCЯñé", 20);
    assert_eq!(
        input.viewport(4),
        TextViewport {
            text: "Яñé".into(),
            cursor: 3,
            offset: 3,
        }
    );
    input.home();
    assert_eq!(input.viewport(4).text, "ABCЯ");
    assert_eq!(input.viewport(4).cursor, 0);
    assert_eq!(input.viewport(4).offset, 0);
    input.right();
    assert_eq!(input.viewport(1).text, "B");
    assert_eq!(input.viewport(1).cursor, 0);
    assert_eq!(input.viewport(1).offset, 1);
    assert_eq!(input.viewport(0).text, "");
    input.end();
    assert_eq!(input.viewport(1).text, "");
    assert_eq!(input.viewport(1).offset, 6);
}

#[test]
fn choices_retain_the_key_through_reorder_and_clamp_after_removal() {
    let mut choices = ChoiceList::new(["en", "ru", "es"]);
    assert!(choices.select(&"ru"));
    assert!(choices.reconcile(["es", "en", "ru"]));
    assert_eq!(choices.selected(), Some(&"ru"));
    assert_eq!(choices.selected_index(), Some(2));
    assert!(!choices.reconcile(["es", "en", "ru"]));
    assert!(choices.reconcile(["es", "en"]));
    assert_eq!(choices.selected(), Some(&"en"));
    assert_eq!(choices.selected_index(), Some(1));
    assert!(!choices.down());
    assert!(choices.up());
    assert!(!choices.up());
    assert!(!choices.select(&"missing"));
    assert_eq!(choices.selected(), Some(&"es"));
    assert!(choices.reconcile([]));
    assert_eq!(choices.selected(), None);
    assert_eq!(choices.selected_index(), None);
    assert!(!choices.up());
    assert!(!choices.down());
    assert!(choices.reconcile(["ru"]));
    assert_eq!(choices.selected(), Some(&"ru"));
}

#[test]
fn choices_bound_input_before_deduplication() {
    let duplicate = ChoiceList::new(std::iter::repeat("ru"));
    assert_eq!(duplicate.selected(), Some(&"ru"));
    assert_eq!(duplicate.keys.len(), 1);
    let choices = ChoiceList::new(0..);
    assert_eq!(choices.keys.len(), MAX_CHOICES);
    assert_eq!(choices.keys.last(), Some(&(MAX_CHOICES - 1)));
}
