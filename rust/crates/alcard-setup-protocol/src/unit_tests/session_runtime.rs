use super::{helper_privilege, shadow_group};

#[test]
fn password_helper_accepts_only_root_or_exact_shadow_privilege() {
    assert!(helper_privilege(0o2755, 0, 42, Some(42)));
    assert!(helper_privilege(0o2751, 0, 42, Some(42)));
    assert!(helper_privilege(0o4755, 0, 0, Some(42)));
    assert!(!helper_privilege(0o0755, 0, 42, Some(42)));
    assert!(!helper_privilege(0o2755, 0, 43, Some(42)));
    assert!(!helper_privilege(0o2755, 0, 42, None));
    assert!(!helper_privilege(0o4755, 1000, 0, Some(42)));
    assert!(!helper_privilege(0o4775, 0, 0, Some(42)));
    assert!(!helper_privilege(0o2757, 0, 42, Some(42)));
    assert!(!helper_privilege(0o4750, 0, 0, Some(42)));
}

#[test]
fn polkit_requires_suid_root_and_shadow_lookup_is_unambiguous() {
    assert!(helper_privilege(0o4755, 0, 0, None));
    assert!(!helper_privilege(0o2755, 0, 42, None));
    assert_eq!(shadow_group("root:x:0:root\nshadow:x:42:\n"), Some(42));
    assert_eq!(shadow_group("shadow:x:42:\nshadow:x:43:\n"), None);
    assert_eq!(shadow_group("shadow:x:invalid:\n"), None);
    assert_eq!(shadow_group("shadow:x:42\n"), None);
    assert_eq!(shadow_group("root:x:0:root\n"), None);
}
