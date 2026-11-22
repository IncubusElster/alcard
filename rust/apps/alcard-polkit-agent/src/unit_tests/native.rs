use super::*;
#[test]
fn identities_are_never_filtered_reordered_or_inferred_from_labels() {
    let make = |uid: u32| {
        (
            "unix-user".into(),
            HashMap::from([("uid".into(), uid.to_variant())]),
        )
    };
    let (_, labels) = public_identities(vec![make(0), make(1000)]).unwrap();
    assert_eq!(labels[0].index, 0);
    assert_eq!(labels[1].index, 1);
    assert!(labels[1].label.contains("uid 1000"));
    assert!(public_identities(vec![make(0), make(0)]).is_none());
    assert!(public_identities(vec![("unix-group".into(), HashMap::new())]).is_none());
    assert!(public_identities(Vec::new()).is_none());
}
