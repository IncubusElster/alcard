# Power, display and audio

**Language:** English · [Русский](../ru/power-audio.md)

> Scope: current source tree · Updated: 2026-11-22

The full Graphics session owns Shell's idle, output-power, battery and sleep policies and starts a user PipeWire stack. Graphics Minimal and Console do not automatically start these Shell policies or that audio stack. Installed packages alone do not make a policy active. [Settings](settings.md) provides the current configuration UI; [Desktop](desktop.md) defines quick-settings navigation, and [Security](security.md) defines actual session locking.

## Configuration and defaults

Use Settings → Display and power, or edit the common `$XDG_CONFIG_HOME/alcard/shell.toml` (normally `~/.config/alcard/shell.toml`) and request SIGHUP for the specific running Shell PID. Settings saves a validated complete candidate atomically and requests reload itself.

```toml
schema_version = 1
theme = "classic"

[idle]
lock_after_seconds = 300
screen_off_after_seconds = 360

[battery]
low_percent = 20
critical_percent = 5
suspend_on_critical = false
```

Place general keys before the TOML tables. These are the defaults, not evidence that a particular device supports every operation.

| Preference | Accepted values and effect |
| --- | --- |
| `idle.lock_after_seconds` | Integer 0–86,400 seconds; zero disables automatic locking |
| `idle.screen_off_after_seconds` | Integer 0–86,400; zero disables automatic screen-off; a positive value requires a positive lock timeout and must be at least that timeout |
| Both idle values zero | Disable both idle actions |
| `battery.low_percent` / `critical_percent` | Integer percentages satisfying `1 <= critical_percent < low_percent <= 100` |
| `battery.suspend_on_critical` | Boolean; automatic critical-battery suspend is opt-in |

Unknown fields, wrong types, duplicate keys or invalid combinations reject the whole candidate. Configuration is limited to 16 KiB. Invalid reload retains the last valid policy. Successful idle reconfiguration starts new notification intervals and restores outputs owned by the old screen-off policy. Battery reconfiguration requires a fresh observation and does not rearm an already consumed automatic-suspend attempt. See [Desktop](desktop.md) for complete theme/CLI/reload rules.

## Idle lock and output power

Shell uses compositor-wide `ext-idle-notify-v1` notifications on every seat. It does not infer inactivity from Shell focus, poll for input or start an independent wall-clock inactivity thread. The normal idle-notification request respects compositor idle inhibitors. All tracked seats must be idle for the relevant action; a newly added seat starts active.

At the lock deadline Shell requests the normal production locker. Screen-off is allowed only after a **current native `LOCKED` lifecycle acknowledgement**. A process, lock journal or session-bus presence is insufficient. If lock fails, the display stays on, a diagnostic is shown and the same idle period does not repeatedly spawn lockers. Manual Lock remains available when the compositor lacks idle notification support.

Input activity wakes outputs that Shell turned off, while the session remains locked until PAM unlock. A late lock acknowledgement after activity cannot switch the display off for the old idle generation. Unlock and resume create a new interval. After Shell restart, an already living locker is preserved, but its advisory presence cannot authorize screen-off without a fresh native acknowledgement.

Output power uses `zwlr_output_power_manager_v1` on the existing Wayland connection. It changes power mode rather than removing an output from the desktop layout. The code tracks desired and acknowledged state separately: a wake request must supersede a pending off request even if the last acknowledged state was still on. Hotplug and protocol removal are handled explicitly.

On startup/reload, a newly controlled output reported off is restored on. With screen-off disabled, Shell performs that restoration once and releases exclusive power control. A rejected output-control object is not retried on each frame or timer; a relevant hotplug/reload can create a new binding. Normal shutdown makes a best-effort on request and releases ownership; after SIGKILL, the replacement Shell restores on when it acquires control. A missing/failed protocol is visible as unavailable, not a pretend successful display change.

## Brightness

The Shell quick control discovers exactly one unambiguous `/sys/class/backlight` device and reads its `brightness` and `max_brightness`. It validates bounded numeric reads, safe names, canonical device identity, inode and range; more than one candidate is unavailable rather than an arbitrary choice. Reads use fixed sysfs paths with nonblocking/type checks, not user-provided paths.

Changes use the current verified active local graphical session's elogind `Session.SetBrightness("backlight", name, value)`. Shell neither writes sysfs directly nor changes permissions, adds broad access groups or launches a root brightness helper. The operation binds to the unique login1 owner, real UID/PID, session and seat and rechecks device identity before/after mutation.

Left/Right moves by approximately five percentage points of the raw range. The normal lower target is `ceil(max_brightness / 100)`, at least one raw step; Shell does not intentionally lower a positive normal level to zero. An externally set value already below that floor is not made lower by Decrease. These are the kernel's configured brightness units, not measured panel luminance, actual PWM duty or proof of hardware power savings.

The **Settings TUI has a distinct privileged brightnessctl operation accepting 0–100%**. Its zero value is not covered by Shell's nonzero-floor policy and does not automatically imply that the session was locked first. Hardware panel behaviour and usable minimum brightness remain acceptance items.

## Session audio stack

