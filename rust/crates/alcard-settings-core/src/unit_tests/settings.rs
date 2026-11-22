use super::*;
#[test]
fn manual_ipv4_requires_a_valid_address_and_sets_the_method_together() {
    let op = Operation::Profile {
        uuid: "fixture".into(),
        field: "address",
    };
    for bad in ["example/24", "192.0.2.3/33", "192.0.2.1", "fe80::1/64"] {
        assert!(op.network_args(bad).is_err());
    }
    assert_eq!(
        op.network_args("192.0.2.3/24").unwrap(),
        [
            "connection",
            "modify",
            "uuid",
            "fixture",
            "ipv4.addresses",
            "192.0.2.3/24",
            "ipv4.method",
            "manual"
        ]
    );
}
#[test]
fn dhcp_clears_previous_static_values_in_one_change() {
    let op = Operation::Profile {
        uuid: "fixture".into(),
        field: "dhcp",
    };
    let args = op.network_args("").unwrap();
    assert!(args.windows(2).any(|a| a == ["ipv4.method", "auto"]));
    for key in ["ipv4.addresses", "ipv4.gateway", "ipv4.dns"] {
        assert!(args.windows(2).any(|a| a == [key, ""]));
    }
}
