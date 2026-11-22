# Authentication, session locking and recovery

**Language:** English · [Русский](../ru/security.md)

> Scope: current source tree · Updated: 2026-11-22

Alcard keeps login, administrative authorization and screen unlock separate. Shell presents actions and observes lifecycle events; it does not receive passwords or decide whether a system operation is authorized. For reporting a vulnerability, follow the repository [security policy](../../SECURITY.md). The contracts below describe the current implementation, not a completed hardware security assessment.

## Process and authority boundaries

| Purpose | Component | Actual authority |
| --- | --- | --- |
| Initial account and machine provisioning | `alcard-settings --hello` and root `alcard-setupd` | Guarded first-boot backend, validated plan and separate stdin credentials |
| Login | greetd/agreety with Alcard's login PAM profile | Distribution authentication/account modules and required `pam_elogind` session registration |
| Installed terminal settings | Settings TUI and setupd's settings allowlist | doas or su in the terminal; target backend permissions |
| Graphical administrative challenge | Separate unprivileged `alcard-polkit-agent` | polkit Authority, native authentication helper and distribution PAM |
| Screen unlock | Separate unprivileged `alcard-lock` | Compositor `ext-session-lock-v1` and PAM for the current real UID |
| Radio mutation/discovery requiring privilege | Fixed-operation radio helper | Installed pkexec action policy and polkit |
| Session power operations | Shell login1 client | Verified current session, elogind policy and inhibitors |

The future graphical Settings frontend must reuse the shared settings operations with an appropriate graphical authorization adapter. The existing polkit form is not a Settings frontend. Conversely, the current TUI's doas/su path is intentional and is not routed through an invisible graphical password prompt.

## First boot and administrative access

Hello accepts a normal user's password, optional doas access and an optional root password. At least one administrative route is required:

| doas selected | Root password supplied | Resulting administrative identities |
| --- | --- | --- |
| Yes | No | Hello user; root password is locked |
| Yes | Yes | Hello user and root |
| No | Yes | Root |
| No | No | Invalid plan; no administration route |

Credentials are transmitted separately from the nonsecret setup plan through stdin, not command arguments, logs or progress markers. The privileged backend revalidates the plan, machine state, requested session and administrative files before creating an account or changing a password. It sets the user's password before creating an administrative route. A replacement root route is established before a retry can revoke doas; root is locked only after a functioning doas policy exists. First boot is a guarded resumable operation, not a transactional rollback of every system change.

Managed files are `/etc/doas.d/alcard.conf` (root, 0600) and, for graphical installation, `/etc/polkit-1/rules.d/49-alcard-admin.rules` (root, 0644). Parent directories, ownership, modes and file types are checked; symlinks, unsafe paths and foreign policy content are rejected before account mutation. A retry reconciles only Alcard's exact known policy for the same progress account. Alcard does not replace an arbitrary administrator's rules.

The doas rule requires the user's password. Validation reads all applicable trusted doas configuration, checks syntax and probes the real account with its actual UID, GID and supplementary groups. The noninteractive probe is the fixed harmless command `doas -n -u root /bin/ash -c 'exit 0'`. Only the expected “authentication required” outcome establishes the selected password route; nopass, denial or unexpected output does not. The probe has a five-second deadline and bounded stderr, which is not published as a user-facing diagnostic.

The generated polkit `addAdminRule` applies to the requesting Hello user in an active local session and selects the configured identities. It does not return an authorization grant, bypass PAM or add broad device-access groups. Action-specific distribution policy still decides whether authentication is needed and whether the operation is allowed. Setup failures expose a fixed stage and allowlisted cause/error kind, not raw credential-bearing subprocess output.

## Real login and service ownership

tty1 has one dispatcher. Before completion it launches Hello; afterward it starts greetd when the graphical login stack is installed, otherwise getty. There is no competing standalone greetd OpenRC service. The serial console remains a getty route after Hello. The dedicated `greeter` account is locked and noninteractive; it is not the logged-in user.

User login uses `/etc/pam.d/alcard-greetd`, with distribution base auth/account/password/session modules and **required `pam_elogind.so type=wayland`** before the session launcher. Greeter authentication has its own distribution service. Launchers require a normal user, a private real `XDG_RUNTIME_DIR` and a valid seat/session; they use `LIBSEAT_BACKEND=logind`. An environment string such as `XDG_SESSION_ID` cannot manufacture an authenticated login.

OpenRC owns system D-Bus, device and cgroup prerequisites. Elogind uses its vendor system-D-Bus activation; there is no second competing elogind supervisor. Both graphical modes require the correct PAM/runtime contract even though Minimal does not run the full Shell supervisor. Detailed installation and mode ownership are in [Installation](installation.md).

