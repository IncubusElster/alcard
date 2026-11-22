//! NetworkManager owns interfaces, DHCP, secrets and connection persistence.
use std::{
    collections::BTreeSet,
    io::{self, Write},
    process::{Command, Stdio},
};

pub const NMCLI: &str = "/usr/bin/nmcli";

pub fn query(args: &[&str]) -> io::Result<String> {
    let out = Command::new(NMCLI)
        .env("LC_ALL", "C")
        .args(["--colors", "no", "--wait", "10"])
        .args(args)
        .stdin(Stdio::null())
        .output()?;
    if !out.status.success() {
        return Err(io::Error::other(
            String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        ));
    }
    String::from_utf8(out.stdout).map_err(io::Error::other)
}

/// Decode nmcli terse output, in which ':' and the escape itself are escaped.
pub fn fields(line: &str) -> Vec<String> {
    let mut result = vec![String::new()];
    let mut escaped = false;
    for c in line.chars() {
        if !escaped && c == ':' {
            result.push(String::new());
        } else if !escaped && c == '\\' {
            escaped = true;
            continue;
        } else {
            result.last_mut().unwrap().push(c);
        }
        escaped = false;
    }
    result
}

pub fn devices() -> io::Result<Vec<Vec<String>>> {
    Ok(query(&[
        "-t",
        "-f",
        "DEVICE,TYPE,STATE,CONNECTION",
        "device",
        "status",
    ])?
    .lines()
    .map(fields)
    .filter(|row| row.len() == 4)
    .collect())
}

pub fn scan_fields(iface: &str, columns: &str) -> io::Result<String> {
    let list = |rescan| {
        query(&[
            "-t", "-f", columns, "device", "wifi", "list", "ifname", iface, "--rescan", rescan,
        ])
    };
    // A console user may read the AP cache even when policy restricts active scans.
    list("yes").or_else(|err| {
        if err
            .to_string()
            .to_ascii_lowercase()
            .contains("not authorized")
            || err.to_string().to_ascii_lowercase().contains("not allowed")
        {
            list("no")
        } else {
            Err(err)
        }
    })
}

pub fn scan(iface: &str) -> io::Result<Vec<String>> {
    Ok(scan_fields(iface, "SSID")?
        .lines()
        .flat_map(fields)
        .filter(|s| valid_ssid(s))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect())
}

pub fn valid_ssid(s: &str) -> bool {
    !s.is_empty() && s.len() <= 32 && !s.chars().any(char::is_control)
}

pub fn connect(iface: &str, ssid: &str, password: &str) -> io::Result<()> {
    connect_until(
        iface,
        ssid,
        password,
        &std::sync::atomic::AtomicBool::new(false),
    )
}

pub fn connect_until(
    iface: &str,
    ssid: &str,
    password: &str,
    cancel: &std::sync::atomic::AtomicBool,
) -> io::Result<()> {
    use std::{
        sync::atomic::Ordering,
        time::{Duration, Instant},
    };
    if cancel.load(Ordering::Acquire) {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "Cancelled"));
    }

    if !valid_ssid(ssid) || password.chars().any(char::is_control) || password.len() > 64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid Wi-Fi credentials",
        ));
    }
    let mut child = Command::new(NMCLI)
        .args([
            "--wait", "25", "--ask", "device", "wifi", "connect", ssid, "ifname", iface,
        ])
        .env("LC_ALL", "C")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    if let Some(mut input) = child.stdin.take() {
        // Open and already saved networks need no input; nmcli may close its pipe early.
        let _ = input
            .write_all(password.as_bytes())
            .and_then(|_| input.write_all(b"\n"));
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if cancel.load(Ordering::Acquire) || Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            query(&["device", "disconnect", iface])?;
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Wi-Fi activation cancelled",
            ));
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        std::thread::sleep(Duration::from_millis(80));
    };
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other("Wi-Fi activation failed"))
    }
}

#[cfg(test)]
#[path = "unit_tests/network.rs"]
mod tests;
