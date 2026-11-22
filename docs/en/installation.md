# Installation, first boot and login

**Language:** English · [Русский](../ru/installation.md)

> Scope: current source tree · Updated: 2026-11-22

Alcard targets Alpine 3.24 aarch64 on Cardputer Zero. The source implements a full initial image and three installation modes. Physical Cardputer boot, display, keyboard and device acceptance remain open; a successful package or VM check is not a hardware result. Use [Building](building.md) for the complete build sequence and [Boot and updates](boot-updates.md) for the disk layout and recovery limitations.

## Initial image and available modes

The initial image contains Hello/Settings, console fonts and keyboard layouts, the hardware profile, both graphical session profiles, and the services needed by them. The retired `ALCARD_WITH_WAYLAND` switch is not required. Hello chooses which installed mode to retain as the default.

| Hello choice | Setup plan value | `/etc/alcard/default-session` | Login and session |
| --- | --- | --- | --- |
| Graphics | `graphics` | `alcard-shell` | greetd/agreety → PAM/elogind → labwc/Pixman → supervised Shell and polkit-agent; independent locker |
| Graphics Minimal | `graphics_minimal` | `labwc` | greetd/agreety → PAM/elogind → labwc/Pixman with fullscreen Foot |
| Console | `console` | `console` | getty/login → ordinary login shell; optional graphical/login packages removed |

Hello initially selects the first available mode in this order: Graphics, Graphics Minimal, Console. A user's explicit available choice survives a refresh of the checks. Unavailable choices cannot be submitted. The frontend and privileged backend independently check installed packages, executables, ownership, modes and the required PAM components. The backend repeats the check before changing accounts and before saving the selected session.

The older serialized plan value `graphical` still means Graphics Minimal. A saved `labwc` value still launches Foot; an upgrade does not silently reinterpret it as Shell. `start-session` accepts only the three saved values above. A missing selection defaults to Console; an unknown value is rejected.

## Hello lifecycle

On a fresh image, `tty1-dispatch` invokes `firstboot-init`, which arms the first boot through `alcard-setupd --arm-firstboot` and runs:

```sh
alcard-settings --hello
```

The canonical flag is `--hello`; `--firstboot` remains an alias. Ordinary `alcard-settings` opens the installed settings browser. `--demo`, a separate demo home, and a factory-reset entry point are not current CLI features.

Hello contains six steps:

1. **Welcome:** system language EN/RU/ES and keyboard selection. Original US is mandatory and first; RU, ES and DE are optional additions.
2. **Account and security:** username, display name, user password and confirmation, doas choice, optional root password and confirmation. At least one administrative route must remain available.
3. **Network:** Ethernet state, Wi-Fi discovery and optional connection through NetworkManager. Skipping network setup is supported.
4. **Date and time:** timezone and optional NTP.
5. **Session:** one of the three available installation modes.
6. **Review and finish:** a summary without secrets, followed by a real apply operation.

The first-boot launcher loads the original US console keymap before account/password input on tty1. Selecting future layouts does not change that input mapping while creating the account. See [Settings](settings.md) and [Fonts and keyboard](fonts-keyboard.md) for normal post-install language/layout behavior.

The backend keeps its state in `/var/lib/alcard/`: `hello.pending` records an armed setup, `hello.progress` identifies resumable account provisioning, and `hello.complete` marks successful completion. Completion is written only after provisioning and session finalization succeed. The launcher retries while setup remains pending. A partially completed apply can be resumed, but it is not a transaction with rollback: account/password changes may already have succeeded before a later failure.

Automatic arming is restricted to a fresh system without a usable administrator or ordinary user, or an existing valid pending setup. Explicit root `--hello` can also initialize an uncompleted Alpine installation with an existing root password, provided no ordinary user already exists. It cannot reconfigure an already completed installation. Do not delete markers to simulate a reset; authenticated factory reset is a separate planned feature.

## What Console removes and retains

Before completing Console setup, the backend removes installed members of this fixed list with `apk del`:

```text
alcard-desktop-shell
alcard-shell
alcard-polkit-agent
alcard-lock
alcard-desktop-minimal
alcard-fonts-gui
greetd-agreety
greetd
```

It verifies that all eight direct packages are absent, lets APK resolve unused dependencies, and then saves `console`. It does not manually prune shared libraries or the BSP. An interrupted retry skips packages already removed. Only a known dangling elogind default-runlevel link is removed by this cleanup.

Hello/Settings, console fonts, keyboard, account tools, device management, system D-Bus and NetworkManager remain available. BlueZ is also a dependency of the console-capable Hello/Settings package. Removing the graphical stack therefore does not remove normal Settings or networking. The `alcard-hello` package does not depend on greetd, labwc, Foot or Shell.

