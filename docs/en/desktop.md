# Desktop Shell and interaction

**Language:** English · [Русский](../ru/desktop.md)

> Scope: current source tree · Updated: 2026-11-22

The Graphics session runs `alcard-shell` under labwc. Shell provides four persistent screens: **Home, Recent, Notifications and Menu**. Quick settings, application options and dialogs are subordinate views. Graphics Minimal runs Foot instead; Console has no Wayland session. See [Installation](installation.md) for their distinct lifecycle and package behaviour.

The target display is 320×170. The visual vocabulary takes inspiration from compact phone interfaces, with original Alcard assets and a keyboard-first layout. Five 64-pixel softkey slots occupy the bottom 16 pixels; each label fits six 8-pixel glyphs. Graphical labels do not print the physical key numbers. Larger surfaces use integer nearest-neighbour scaling with preserved aspect ratio and margins.

## Keys, focus and context

| Slot | Keyboard aliases | Role outside text entry | Future controller role |
| --- | --- | --- | --- |
| 4 | Enter | Primary action | A |
| 5 | Space | Options, context or selection | X |
| 6 | Tab | Special action | Share |
| 7 | F3 without modifiers | Secondary action; Menu search | Y |
| 8 | Esc | Back or cancel | B |

Controller names describe the interaction model; a gamepad backend is not implemented. Physical 4–8 are independent of the active XKB layout only outside an editor. Shell bindings apply only while its surface owns keyboard focus. They do not consume digits, Space, Tab, F3 or Esc from another application.

The current dialog or editor takes priority over the underlying screen. In text entry, digits 4–8 and Space are text. Enter submits, Esc discards, Left/Right/Home/End move the cursor, Backspace/Delete remove characters and Ctrl+U clears the field. UTF-8 positions are counted in characters, with XKB compose providing the text. Search accepts 96 characters; the reusable nonsecret editor allows 1,024, accepts at most 32 characters per event and shows a 32-cell viewport. These controls do not handle authentication secrets.

Held keys on focus arrival, key repeats and unsupported modifier combinations cannot confirm an action. A fresh modifier snapshot is required after entering focus. Confirmations use only the labelled bottom actions: 4/Enter confirms and 8/Esc cancels; there is no second row of internal buttons or left/right button selection. A vanished selected window is not replaced silently with its neighbour.

### Home and the global shortcut

Home binds **4 = Recent, 5 = Quick settings, 6 = Menu, 7 = empty, 8 = Notifications**. Its physical slot 8 is a destination; Esc and Backspace retain the semantic Back action and leave an already open Home at Home. Plain Esc in an external application remains that application's Esc.

The installed labwc profile maps a **lone Super_L release** to cursor hiding followed by `alcard-shell --home`. On the physical keyboard, Fn+Esc produces this virtual Super key; there is no held-Esc timer. Super combinations retain their normal meaning. Home requests return to Shell without terminating or freezing the foreground application. Lock suppresses Home through the compositor's lock policy and Shell's lock state.

The first Shell map becomes fullscreen and undecorated. The pointer is hidden on first map and Home, and reappears with pointer activity; mouse-follow focus is disabled. These are ordinary Shell window rules, not the mechanism securing lock surfaces. See [Fonts and keyboard](fonts-keyboard.md) and [Security](security.md).

## Menu, Recent and applications

Menu displays eight applications per page in a 4×2 grid. 4/Enter activates the most recently used matching window or launches the application; 5/Space opens options; 7/F3 edits the search; 8/Esc returns Home. Search matches the localized name and desktop ID without case sensitivity. Editing updates a match count; only Enter changes the applied filter. An empty query shows everything, and Esc preserves the previous filter and selection.

Desktop discovery follows XDG application locations and respects localized names, `Hidden`, `NoDisplay`, desktop visibility and `TryExec`. A higher-priority hidden entry masks a lower-priority entry with the same desktop ID. Exec is parsed into a program and arguments, not evaluated as a shell command. `DBusActivatable=true` currently uses an available Exec fallback; native D-Bus application activation is not implemented. `Terminal=true` launches through Foot with the installed Alcard profile, the desktop ID as app ID, and the entry's working directory. Missing or invalid terminal prerequisites produce a launch error.

Recent lists running windows in MRU order and up to 12 closed-application entries retained in memory. Matching uses desktop ID/app ID and `StartupWMClass`, not a title substring or a guessed process. An application with several windows exposes a keyed window picker. Close sends the compositor's graceful foreign-toplevel close request; it does not send a process-kill signal. If foreign-toplevel support is absent, window operations are unavailable.

Launch and window operations distinguish an attempt from an observed result:

