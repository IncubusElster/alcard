use super::*;

#[test]
fn exactly_one_enum_argument_is_required() {
    for values in [
        vec![],
        vec!["wifi-on", "wlan0"],
        vec!["wifi-on", "--help"],
        vec!["/bin/sh"],
        vec!["status\n"],
    ] {
        assert!(argument(values.into_iter().map(OsString::from)).is_err());
    }
    assert_eq!(argument([OsString::from("status")]), Ok(Operation::Refresh));
}

#[test]
fn non_utf8_arguments_are_rejected() {
    use std::os::unix::ffi::OsStringExt;
    assert!(argument([OsString::from_vec(vec![0xff])]).is_err());
}
