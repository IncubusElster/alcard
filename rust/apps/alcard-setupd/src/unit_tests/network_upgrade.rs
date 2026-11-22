use super::*;
#[test]
fn imports_both_previous_hello_formats_without_reinterpreting_ssid() {
    let make = |s: &str| {
        format!("ctrl_interface=/run/wpa_supplicant\nupdate_config=0\nnetwork={{\n    ssid=\"A\\\"B\"\n    {s}\n}}\n")
    };
    let open = profile("wlan0", &make("key_mgmt=NONE")).unwrap();
    assert!(open.contains("ssid=65;34;66;"));
    assert!(!open.contains("wifi-security"));
    let psk = "a".repeat(64);
    let secured = profile("wlan0", &make(&format!("psk={psk}"))).unwrap();
    assert!(secured.contains(&format!("psk={psk}\n")));
    assert!(profile("wlan0", &make("psk=invalid")).is_err());
}