| Operation | Observed contract |
| --- | --- |
| Launch | Starting until a matching new window, a failure, or a 15-second unconfirmed deadline |
| Early child failure | A nonzero exit has a short 350 ms grace period for a window event; a zero-exit wrapper can still be waiting for its window |
| No observed new window | Unconfirmed; retry is explicit and warns that another instance may be created |
| Activate or close | Wait for the relevant window event; a 5-second missing acknowledgement is not reported as success |
| Late events | Update the tracked operation without unexpectedly changing focus or targeting another window |

An existing window whose app ID changes is not evidence of a newly launched window. Selection and confirmations retain stable IDs while inventories change. There is no fabricated progress percentage for an operation without a measurable backend.

## Quick settings

Home 5/Space opens one scrollable list, in this fixed order:

| Row | Interaction |
| --- | --- |
| Brightness | Left/Right changes the level by approximately five percentage points |
| Volume | Left/Right changes volume; 5/Space toggles mute |
| Wi-Fi | 4/Enter opens the network list; 5/Space toggles power |
| Bluetooth | 4/Enter opens the device list; 5/Space toggles power |
| Logout | 4/Enter opens confirmation |
| Reboot | 4/Enter opens confirmation |
| Power off | 4/Enter opens confirmation |
| Lock | 4/Enter starts the separate locker immediately, without another confirmation |
| Sleep | 4/Enter opens confirmation and the lock-before-suspend flow |

7 refreshes and 8 returns Home. Radio lists use 4 for details, 7 for refresh and 8 for Back; details scroll with arrows. Unavailable controls show a reason. Pending operations retain their identity when a view closes and reopens, and are not submitted twice by repeated keys.

Shell radio views provide power, discovery/listing and details. Wi-Fi passwords, saved profiles and Bluetooth pairing are implemented in the **Settings TUI**, not these compact views. The current Wi-Fi backend is shared NetworkManager, including radio power, not Alcard-owned standalone DHCP or wpa_supplicant management. Bluetooth uses BlueZ; its fixed privileged power operation can additionally change the Bluetooth soft block through a checked `/dev/rfkill` device. Ordinary status only reads radio state. A missing or unreadable service is not displayed as “off”.

`alcard-radio-helper` accepts only fixed status/Wi-Fi/Bluetooth operations. Read-only status is unprivileged; operations requiring authority use the installed pkexec policy and graphical agent. There is no general command or device-path argument. Reads, discovery, JSON output and child lifetimes are bounded: the helper watchdog is 10 seconds and the Shell worker has a 12-second deadline. A successful mutation is followed by readback. The UI does not claim an optimistic state, continuously rescan or automatically repeat a failed mutation.

Logout, reboot and power-off use elogind's login1 API. Capability discovery distinguishes allowed, authentication required, denied, blocked, unsupported and unavailable. The operation rechecks its real user/session identity and applicable inhibitors before sending; logout targets the verified current session. Reboot and power-off retain inhibitor checks rather than force flags. A challenge goes through the separate polkit agent; Shell never collects the password.

Read-only preflight has a six-second budget and an interactive system operation up to 120 seconds. A service accepting a request is not proof that logout or a power cycle has completed. Losing the reply after sending gives an unknown outcome. Accepted or unknown requests keep their duplicate-submission latch; Back hides the card but does not cancel the submitted operation. Retry on an unavailable card refreshes capabilities, not the destructive operation. Suspend has its own stricter coordination described in [Power and audio](power-audio.md).

## Notifications

Shell implements `org.freedesktop.Notifications` on the session bus with plain body text and actions. History is in memory and bounded to 64 entries; it is not persistent across login. Titles allow 128 characters, bodies 4,096, application/action labels 64, and action keys 128. An incoming notification can contain at most 64 action pairs. The command queue is bounded to 32 and UI snapshots coalesce.

The Home bell counts unread history, including entries whose external action lifetime has expired. Merely seeing a banner or opening the list does not mark a message read. **4/Enter reads the selected notification**; it never implicitly invokes its default action. **5/Space opens context**, where named/default actions, deletion, clearing and service retry are separate choices. Within context, 5 selects and 8 returns. Detail text uses a 36-column, ten-row viewport with line/page scrolling and shows the local time/source.

Every replacement increments a generation and becomes unread. UI commands carry `(id, generation)`, so an old detail or confirmation cannot operate on a replacement. Replacing the open message returns to the list. Expired ordinary messages stay readable with external actions disabled; transient entries are removed on their lifecycle close. A sender's `CloseNotification` removes its active notification and history. User deletion/clearing emits the appropriate close reasons. A normal external action closes the notification; a resident one remains available.

The normal default expiration is five seconds. Critical, resident and explicit zero-timeout notifications do not auto-expire; other notifications honour positive explicit timeouts. A banner remains for at most four seconds, independently of history. It is suppressed while Notifications, a modal dialog, authentication or lock owns attention, and old suppressed banners are not replayed later. The overlay takes no keyboard focus, has an empty input region and does not reserve desktop space.

