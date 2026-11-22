//! Bounded service commands shared by inventory and settings operations.
use super::BT;
use crate::network;
use std::{
    fs, io,
    process::{Command, Stdio},
};

pub(super) fn file(path: &str) -> String {
    fs::read_to_string(path)
        .unwrap_or_default()
        .trim()
        .to_owned()
}
pub(super) fn output(program: &str, args: &[&str]) -> io::Result<String> {
    let result = Command::new("/usr/bin/timeout")
        .args(["-k", "2", "20", program])
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .output()?;
    if !result.status.success() {
        return Err(io::Error::other(format!(
            "{}: {} {}",
            program,
            result.status,
            String::from_utf8_lossy(&result.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&result.stdout).trim().into())
}
pub(super) fn bt(args: &[&str]) -> io::Result<String> {
    let text = output(BT, args)?;
    if text.contains("No default controller available")
        || text.lines().any(|l| l.trim_start().starts_with("Failed "))
    {
        return Err(io::Error::other(text));
    }
    Ok(text)
}
pub(super) fn nm(args: &[&str]) -> io::Result<String> {
    network::query(args).map(|s| s.trim_end().to_owned())
}
pub(super) fn sinks() -> io::Result<Vec<(String, String)>> {
    let text = output("/usr/bin/pw-dump", &[])?;
    let nodes: Vec<serde_json::Value> = serde_json::from_str(&text)?;
    Ok(nodes
        .iter()
        .filter_map(|node| {
            let props = node.pointer("/info/props")?;
            if props.get("media.class")?.as_str()? != "Audio/Sink" {
                return None;
            }
            let name = props
                .get("node.description")
                .or_else(|| props.get("node.nick"))
                .or_else(|| props.get("node.name"))?
                .as_str()?;
            Some((node.get("id")?.as_u64()?.to_string(), name.to_owned()))
        })
        .collect())
}

pub(super) fn network_admin(required: &[&str]) -> bool {
    if unsafe { libc::geteuid() } == 0 {
        return false;
    }
    let Ok(text) = nm(&["-t", "-f", "PERMISSION,VALUE", "general", "permissions"]) else {
        return true;
    };
    !required.iter().all(|r| {
        text.lines()
            .any(|line| line == format!("org.freedesktop.NetworkManager.{r}:yes"))
    })
}
