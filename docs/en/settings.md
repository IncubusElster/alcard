# Settings and system configuration

**Language:** English · [Русский](../ru/settings.md)

> Scope: current source tree · Updated: 2026-11-22

`alcard-settings` is the working Ratatui settings application. It is retained in Graphics, Graphics Minimal and Console. In the graphical application menu, **Settings** launches this same TUI through a `Terminal=true` desktop entry and Foot. A separate graphical Settings frontend is planned; the existing polkit window is an authentication agent, not that application.

```sh
alcard-settings
alcard-settings --help
```

`alcard-settings --hello` is the guarded first-boot wizard, described in [Installation](installation.md). It is not a way to reset or reconfigure an already completed installation. The TUI needs an interactive input and output terminal; 40×21 is the compact target.

## Navigation

| Context | Keys | Result |
| --- | --- | --- |
| List | Up/Down or Tab | Select a row |
| List | Enter/4; Space/5 also activates the selected row | Open, edit, choose or perform the row's action |
| List | 6 | Refresh the current page |
| List | Esc/8, Backspace or Shift+Tab | Return; exit from the top level |
| Text/password field | Ordinary characters, digits 4–8 and Space | Enter text, not softkey actions |
| Text/password field | Backspace | Delete the last character |
| Text/password field | Enter / Esc | Submit / discard the edit |
| Choice | Up/Down/Tab, then Enter/4 | Choose a value |
| Confirmation | Enter/4 / Esc/8 | Confirm / cancel |

The footer describes the current context. TUI Tab moves focus; it is not the graphical Shell's slot-6 alias. Shell F3/7 bindings also do not define TUI shortcuts. Ctrl+C exits or requests cancellation where supported. During an external authentication prompt, Ctrl+C belongs to the foreground command; the TUI restores its terminal screen afterward.

Inventory reads and most service actions run outside the render loop. A pending read can be left with Back. Wi-Fi connection and Bluetooth pairing have explicit cancellation paths; Back does not promise to undo every already submitted system operation. Failed or unavailable services are shown as such, rather than as an invented disabled state.

## The eight sections

| Section | Implemented rows and actions |
| --- | --- |
| System | Edit hostname; view current user, selected session, Alpine version and kernel version |
| Language and keyboard | One system language; retained layouts; fixed US boot layout; graphical switching hint; active console layout when running on a numbered Linux VT |
| Date and time | Timezone, NTP on/off, local date/time; manual time is editable only with NTP disabled |
| Network | Wi-Fi power, Ethernet/Wi-Fi interfaces, nearby networks, connection state and saved profiles |
| Bluetooth | Adapter power, discovery, device details, pairing, connection/disconnection and removal |
| Display and power | Available brightness control and battery percentage; Shell idle and battery-policy preferences when Shell is installed |
| Sound | Current default output volume and mute; choose an available PipeWire audio sink |
| Appearance | Classic/Emerald, accent colour and solid/outline icons when Shell is installed |

There is no “Alcard version” row. Account creation/password administration belong to first boot; installed-mode switching, factory reset and a package manager are not current Settings features.

## System, language and keyboard

Hostname accepts 1–63 ASCII letters, digits or hyphens, with no leading or trailing hyphen. The backend changes the running hostname and `/etc/hostname`.

The system language choices are English, Russian and Spanish. The backend writes the validated locale into `/etc/profile.d/alcard-locale.sh`. Settings reloads its language after an operation and requests a reload of the invoking user's installed Shell. The Shell/shared configuration loader honours an explicit CLI locale first, then the system language, then the older per-user locale field or environment/default fallback. Normal Settings exposes one system-language setting, not competing per-application language selectors. Already-running third-party applications may need a new login/start to inherit the environment.

Settings, Shell, locker and polkit-agent share `alcard_l10n::Locale`. Automatic fallback uses the first non-empty variable in `LC_ALL`, `LC_MESSAGES`, `LANG` order, recognizes supported language prefixes without case sensitivity and falls back to English for an unsupported value. An explicit system/CLI language still takes precedence. The JSON catalogs and application-specific labels retain EN/RU/ES text.