Both graphical choices retain the full installed package set. Graphics Minimal preserves Shell packages for inspection and repair, although its own profile does not depend on Shell. Choosing the fallback does not perform a reset, and a failed Graphics session does not automatically switch to it.

## Who owns login and the session

`tty1-dispatch` is the sole inittab owner of tty1: pending Hello first, then greetd/agreety when installed, otherwise getty. A separate greetd OpenRC service must not compete for the same VT. Agreety runs the fixed `/usr/libexec/alcard/start-session`; arbitrary commands are not read from the saved selection.

Graphical user login uses the `alcard-greetd` PAM service. Distribution auth/account/password checks remain in place; required `pam_elogind.so type=wayland` opens the actual graphical login session before the launcher starts. The greeter uses the distribution `greetd` PAM service. The unprivileged `greeter` account has a locked password, `/sbin/nologin` and no password aging/account expiry.

The graphical launchers require a non-root user and a private `XDG_RUNTIME_DIR` established by the real login stack. They select `LIBSEAT_BACKEND=logind` and `WLR_RENDERER=pixman`. OpenRC owns system D-Bus, cgroups and device services; elogind uses its vendor D-Bus activation path. An installed but inactive `elogind-openrc` subpackage is acceptable; a competing elogind supervisor is not. [Security](security.md) documents administration, authentication and recovery boundaries.

The main Graphics profile uses a private session bus, `shell-session` and `session-client`. Closing a launched Foot window does not end that session. Graphics Minimal starts Foot as the compositor's session client; exiting that Foot ends labwc and returns to login. Minimal does not provide the main profile's service-loss supervision.

Daemon and early failed-PAM stderr are redirected to the root-only volatile `/run/alcard-greetd/daemon.log`; greeter/user descriptors return to the VT. The file is truncated on a new daemon start, not kept under a permanent byte cap. Treat it as local diagnostic data, not a public artifact.

## Graphics Minimal and diagnostic recovery

Minimal provides fullscreen Foot with Alcard Pixel Regular/Bold TTF, no font antialiasing, zero padding and a dark palette. The target at 320×170 is 40×21 cells with two spare vertical pixels. The configured size and generated font metrics do not by themselves prove exact physical cell rendering.

These read-only checks are useful in an installed graphical environment:

```sh
foot --check-config --config=/etc/alcard/foot.ini
fc-match 'Alcard Pixel:style=Regular:pixelsize=8'
```

For diagnosis in an already configured isolated Alpine guest, root can select the installed fallback:

```sh
mkdir -p /etc/alcard
printf 'labwc\n' > /etc/alcard/default-session
chmod 644 /etc/alcard/default-session
```

Log out and log in again through the real VT greeter. This changes a fixed login target only: it neither installs missing packages nor reruns Console cleanup. A user-facing installed-mode switch in Settings is not implemented. Use an available diagnostic VT or the guest's serial console to repair a broken selection; graphical launchers are not intended to run as root.

The development root-password option belongs to image construction and has different first-boot behavior. See [Building](building.md); do not treat a diagnostic image as a normal release image.

## VM and hardware boundaries

The serial QEMU firstboot runner uses `ttyAMA0`, where the dispatcher always uses getty after Hello, regardless of the saved graphical choice. It can check firstboot and persistence but does not exercise agreety or a graphical VT login. The separate real-login VM uses VT1, virtio DRM and keyboard input to exercise actual greetd/PAM/elogind.

Use the current CI result for the source and package set being installed. Installed-package, three-mode VM and physical Cardputer acceptance have separate scopes. Physical installation still needs boot, display, keyboard, login/logout and device verification. See [Testing](testing.md) and [Current status](status.md) for the relevant gates.

## Source map

- [CLI and Hello](../../rust/apps/alcard-settings/src/main.rs), [firstboot apply boundary](../../rust/apps/alcard-settings/src/firstboot.rs).
- [Provisioning and mode finalization](../../rust/apps/alcard-setupd/src/apply.rs), [session runtime validation](../../rust/crates/alcard-setup-protocol/src/session_runtime.rs).
- [tty1 dispatcher](../../scripts/hello/tty1-dispatch), [firstboot launcher](../../scripts/hello/firstboot-init), [fixed session dispatcher](../../scripts/hello/start-session).
- [Graphics launcher](../../scripts/wayland/shell-session), [supervisor](../../scripts/wayland/session-client), [Minimal launcher](../../scripts/wayland/wayland-session).