## Graphical polkit agent

`alcard-polkit-agent` registers the standard AuthenticationAgent for its own real Unix login session. It pins the unique system-bus owner of polkit Authority and accepts authentication calls only from that owner. GIO/GLib and native `PolkitAgentSession` use the trusted distribution helper for the PAM conversation. The GUI has no equivalent of “grant permission” based on form state.

One worker owns native sessions, while the main thread owns the Wayland surface. Callbacks enqueue events; session destruction and D-Bus invocation completion happen after callback return. Replies carry a request ID and fresh prompt serial, are single-use, and cannot answer a later prompt. One allowed identity is selected automatically; several require explicit selection. Exact Authority indices and numeric UIDs are retained. Unknown, duplicate, empty or oversized identity sets fail rather than silently selecting root.

The active request uses a 320×170 layer-shell overlay with keyboard focus only while displayed. It shows the action, reason, permitted identity, PAM prompt and error. F1 opens bounded scrollable details; arrows select an identity or scroll. Shared Classic/Emerald themes, EN/RU/ES, Caps Lock and XKB-group indicators are available. Enter selects/submits or explicitly retries, Esc cancels, Backspace removes a character and Ctrl+U clears. Digits 4–8 are text during input, and held/repeated keys cannot confirm after focus arrival.

Wrong authentication permits explicit retry, with at most three attempts and five minutes per request; PAM/polkit can be stricter. Loss of focus, keyboard, output, surface, login session or Authority cancels unfinished work. Cancellation does not retrospectively revoke an authorization already issued by Authority. There is no hidden TTY fallback.

All PAM answers are masked, including echo-on prompts and one-time codes. Clipboard paste and text-input/IME are not implemented; characters must be entered through the active XKB layout. The secret buffer allows 512 bytes/128 Unicode characters, has no Clone/Debug representation, and clears removed bytes and the complete buffer on submission, cancellation and destruction. The renderer sees only mask length. Production disables core dumps and dumpability and rejects `POLKIT_DEBUG`, `G_DBUS_DEBUG` and `WAYLAND_DEBUG`. It deliberately preserves the normal privilege transition needed by the trusted helper; `NO_NEW_PRIVS` is not set for this path.

For a direct diagnostic start inside an existing ordinary-user Wayland login session:

```sh
alcard-polkit-agent --locale ru --theme classic
```

The agent needs system/session D-Bus, polkitd, PAM/helper, real elogind/logind registration, private runtime storage and layer-shell. Do not run it through sudo. A second agent does not replace the registered one. A build container without a login session can run unit tests; production registration there correctly fails.

While active, the agent owns `org.alcard.Authentication` on the session bus. Shell uses presence only to suppress banners while preserving notification history. Spoofing this presentation signal can disrupt display but cannot authorize an operation. The layer overlay is not a secure-attention mechanism against a compromised compositor.

## Session lock and current-user PAM

Quick settings' eighth row starts Lock with 4/Enter and no extra confirmation. Shell resolves the production locker next to its own executable, not from PATH or an override helper variable. The same locker handles manual, idle and sleep-related locking. Direct launch inside the current user's Wayland session is also supported:

```sh
alcard-lock
alcard-lock --locale ru --theme classic
```

The locker rejects root, setuid/setgid execution and mismatched real/effective identity. It validates the fixed `/etc/pam.d/alcard-lock` service: a nonempty root-owned regular file, no group/other write access and no symlink/FIFO. Packaging installs the distribution-appropriate profile as 0644. Alpine uses `base-auth`/`base-account`; Ubuntu/Debian uses `common-auth`/`common-account`.

The client requests **`ext-session-lock-v1`**, creates opaque lock surfaces for all current outputs and covers new outputs on hotplug. Authentication starts only after the compositor's `locked` acknowledgement. The shared SHM implementation bounds a surface to 4,096 pixels per side and 4,194,304 total pixels; unsupported sizes or other failures must not create an unlock route. Process death, missing protocol, output failure, timeout or PAM denial does not send unlock. After the compositor has acknowledged lock, a locker crash leaves an abandoned lock according to the protocol.

On the locked splash, 4/Enter opens password entry. Digits 4–8, Space and letters are password text; Enter submits, Backspace deletes and Ctrl+U clears. Esc cancels only the current authentication attempt and returns to the still-locked splash. Focus loss also cancels the attempt. All PAM responses remain masked.

A separate child of the same binary handles PAM over bounded anonymous pipes. The service is fixed as `alcard-lock`; username, password and a selectable service are absent from command arguments/environment. It requires both `pam_authenticate(PAM_DISALLOW_NULL_AUTHTOK)` and `pam_acct_mgmt`, then verifies that resulting `PAM_USER` resolves to the same real UID. It does not open a new login session, change passwords or accept another administrator's password as the session owner's identity.

