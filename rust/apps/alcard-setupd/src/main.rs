//! Alcard firstboot backend: root-gated account setup and read-only checks.
//! No listener, socket, developer service or remote API.
mod admin_policy;
mod apply;
mod failure;
mod network_upgrade;

use alcard_setup_protocol::{validate, SetupPlan};
use std::io::{self, Read};

const MAX_PLAN_BYTES: u64 = 16 * 1024;

fn check() -> Result<(), &'static str> {
    let mut input = Vec::new();
    io::stdin()
        .take(MAX_PLAN_BYTES + 1)
        .read_to_end(&mut input)
        .map_err(|_| "input_read_failed")?;
    if input.len() as u64 > MAX_PLAN_BYTES {
        return Err("input_too_large");
    }
    let plan: SetupPlan = serde_json::from_slice(&input).map_err(|_| "invalid_plan_json")?;
    validate(&plan).map_err(|_| "invalid_plan")
}

fn usage() -> ! {
    eprintln!(
        "alcard-setupd: --check | --needs-hello | --arm-firstboot | --arm-explicit-root | --apply | --setting KEY VALUE | --migrate-network"
    );
    std::process::exit(2);
}

fn main() {
    let mut args = std::env::args().skip(1);
    let operation = args.next().unwrap_or_else(|| usage());
    if operation == "--setting" {
        let key = args.next().unwrap_or_else(|| usage());
        let value = args.next().unwrap_or_else(|| usage());
        if args.next().is_some() {
            usage();
        }
        if let Err(error) = apply::setting(&key, &value) {
            eprintln!("{} ({})", error, error.kind());
            std::process::exit(1);
        }
        return;
    }
    if args.next().is_some() {
        usage();
    }
    match operation.as_str() {
        "--migrate-network" => {
            if let Err(err) = network_upgrade::run() {
                eprintln!("Alcard network upgrade: {err}");
                std::process::exit(1);
            }
        }
        "--needs-hello" => match apply::needs_hello() {
            Ok(true) => std::process::exit(0),
            Ok(false) => std::process::exit(1),
            Err(_) => std::process::exit(2),
        },
        "--arm-firstboot" | "--arm-explicit-root" => {
            let result = if operation == "--arm-firstboot" {
                apply::arm_firstboot()
            } else {
                apply::arm_explicit_root()
            };
            if let Err(err) = result {
                eprintln!(
                    "alcard-setupd: cannot_arm cause={} ({})",
                    failure::cause(&err),
                    err.kind()
                );
                std::process::exit(1);
            }
        }
        "--apply" => match apply::run_firstboot_apply() {
            Ok(()) => println!(r#"{{"accepted":true,"applied":true}}"#),
            Err(err) => {
                eprintln!(
                    "alcard-setupd: apply_failed stage={} cause={} ({})",
                    failure::stage(&err),
                    failure::cause(&err),
                    err.kind()
                );
                std::process::exit(1);
            }
        },
        "--check" => match check() {
            Ok(()) => println!(r#"{{"accepted":true,"applied":false}}"#),
            Err(code) => {
                eprintln!("alcard-setupd: {code}");
                std::process::exit(1);
            }
        },
        _ => usage(),
    }
}
