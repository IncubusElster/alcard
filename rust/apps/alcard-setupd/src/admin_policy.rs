//! Firstboot owns two narrowly named policy files, never group membership or
//! distribution policy. Changing a retry's choices replaces only our exact
//! rules for the account recorded by apply's progress marker.
use crate::failure::invalid;
use alcard_setup_protocol::{is_valid_username, Session, SetupPlan};
use std::{
    ffi::CString,
    fs::{self, File},
    io::{self, Read},
    os::{
        fd::AsRawFd,
        unix::{
            fs::{MetadataExt, PermissionsExt},
            process::CommandExt,
        },
    },
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

const DOAS: &str = "/etc/doas.d/alcard.conf";
const POLKIT: &str = "/etc/polkit-1/rules.d/49-alcard-admin.rules";

fn trusted_doas_config(regular: bool, uid: u32, mode: u32) -> bool {
    regular && uid == 0 && mode & 0o022 == 0
}

fn doas_configs() -> io::Result<Vec<PathBuf>> {
    // Alpine reads the mandatory main file before the *.conf directory. Do
    // not rewrite, copy or loosen permissions on an administrator's policy.
    let mut paths = vec![PathBuf::from("/etc/doas.conf")];
    let directory = Path::new("/etc/doas.d");
    check_directories(&directory.join("policy"))?;
    match fs::read_dir(directory) {
        Ok(entries) => {
            for entry in entries {
                let path = entry?.path();
                if path
                    .extension()
                    .is_some_and(|extension| extension == "conf")
                {
                    paths.push(path);
                }
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    for path in &paths {
        check_directories(path)?;
        let metadata = fs::symlink_metadata(path)?;
        if !trusted_doas_config(metadata.is_file(), metadata.uid(), metadata.mode()) {
            return Err(invalid("doas_config_untrusted"));
        }
    }
    Ok(paths)
}

fn doas_command() -> Command {
    let mut command = Command::new("/usr/bin/doas");
    command
        .env_clear()
        .env("LC_ALL", "C")
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    command
}

fn run_doas(mut command: Command) -> io::Result<(ExitStatus, Zeroizing<Vec<u8>>)> {
    let mut child = command.spawn()?;
    let result = (|| {
        let stderr = child.stderr.take().expect("piped doas stderr");
        // A parser error must not block firstboot on a full diagnostic pipe.
        // Bound both completion and retained bytes, and never log this text.
        let descriptor = stderr.as_raw_fd();
        // SAFETY: stderr owns a live descriptor throughout these fcntl calls.
        let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFL) };
        if flags < 0
            || unsafe { libc::fcntl(descriptor, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
        {
            return Err(io::Error::last_os_error());
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                return Err(invalid("doas_check_timeout"));
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let mut output = Zeroizing::new(Vec::new());
        stderr.take(4097).read_to_end(&mut output)?;
        if output.len() > 4096 {
            return Err(invalid("doas_check_output_invalid"));
        }
        Ok((status, output))
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

fn check_doas_syntax() -> io::Result<()> {
    for path in doas_configs()? {
        // -C drops to the caller's real UID. Firstboot is root, so this native
        // syntax-only check can read root0600 without changing any file mode.
        let mut command = doas_command();
        command.arg("-C").arg(path);
        if !run_doas(command)?.0.success() {
            return Err(invalid("doas_config_invalid"));
        }
    }
    Ok(())
}

fn doas_identity(username: &str) -> io::Result<(libc::uid_t, libc::gid_t, Vec<libc::gid_t>)> {
    let name = CString::new(username).map_err(|_| invalid("doas_account_invalid"))?;
    let mut entry = std::mem::MaybeUninit::<libc::passwd>::uninit();
    let mut storage = vec![0_u8; 16384];
    let mut found = std::ptr::null_mut();
    // SAFETY: all output buffers are valid and live; no borrowed passwd fields
    // escape this call. Account/group lookup happens before fork, not pre_exec.
    if unsafe {
        libc::getpwnam_r(
            name.as_ptr(),
            entry.as_mut_ptr(),
            storage.as_mut_ptr().cast(),
            storage.len(),
            &mut found,
        )
    } != 0
        || found.is_null()
    {
        return Err(invalid("doas_account_invalid"));
    }
    // SAFETY: getpwnam_r succeeded with a non-null result above.
    let entry = unsafe { entry.assume_init() };
    if !(1000..65534).contains(&entry.pw_uid) || entry.pw_gid == 0 {
        return Err(invalid("doas_account_invalid"));
    }
    let mut groups = vec![entry.pw_gid; 16];
    let mut count = groups.len() as libc::c_int;
    // SAFETY: count describes the writable groups array; name remains alive.
    if unsafe { libc::getgrouplist(name.as_ptr(), entry.pw_gid, groups.as_mut_ptr(), &mut count) }
        < 0
    {
        if !(1..=65536).contains(&count) {
            return Err(invalid("doas_groups_unavailable"));
        }
        groups.resize(count as usize, entry.pw_gid);
        // SAFETY: the resized array now has the requested capacity.
        if unsafe {
            libc::getgrouplist(name.as_ptr(), entry.pw_gid, groups.as_mut_ptr(), &mut count)
        } < 0
        {
            return Err(invalid("doas_groups_unavailable"));
        }
    }
    if count < 1 || count as usize > groups.len() {
        return Err(invalid("doas_groups_unavailable"));
    }
    groups.truncate(count as usize);
    Ok((entry.pw_uid, entry.pw_gid, groups))
}

fn password_route(code: Option<i32>, stderr: &[u8]) -> bool {
    // Exact Alpine OpenDoas shadow-auth path: after parsing ALL real config
    // and matching this identity, -n returns before authentication/command.
    code == Some(1) && stderr == b"doas: Authentication required\n"
}

fn check_doas_route(username: &str) -> io::Result<()> {
    doas_configs()?;
    let (uid, gid, groups) = doas_identity(username)?;
    let mut command = doas_command();
    command.args(["-n", "-u", "root", "/bin/ash", "-c", "exit 0"]);
    // SAFETY: only async-signal-safe identity syscalls run between fork/exec.
    // Drop supplementary groups and all real/effective/saved IDs to the actual
    // account. The installed setuid doas then applies its NORMAL policy path.
    unsafe {
        command.pre_exec(move || {
            if libc::setgroups(groups.len(), groups.as_ptr()) != 0
                || libc::setgid(gid) != 0
                || libc::setuid(uid) != 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let (status, output) = run_doas(command)?;
    if !password_route(status.code(), &output) {
        // A foreign nopass rule can only run the fixed harmless `exit 0`.
        // It is still not accepted as the password-based route selected here.
        return Err(invalid("doas_route_unavailable"));
    }
    Ok(())
}

fn doas_rule(username: &str) -> String {
    format!("permit persist {username} as root\n")
}

fn identities(username: &str, doas: bool, root: bool) -> io::Result<Vec<String>> {
    if !is_valid_username(username) || (!doas && !root) {
        return Err(invalid("no_admin_path"));
    }
    let mut users = Vec::new();
    if doas {
        users.push(format!("unix-user:{username}"));
    }
    if root {
        users.push("unix-user:root".into());
    }
    Ok(users)
}

fn polkit_rule(username: &str, doas: bool, root: bool) -> io::Result<String> {
    let identities = identities(username, doas, root)?
        .into_iter()
        .map(|identity| format!("\"{identity}\""))
        .collect::<Vec<_>>()
        .join(", ");
    // The protocol's canonical ASCII username grammar cannot inject JavaScript.
    // addAdminRule chooses authentication identities; it grants no action and
    // leaves the distribution's action authorization and PAM checks intact.
    Ok(format!(
        r#"// Managed by Alcard firstboot for {username}.
polkit.addAdminRule(function(action, subject) {{
    if (subject.user === "{username}" && subject.local && subject.active) {{
        return [{identities}];
    }}
}});
"#
    ))
}

fn safe_file(regular: bool, uid: u32, mode: u32, expected_mode: u32) -> bool {
    regular && uid == 0 && mode & 0o7777 == expected_mode
}

fn check_directories(path: &Path) -> io::Result<()> {
    for parent in path.ancestors().skip(1) {
        match fs::symlink_metadata(parent) {
            Ok(metadata)
                if metadata.is_dir() && metadata.uid() == 0 && metadata.mode() & 0o022 == 0 => {}
            Ok(_) => return Err(invalid("invalid_admin_policy_directory")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn read_policy(path: &Path, mode: u32) -> io::Result<Option<String>> {
    check_directories(path)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !safe_file(metadata.is_file(), metadata.uid(), metadata.mode(), mode) {
                return Err(invalid("invalid_admin_policy_file"));
            }
            fs::read_to_string(path).map(Some)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[derive(Debug, Eq, PartialEq)]
enum Change {
    Keep,
    Replace,
    Remove,
}

fn policy_change(
    current: Option<&str>,
    desired: Option<&str>,
    known: &[String],
) -> io::Result<Change> {
    if current.is_some_and(|value| !known.iter().any(|allowed| allowed == value)) {
        return Err(invalid("existing_admin_policy_conflict"));
    }
    Ok(if current == desired {
        Change::Keep
    } else if desired.is_some() {
        Change::Replace
    } else {
        Change::Remove
    })
}

struct ManagedFile {
    path: &'static str,
    mode: u32,
    previous: Option<String>,
    desired: Option<String>,
    change: Change,
}

impl ManagedFile {
    fn prepare(
        path: &'static str,
        mode: u32,
        desired: Option<String>,
        known: &[String],
    ) -> io::Result<Self> {
        let previous = read_policy(Path::new(path), mode)?;
        let change = policy_change(previous.as_deref(), desired.as_deref(), known)?;
        Ok(Self {
            path,
            mode,
            previous,
            desired,
            change,
        })
    }

    fn apply(&self) -> io::Result<()> {
        let path = Path::new(self.path);
        // Do not overwrite an administrator's edit made after preflight.
        if read_policy(path, self.mode)? != self.previous {
            return Err(invalid("admin_policy_changed_during_apply"));
        }
        match self.change {
            Change::Keep => Ok(()),
            Change::Replace => {
                let parent = path.parent().expect("fixed absolute policy path");
                ensure_directory(parent)?;
                crate::system_config::atomic_write(
                    path,
                    self.desired
                        .as_ref()
                        .expect("replacement content")
                        .as_bytes(),
                    self.mode,
                )
            }
            Change::Remove => {
                fs::remove_file(path)?;
                File::open(path.parent().expect("fixed absolute policy path"))?.sync_all()
            }
        }
    }
}

fn ensure_directory(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            // Validate this directory as well as its ancestors without following
            // directory symlinks. Existing distribution ownership/modes stay intact.
            return check_directories(&path.join("policy"));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let parent = path
        .parent()
        .ok_or_else(|| invalid("invalid_policy_path"))?;
    ensure_directory(parent)?;
    fs::create_dir(path)?;
    // A root umask of 077 must not hide new polkit rules from polkitd.
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
}

fn polkit_directory_readable(mode: u32, gid: u32, polkit_gid: Option<u32>, list: bool) -> bool {
    let access = if list { 0o005 } else { 0o001 };
    // Matching group bits take precedence over other bits in Unix DAC.
    let access = if polkit_gid == Some(gid) {
        access << 3
    } else {
        access
    };
    mode & access == access
}

fn polkit_directory_access() -> io::Result<()> {
    let passwd = fs::read_to_string("/etc/passwd")?;
    let polkit_gid = passwd.lines().find_map(|line| {
        let fields: Vec<_> = line.split(':').collect();
        (fields.first() == Some(&"polkitd"))
            .then(|| fields.get(3)?.parse::<u32>().ok())
            .flatten()
    });
    for directory in Path::new(POLKIT).ancestors().skip(1) {
        match fs::symlink_metadata(directory) {
            Ok(metadata) => {
                // Stock Alpine rules.d is root:polkitd 0750. Do not widen it;
                // require search access for polkitd and listing of rules.d.
                if !polkit_directory_readable(
                    metadata.mode(),
                    metadata.gid(),
                    polkit_gid,
                    directory == Path::new("/etc/polkit-1/rules.d"),
                ) {
                    return Err(invalid("polkit_cannot_read_admin_policy"));
                }
            }
            // New directories are created as root0755 when the policy is applied.
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

pub(crate) struct PreparedAdmin {
    doas: ManagedFile,
    polkit: ManagedFile,
    doas_account: Option<String>,
}

impl PreparedAdmin {
    pub(crate) fn prepare(plan: &SetupPlan, root_password: bool) -> io::Result<Self> {
        let username = &plan.account.username;
        let doas = plan.account.doas_enabled;
        let desired_polkit = polkit_rule(username, doas, root_password)?;
        if doas {
            // A policy alone cannot provide the replacement administrative
            // route when the distribution's setuid executable is unavailable.
            let path = Path::new("/usr/bin/doas");
            check_directories(path)?;
            let metadata = fs::symlink_metadata(path)?;
            if !metadata.is_file()
                || metadata.uid() != 0
                || metadata.mode() & 0o022 != 0
                || metadata.mode() & 0o4001 != 0o4001
            {
                return Err(invalid("doas_package_required"));
            }
            check_doas_syntax()?;
        }
        let doas_account = doas.then(|| username.clone());
        let rule = doas_rule(username);
        let doas = ManagedFile::prepare(DOAS, 0o600, doas.then(|| rule.clone()), &[rule])?;
        let known_polkit = [
            polkit_rule(username, true, false)?,
            polkit_rule(username, true, true)?,
            polkit_rule(username, false, true)?,
        ];
        let polkit = ManagedFile::prepare(
            POLKIT,
            0o644,
            (plan.session != Session::Console).then_some(desired_polkit),
            &known_polkit,
        )?;
        if plan.session != Session::Console {
            polkit_directory_access()?;
        }
        Ok(Self {
            doas,
            polkit,
            doas_account,
        })
    }

    pub(crate) fn apply(&self) -> io::Result<()> {
        self.doas.apply()?;
        self.polkit.apply()?;
        if let Some(username) = &self.doas_account {
            check_doas_route(username)?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "unit_tests/admin_policy.rs"]
mod tests;