Secrets use the same 512-byte/128-character limit and explicit clearing discipline. Each attempt/prompt has its own generation/serial. A cancelled reply or old success cannot unlock a newer attempt. Authentication has a 120-second deadline; cancellation/timeout ends only its own PAM process group and leaves the session locked. Core dumps and dumpability are disabled; the distribution's trusted `unix_chkpwd` privilege transition remains available. `WAYLAND_DEBUG` is forbidden.

Only current successful PAM permits `unlock_and_destroy`. The locker reports completion after a separate Wayland sync acknowledgement. Shell receives only lifecycle events (`FAILED` before locking, `LOCKED`, `UNLOCKED`), never secrets or unlock authority. Shell dying does not kill the independent locker. Session-bus name `org.alcard.SessionLock` suppresses Home/banners as an advisory signal; losing or spoofing it cannot unlock the compositor.

Production has no demo password, bypass, helper override, capture mode or IPC unlock command. `--pam-worker` is an internal entry to the real PAM backend, and cannot unlock without the GUI's session-lock object. Test-only empty-form SHM captures are distinct from compositor screenshots and do not establish that screencopy is allowed while locked.

## Crash recovery and session loss

Recovery uses the shared `alcard-lock-state` journal, not a PID file pretending that a lock succeeded. Its identity comes from peer credentials on the **connected Wayland socket**, including compositor UID, PID and process start time. The file is `$XDG_RUNTIME_DIR/alcard-lock-v1-<pid>-<start>.state`, in a user-owned 0700 runtime directory. It is a 0600 regular single-link file with checked inode/path identity; symlinks, replacement and unsafe ownership are rejected.

A live locker holds an exclusive lease on the same inode. It records intent before requesting lock; a crash releases the kernel lease while leaving intent. Shell observes changes through inotify and can restart an abandoned locker with backoff from one to 30 seconds. A living locker is preserved across Shell restart. The replacement must acquire its own protocol lock and authenticate with fresh PAM. Only the locker clears intent after authenticated unlock and Wayland sync; unknown or corrupt state is never evidence of unlock. A new compositor lifetime has a separate identity.

The Graphics session supervisor owns Shell, polkit and its optional audio stack, and checks PID start times before signalling its own children. Recovery behaviour is explicit:

| Failure | Behaviour |
| --- | --- |
| Shell exits | Restart with backoff; do not kill an existing locker |
| Locker exits after lock | Preserve compositor lock; start replacement through the abandoned-intent path, requiring fresh PAM |
| Agent/polkit Authority failure only | Cancel the challenge; restart/re-register for the same valid login session |
| Real login binding, elogind, system bus or private session bus is lost | Agent reports SessionLost with exit 75; end the session in a controlled way and return to greetd for a new PAM login |
| Audio stack fails | Report failure and leave desktop usable; retry the stack on a fresh login |

Recovery does not fabricate a replacement session ID or reuse stale authentication. Graphics Minimal intentionally has a smaller Foot lifecycle and does not inherit every full-Graphics monitoring guarantee.

## Limits, source and verification

Explicit secret clearing reduces exposure in Alcard's owned buffers; it does not prove removal of every copy in PAM, libraries, the kernel or swap. Root or a compromised compositor is outside the protection offered by these user-session clients. Native protocol acknowledgement, successful PAM and real login binding remain necessary; service names, journal presence, process existence and screenshots are not substitutes.

Source tests and isolated native/VM scenarios validate different layers. Native test harnesses provision temporary users/PAM services only inside disposable containers or VMs. Never adapt those scripts into host-account tests. Hardware unlock, focus isolation, actual suspend/resume and recovery remain separate acceptance work; [Testing](testing.md), [Hardware](hardware.md) and [Current status](status.md) preserve those boundaries.

Source: [administration policy](../../rust/apps/alcard-setupd/src/admin_policy.rs), [provisioning](../../rust/apps/alcard-setupd/src/apply.rs), [safe failure protocol](../../rust/apps/alcard-setupd/src/failure.rs), [agent registration/PAM](../../rust/apps/alcard-polkit-agent/src/native.rs), [locker PAM](../../rust/apps/alcard-lock/src/authentication.rs), [locker protocol lifecycle](../../rust/apps/alcard-lock/src/gui.rs), [lock journal](../../rust/crates/alcard-lock-state/src/lib.rs), [Shell recovery](../../rust/apps/alcard-shell/src/locking.rs), [session supervisor](../../scripts/wayland/session-client), [login PAM](../../scripts/pam/alcard-greetd.alpine).