Original US cannot be removed and stays first. RU/ES/DE can be enabled or disabled. Their letter mappings change while the original symbol layer is retained. The saved selection is `/etc/alcard-keyboards`; generated XKB settings are in `/etc/alcard/keyboard-xkb.env`. Multiple graphical layouts use Alt+Shift after the next login. The boot console remains US. On a numbered Linux VT, Settings can temporarily load one of the retained console layouts and records it in `/run/alcard-active-layout`; a terminal emulator does not expose this VT-only row. See [Fonts and keyboard](fonts-keyboard.md).

## Date and time

Timezone is an installed IANA zone such as `America/Montevideo`. The backend validates the name and zone file before updating `/etc/localtime` and `/etc/timezone`.

NTP uses the installed OpenRC service, preferring `chronyd` and falling back to `ntpd`. Toggling it changes both the running service and its default-runlevel registration. Manual local time uses the exact format `YYYY-MM-DD HH:MM:SS`, is validated by `date`, and is rejected while either NTP service is enabled/running. If `/dev/rtc0` exists, successful manual time setting also writes the hardware clock in UTC.

## NetworkManager

NetworkManager owns Wi-Fi/Ethernet connections, DHCP, saved profiles and network secrets in Hello and installed Settings. Inventory and profile operations use fixed `/usr/bin/nmcli` with structured arguments, not command-shell interpolation. Cancellable Wi-Fi activation uses the system D-Bus API. The underlying Wi-Fi implementation is NetworkManager's responsibility; Alcard no longer starts a separate standalone `wpa_supplicant`/DHCP workflow alongside it.

The Network page lists saved profiles, Wi-Fi power when a Wi-Fi device exists, and Ethernet/Wi-Fi interfaces. An interface page can activate or disconnect it and shows available GENERAL/IP4/IP6 details. Nearby Wi-Fi entries show SSID, signal and security; identical SSIDs are deduplicated. Unsupported/hidden empty SSIDs are not fabricated into selectable networks. An active scan denied by policy can fall back to NetworkManager's available AP cache.

For a protected network with already sufficient permissions, the TUI collects a masked password. An operation-local D-Bus secret agent supplies it only to the expected NetworkManager owner and selected connection. Existing compatible profiles are retained; a new connection uses NetworkManager's profile creation path. If elevated permissions are needed, the row confirms the operation and lets the terminal authorization/nmcli prompt obtain the required input. Account and Wi-Fi passwords are not placed in command arguments. Failure is reported; successful activation is not an Internet-connectivity test.

The cancellable client tracks the exact `ActiveConnection` returned by its activation request and the unique service owner. Cancellation or timeout revokes its pending secret and requests deactivation of that object; it never disconnects by interface or saved-profile UUID. An already connected matching network is left active. A delayed reply can be cleaned up by a bounded background operation. If NetworkManager has not returned the handle or acknowledged cleanup, the caller receives an explicit unconfirmed-cancellation result rather than a guarantee that the connection stopped. A restarted daemon or a different connection is not a substitute cancellation target.

Starting a new activation may replace the previous connection; cancellation does not restore it. This ownership mechanism applies to shared `network::connect_until` callers, including Hello and the ordinary authorized asynchronous Wi-Fi operation. The elevated foreground `nmcli` path continues to use terminal input and its own command lifecycle.

Saved Ethernet/Wi-Fi profiles are selected by UUID. The profile page supports:

- activation and autoconnect on/off;
- an IPv4 address with prefix, gateway and IPv4 DNS values;
- switching back to DHCP;
- forgetting a profile after confirmation.

Setting an address selects manual IPv4. Returning to DHCP clears the manual address/gateway/DNS and restores automatic DNS. After editing, explicitly activate the profile to apply it to the device. IPv6 information is visible, but a full IPv6 editor, VPN and enterprise Wi-Fi configuration are not implemented in this UI.

The upgrade hook imports only the known single Wi-Fi profile format created by older Alcard Hello versions. It retains original files, enables `dbus` and `networkmanager`, and removes the old `networking`/standalone `wpa_supplicant` runlevel ownership for the next boot. Unrecognised legacy data is rejected instead of guessed. This is a compatibility path, not the current connection backend.

## Bluetooth

Settings uses BlueZ: power, bounded discovery and device actions through `bluetoothctl`, with a dedicated D-Bus agent for pairing. The device page shows address, name, paired/connected state, and the relevant pair/connect/disconnect/remove actions. Removing a device requires confirmation.