Full Graphics starts PipeWire, WirePlumber and pipewire-pulse through the session-owned `audio-session` helper. The package set includes the PipeWire ALSA integration and tools. The launcher uses the existing private session D-Bus and real `XDG_RUNTIME_DIR`; it does not require systemd user units or a root audio daemon. The helper has a runtime lifetime lock, verifies its owning session process and tracks the exact PID/start time of every daemon it owns.

An existing foreign audio endpoint causes an explicit unavailable result rather than a user-wide kill, socket deletion or takeover. Inherited remote/runtime overrides are cleared; this stack owns the local `pipewire-0` and session Pulse socket. Normal shutdown stops only its own processes. Idle waiting does not poll or keep a Python supervisor running. If the stack fails, Shell remains usable and a fresh logout/login retries initialization; reopening or refreshing volume does not restart the daemons.

Minimal and Console need independently available user audio services for their Sound page. They do not silently start this Graphics stack. A full route/profile editor is not implemented; **the current Settings TUI can already choose an available default Audio/Sink output**.

### Volume and mute

Shell controls WirePlumber's actual `default.audio.sink`, never the first sound card, an arbitrary node or a microphone. Left/Right adjusts software volume in approximately five-point steps within 0–100% unity; 5/Space toggles real software mute independently of whether the codec exposes a hardware mute switch. Zero volume and mute are distinct states.

The displayed level is derived from the largest channel value. If another client sets any channel above unity, Shell shows `100%+` and refuses further increase or unmute until volume is reduced. A volume change sets a common channel level; it does not promise to preserve a pre-existing channel balance. The software ceiling is not a claim about acoustic safety, speaker loudness or the ES8389's physical 0 dB setting.

The current backend is PipeWire; there is no automatic direct-ALSA fallback. Old direct-mixer raw values and codec-gain experiments belong to their dated validation records and must not be applied as current Shell control instructions.

### Bounded control and readback

Brightness and audio share one bounded worker with one pending request. Opening/refreshing the controls performs discovery, and mutation performs fresh checks and readback. The UI does not block on D-Bus or subprocesses, optimistically claim the requested result, auto-repeat a mutation or steal focus when a late reply arrives. Request IDs reject stale responses.

Brightness discovery and audio discovery each have a six-second budget; a combined refresh can therefore take up to their combined budgets. Each mutation has one six-second budget across its work. Child commands use fixed absolute paths and separate arguments, with bounded stdout/stderr and cleanup of their own timed-out children.

Audio uses `/usr/bin/pw-dump -N -r pipewire-0` and `/usr/bin/wpctl` under `LC_ALL=C`. A bounded complete graph snapshot pins the PipeWire core cookie, actual default metadata, Audio/Sink ID and serial, device and active route. Device Route properties take precedence over Node Props; all channels are checked. Graph output is limited to 1 MiB/1,024 objects, parameters to 128 and channels to 64; ordinary command/error output is bounded to 64 KiB.

A refresh uses one graph snapshot; mutation uses fresh snapshot → fixed wpctl action → fresh readback, without retrying the write. Server restart, default change, hotplug, identity/range change, timeout or contradictory readback can produce Changed/Unknown/Unavailable. Separate CLI connections do not provide a global atomic compare-and-set against another mixer client; the checks detect important races but do not claim such a lock. The simpler TUI Sound controls share the user service but are a separate implementation from Shell's full channel/route validation.

## Manual suspend

Quick settings' ninth row opens Sleep confirmation. 4/Enter confirms; 8/Esc cancels. Shell requires noninteractive `CanSuspend=allowed`, a valid current local user/session/seat, supported API and inhibitor checks, and a ready sleep monitor. An authentication challenge is displayed as a limitation; Shell does not open a hidden administrative prompt for this flow or bypass policy.

Preparation is tied to a request ID and login1 owner epoch. It has an eight-second preparation deadline followed by a ten-second lock deadline. The same production locker must acknowledge `LOCKED`. Coordination waits until the complete event batch is applied, so a later `UNLOCKED`, uncertainty, session loss or battery cancellation in that batch revokes the grant before the worker can use it. The worker checks a revocable permit immediately before dispatch.

The backend repeats session/capability/inhibitor checks and sends `SuspendWithFlags` with `ROOT_CHECK_INHIBITORS` only. It does not use force/skip-inhibitor flags, authentication retry or a shutdown fallback. Back/Home can cancel preparation while Shell still owns the relevant input and before dispatch; they do not terminate an already started locker. Once the locker owns input, its Esc cancels PAM only. Once suspend is sent, Back only hides the result and cannot promise to recall it.

An accepted method call is not proof that the hardware slept. A correlated `PrepareForSleep(true)` followed by `false` is the observed cycle; a lone false does not invent a completed cycle. An uncertain sent request retains a no-resubmit latch. It is cleared only by the relevant observed cycle, or a fresh service epoch with verified non-preparing state and a new delay inhibitor; the old operation remains recorded as unknown rather than retroactively successful.

## External sleep and resume