If the service disconnects, old history remains readable but its external actions are disabled. Retry is available in context; reconnect uses a 2–30 second backoff, does not replace another notification server, and does not reactivate stale actions. Internal notifications have a bounded queue during reconnection. HTML, remote images, notification sounds, do-not-disturb scheduling and disk history are not implemented.

## Status and presentation

Home's battery status distinguishes absent, unavailable and a valid 0%. Device-scope batteries are excluded; multiple batteries are not combined into an invented energy-weighted total. Wi-Fi and Ethernet are separate physical-interface indicators. Network states distinguish unavailable/absent, disabled, disconnected, connecting, link only, addressed and routed. “Routed” does not prove Internet access, DNS resolution or portal completion. Wi-Fi strength maps valid negative dBm readings to four levels; a failed read is not zero signal.

Classic is blue with coloured semantic icons; Emerald provides the second palette and geometry. Both use the shared Alcard Pixel UI, including dialogs, notifications, the locker and polkit agent. The working Settings TUI can edit appearance. There is no F2 theme toggle and no separate graphical Settings application yet.

The common preference file is `$XDG_CONFIG_HOME/alcard/shell.toml`, normally `$HOME/.config/alcard/shell.toml`; base directories must be absolute. A minimal example is:

```toml
schema_version = 1
theme = "classic"
icon_style = "solid"
accent = "#75DCEB"
locale = "auto"
```

All selection fields are optional. Theme is `classic`, `emerald` or a local TOML path; icon style is `solid`/`outline`; accent is `#RRGGBB`. The legacy per-user locale accepts `auto`/`en`/`ru`/`es`, but the configured **system language takes precedence**. An explicit CLI `--locale` is still a diagnostic override; see [Settings](settings.md) for the complete language policy.

CLI `--theme`, `--icon-style`, `--accent` and `--locale` override corresponding values without writing a file. A relative theme path in the file is resolved from its directory; a CLI path is resolved from the current directory. Config and theme files are limited to 16 KiB each. Unknown fields, incompatible schema, invalid types or values reject the complete candidate. Missing configuration uses defaults and explicit CLI values; invalid startup configuration is diagnosed with a safe default presentation.

Send SIGHUP to the specific Shell PID to reload. The entire candidate is validated before replacement; a failure retains the last valid presentation and emits a deduplicated diagnostic notification. Successful reload preserves navigation/selection/dialog targets and clears the icon cache; deleting the file restores defaults plus CLI overrides. There is no background file watcher. Settings performs a validated atomic save and requests this reload automatically. New authentication forms read preferences; an already active password form keeps its presentation stable.

Native icons have separate 16- and 24-pixel geometry. Other desktop icons use local PNG resolution with semantic/generic fallback. The resolver limits files to 256 KiB, dimensions to 1–256, chunks to 256 and decoder allocation to 1 MiB; rejects APNG, invalid chunk structure and data after IEND; and does not read final symlinks, FIFO or devices. It considers at most four XDG data roots plus `$HOME/.icons` and 64 candidate paths; names/paths are bounded to 128/1,024 bytes. Network URLs, SVG and recursive scans are absent. The LRU holds at most 64 positive/negative entries and 512 KiB of scaled pixels, with draw sizes 1–64. These are cache bounds, not a total process-RAM measurement.

## Architecture and verification boundary

Shell uses SCTK/calloop with one Wayland connection/event queue, tiny-skia rendering and ARGB8888 `wl_shm`. Each surface has at most two SHM buffers. Glyph geometry is cached. The main interactive surface is an xdg-toplevel with app ID `alcard-shell`; notifications use a separate noninteractive overlay. It does not put a permanent panel over other applications.

Rendering follows dirty state, configure events and frame callbacks; a callback alone does not create a continuous redraw loop. The clock updates by minute and ordinary status polling is coalesced at five seconds. Missing optional protocols disable their corresponding features: no layer-shell means history without banners, and no foreign-toplevel means no fabricated window control. Idle, brightness, audio and suspend have separate bounded policies documented in [Power and audio](power-audio.md).

Relevant source: [model/navigation](../../rust/apps/alcard-shell/src/model.rs), [input](../../rust/apps/alcard-shell/src/input.rs), [desktop entries](../../rust/apps/alcard-shell/src/desktop.rs), [launch tracking](../../rust/apps/alcard-shell/src/launch.rs), [notifications](../../rust/apps/alcard-shell/src/notifications.rs), [system actions](../../rust/apps/alcard-shell/src/system.rs), [radio operations](../../rust/apps/alcard-shell/src/radio_system.rs), [icon cache](../../rust/apps/alcard-shell/src/icon_cache.rs), [labwc profile](../../scripts/wayland/rc.xml).

Source tests, native/headless scenarios and installed-session acceptance verify different layers. The current NetworkManager/Settings service paths require focused installed-system acceptance. Actual display, keyboard, power, audio and radio behaviour still require [hardware acceptance](hardware.md); see [Testing](testing.md) and [Current status](status.md).
