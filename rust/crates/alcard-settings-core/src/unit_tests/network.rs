use super::*;

#[test]
fn escaped_names_remain_single_fields() {
    assert_eq!(fields(r"wlan0:Café\: 1\\2"), ["wlan0", r"Café: 1\2"]);
    assert_eq!(fields("::"), ["", "", ""]);
    assert!(!valid_ssid("bad\nname"));
}