The independent sleep monitor subscribes before declaring readiness, reads actual preparation state and owns a `sleep` delay-inhibitor file descriptor. It can react to an external `PrepareForSleep(true)` while the manual suspend RPC is still waiting. It requests the locker without switching the underlying Shell screen and releases its own delay only after the matching native lock acknowledgement for the same epoch/cycle.

A delay inhibitor is time-limited by the service's `InhibitDelayMaxUSec`, **not an indefinite veto on an external suspend**. If lock fails, Alcard retains its delay rather than claiming success, but the service may proceed when its limit expires. There is one lock attempt per external cycle. Owner loss revokes outstanding permits; reconnect restores observation and never replays a previous system mutation.

On observed resume the monitor reacquires its inhibitor, invalidates old input/idle intentions, requests outputs on and preserves the current UI, editor, selection and notification history. A real native lock stays locked until PAM succeeds. Shell does not reset brightness, volume or the default audio sink. Device controls refresh on their next discovery, and battery policy requires a post-resume observation instead of reusing a pre-sleep sample.

## Low-battery policy

Default thresholds are low at 20% and critical at 5%, with automatic suspend disabled. Only one unambiguous system battery with valid capacity 0–100%, a known source identity and explicit `Discharging` can drive automatic action. `scope=Device` batteries, missing/invalid capacity, ambiguous multiple batteries, vanished sources and unknown charge state cannot. An optional absent `present` attribute is not automatically a missing battery; actual data still has to validate. Unknown external-power telemetry is not fabricated as “unplugged”; explicit Discharging remains useful when no external-supply information exists.

The status worker samples every five seconds, assigns a sequence even when values are unchanged, and timestamps the **start** of reading. Coalesced delivery cannot refresh old data's age. A sample must be younger than 15 seconds and newer than the relevant resume/configuration/lock barrier. Loss of data invalidates the action; a real 0% remains a valid critical value.

One internal notification represents the episode. Entering low warns once; entering critical updates/escalates that warning once. Percentage/language updates do not create another unread event or banner, and user dismissal does not cause the next poll to recreate it. Warnings respect normal authentication/lock banner suppression and do not steal focus or force Home. Charging, Full or confirmed external power clears the episode. During discharge, complete recovery requires a level above `low_percent` and at least `min(low_percent + 3, 100)`.

The critical warning band has three percentage points of hysteresis. However, a pending automatic sleep is cancelled **as soon as a fresh level exceeds `critical_percent`**; it does not wait for that visual hysteresis to clear. Source change or stale telemetry does not rearm a previously used attempt. On resume, a temporary inhibition prevents an immediate return to sleep; a fresh recovery above the critical band can clear that temporary block, while a consumed automatic attempt remains consumed for the discharge episode.

### Opt-in automatic suspend

When enabled and freshly critical, policy consumes its one attempt **before** handing control to the sleep flow. This includes attempts that fail, are denied, cancelled or become unknown. It first persists the attempt marker, requests a correlated availability refresh with an eight-second deadline, then uses the same noninteractive preflight and native lock-before-suspend coordinator as manual sleep.

After the locker acknowledges lock, a further fresh battery reading must have started after that lock barrier. Charging, threshold recovery, data loss and cancellation from the whole event batch are applied before dispatch. The permit expires with the battery observation and is revoked if the episode ceases to qualify. Failure produces a reason and leaves any acquired lock intact; it does not force power-off, ignore inhibitors or repeatedly retry suspend.

The runtime journal `$XDG_RUNTIME_DIR/alcard-battery-inhibit-v1` preserves attempt/resume inhibition across Shell restarts in the same runtime lifetime. Owner, mode, regular-file type, inode and content are verified; creation is exclusive, and cleanup only removes the expected state. A corrupt or unavailable journal disables automatic suspend for the process lifetime until restart rather than enabling another attempt. Actual charging/recovery clears valid markers through policy; deleting a marker manually is not the normal recovery procedure. This is a runtime guard, not a durable history across a new boot.

## Verification and source map

Source-level state machines and isolated fixtures check timing, readback, races and refusal paths. Installed-APK and native VM scenarios must identify the exact source and package set. They do not prove actual Cardputer brightness, acoustic output, charging thresholds, physical suspend, wake or energy consumption. [Hardware](hardware.md), [Testing](testing.md) and [Current status](status.md) identify the remaining acceptance work.

Source: [preferences](../../rust/crates/alcard-settings-core/src/preferences.rs), [idle policy](../../rust/apps/alcard-shell/src/idle.rs), [Wayland idle/output power](../../rust/apps/alcard-shell/src/wayland/idle.rs), [control worker](../../rust/apps/alcard-shell/src/controls.rs), [brightness](../../rust/apps/alcard-shell/src/controls/brightness.rs), [PipeWire control](../../rust/apps/alcard-shell/src/controls/audio.rs), [audio launcher](../../scripts/wayland/audio-session), [sleep backend/monitor](../../rust/apps/alcard-shell/src/sleep.rs), [sleep coordination](../../rust/apps/alcard-shell/src/sleep_intent.rs), [battery policy](../../rust/apps/alcard-shell/src/battery.rs), [battery journal](../../rust/apps/alcard-shell/src/battery_journal.rs), [status sampling](../../rust/apps/alcard-shell/src/status.rs).