The pairing agent supports PIN, passkey and numeric confirmation, binds requests to the selected device and the unique BlueZ service owner, and unregisters when finished. Daemon `Cancel` closes the outstanding prompt and resolves its pending callback; `Release` also ends the registered session. User cancellation requests `CancelPairing`; it does not claim to revoke an already completed pairing. Pairing and connection are distinct operations. A missing controller/service or denied permission is an unavailable result. Physical Bluetooth interoperability is still an acceptance item.

Interactive rejections and cancellations use the BlueZ `Rejected` and `Canceled` error names. A cancellation queued during agent registration prevents a subsequent `Pair` call. Discovery and registration still use the existing per-method timeout of 90 seconds; they do not yet have immediate cancellation, and connection establishment has no separate global deadline. These pre-pair responsiveness limits are distinct from closing an active pairing prompt.

The shared installed Bluetooth configuration disables automatic adapter power-on (`AutoEnable=false`). Shell quick settings can list/toggle radios but do not contain this pairing UI; see [Desktop](desktop.md).

## Display, power and sound

Settings exposes brightness when `brightnessctl` can read a backlight, and shows battery capacity from sysfs when available. The privileged TUI brightness action accepts 0–100%. This differs from Shell quick settings, whose relative brightness control keeps a nonzero lower limit. Do not infer an automatic-lock guarantee from setting brightness to zero.

When Shell is installed, the page edits idle-lock time, screen-off time, low/critical battery thresholds and opt-in automatic suspend. These are Shell policies; simply retaining the package in Graphics Minimal does not start them. Their validation, defaults and safeguards are documented in [Power and audio](power-audio.md).

Sound uses the current user's PipeWire/WirePlumber service. It lists Audio/Sink outputs, identifies the default when available, accepts volume 0–100%, toggles mute and sets the chosen sink as default. Output IDs are checked against a fresh list. There is no full route/profile editor. The Graphics session starts the audio stack; Minimal and Console do not start it automatically. Their Sound page depends on separately available services and does not silently launch daemons. Volume is a software control, not a measured speaker level or guarantee about codec gain.

## Preferences and authorization

Appearance and Shell policy settings are stored in `$XDG_CONFIG_HOME/alcard/shell.toml`, normally `~/.config/alcard/shell.toml`. Saving uses a per-file lock, validates the whole candidate, writes a private temporary file, then atomically replaces the target and syncs the directory. An invalid existing configuration is reported rather than overwritten as if it were valid. Successful writes request SIGHUP only for matching `/usr/bin/alcard-shell` processes owned by the caller, with pidfds protecting against PID reuse.

System mutations use the root-only `alcard-setupd --setting KEY VALUE` allowlist. This path cannot perform firstboot account provisioning. The TUI temporarily leaves alternate-screen/raw mode for doas or su; NetworkManager operations similarly request elevation when its reported permissions require it. Permission to perform an operation is still enforced by the target backend. User-controlled values are separate arguments; the fixed su wrapper preserves argument boundaries.

The shared core currently exposes a terminal command adapter. A future GUI must add an appropriate graphical authorization adapter around the same operations, using the existing polkit infrastructure. It must not duplicate the settings rules or run the TUI as its business backend. [Security](security.md) describes the process boundaries.

## Source and verification

- [TUI browser](../../rust/apps/alcard-settings/src/settings.rs), [desktop entry](../../scripts/hello/alcard-settings.desktop).
- [Shared row/action model](../../rust/crates/alcard-settings-core/src/settings.rs), [NetworkManager client](../../rust/crates/alcard-settings-core/src/network.rs), [pairing](../../rust/crates/alcard-settings-core/src/pairing.rs).
- [Preferences](../../rust/crates/alcard-settings-core/src/preferences.rs), [shared language resolution](../../rust/crates/alcard-l10n/src/locale.rs), [privileged settings](../../rust/apps/alcard-setupd/src/settings.rs), [shared configuration writes](../../rust/apps/alcard-setupd/src/system_config.rs), [legacy network migration](../../rust/apps/alcard-setupd/src/network_upgrade.rs).

Current NetworkManager/TUI runtime operations, cancellation, authentication routes and persistence require focused installed-system acceptance. Parser, argument and rendering tests do not establish all service behavior. See [Testing](testing.md) and [Current status](status.md).
