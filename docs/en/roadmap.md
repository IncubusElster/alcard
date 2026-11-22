# Active roadmap

**Language:** English · [Русский](../ru/roadmap.md)

> Scope: current source tree · Updated: 2026-11-22

This is the single plan for the current source tree. [Current status](status.md) describes implemented capabilities and readiness boundaries. Opening the source repository as an experimental project and publishing a supported device image are separate milestones.

Package versions come from APKBUILD through the shared metadata reader. The single CI workflow uses one runner with sequential stages; full package, VM and Hello acceptance is an explicit manual mode. Keep those constraints while completing the work below. See [building](building.md), [packaging](packaging.md) and [testing](testing.md).

## 1. Finish source-publication preparation

- [ ] Review the source bundle and remaining repository references for credentials, private signing material and unintended files. Revoke any live exposed credential; rewriting Git history does not revoke it.
- [ ] Review license scope and distributed source inputs against [licensing](licensing.md) and [third-party notices](../../THIRD_PARTY_NOTICES.md), retaining the separate GPL-2.0, BSD and CC0 terms.
- [ ] Define a private security-reporting contact and verify that the documented channel is available.
- [ ] Review the repository description, first source tag/release notes and branch safeguards.
- [ ] Mark the initial source publication experimental and link current verification scope and outstanding physical/image limitations.

**Completion:** a contributor can identify the license, current source, build path and tested/untested behavior from the repository. Repository visibility changes are a separate publication action.

## 2. Accept the installed NetworkManager/Settings integration

Run a bounded acceptance pass on the final SHA and actual APKs after relevant fixes. Preserve one-runner economy and avoid rebuilding a different package set between native and VM stages.

- [ ] Console retains functional Settings/networking after graphical package removal.
- [ ] Test doas and root-password/su paths, denial/cancel, and terminal restoration after external prompts.
- [ ] Exercise NetworkManager profile creation/application, wrong password, offline/unavailable services, timeout/cancel, saved profiles and persistence after reboot.
- [ ] Verify operation-owned cancellation with a previous/concurrent connection, delayed activation replies and a restarted service; distinguish confirmed cleanup from an unconfirmed cancellation result.
- [ ] Test static IPv4/DHCP transitions and read back the applied state.
- [ ] Exercise the BlueZ agent's PIN/passkey/confirmation/cancel lifecycle and disappearance of a device.
- [ ] Check the single system-language setting in Settings, Shell and subsequent polkit/locker requests.

**Completion:** publish a receipt separating real services, simulated devices and physical adapter work. The current parser/argument/render tests are retained but are not the whole integration claim.

## 3. Prepare the next graphical application

The next application is **GUI Settings**, separate from Shell quick settings and from the terminal frontend. Keep `alcard-settings --hello` as the first-run entry point; a new GUI must not overwrite the existing binary owned by `alcard-hello`.

1. Define the GUI executable/package and launch behavior, while preserving all three installation modes.
2. Give shared operations an explicit result/error/cancellation contract and authorization requirement. Retain the TUI doas/su adapter and connect GUI operations to the existing polkit path; do not copy the terminal application's business logic into the GUI.
3. Implement and accept the GUI with the same settings semantics, including audio-device selection and service-unavailable states. Keep Console free of GUI dependencies.

Reuse the common `alcard_l10n::Locale` and settings operations. Keep feature-specific translations with their owning UI, preserve EN/RU/ES and small-screen label constraints, and follow the existing [module responsibilities](overview.md#rust-implementation-map). Independent authentication/lock processes and the five intentional lockfiles remain architecture boundaries.

**Completion:** one shared system operation has the same validated behavior from TUI and GUI, with the correct authorization UI for each environment. Then extend the remaining forms using that established contract.

## 4. Prepare a distributable device image

The following are release-image requirements, not claims that the current default image has an unlocked root account:

- [ ] Correct [the root-state verifier](../../tools/verify-image-deep-zero.py): an absent `root` record or empty password field is not proof of a locked account.
- [ ] Distinguish explicit `ALCARD_DEV_ROOT=1` images by filename and manifest. They currently share the ordinary image filename. Add a strict release check for the intended locked-root/pending-Hello state and absence of development credentials.
- [ ] Ensure complete applicable licence texts/notices accompany distributed source and binary artifacts. Record the source commit, lockfiles, toolchain/container identity, installed package list and output hashes for each distributed build.
- [ ] Publish and document a signed Alcard APK channel, public-key fingerprint and bootstrap configuration. Current images list only official Alpine repositories.
- [ ] Define normal application updates and the coordinated kernel/BSP update unit; review exact-version pins in `world` accordingly.
- [ ] Implement staged BOOT update/activation/recovery before promising automatic A/B rollback. BOOT-A/BOOT-B are currently duplicate initial content; ROOT remains a single persistent ext4 filesystem.
- [ ] Complete a clean build/install/image verification for the intended profile and retain source/package/image hashes.

**Completion:** the published image is identifiable, contains the intended first-run state, has a defined update/recovery path and reports its actual physical test coverage. See [boot and updates](boot-updates.md).

## 5. Physical Cardputer acceptance

Follow the [hardware matrix](hardware.md), recording the actual SKU/revision. The minimum bring-up result is cold boot into the built-in DRM/fbcon display with built-in keyboard input. Continue with:

- correct v5 versus v5-lite device population and no competing resource owners;
- persistent ROOT, first-run setup and all selected login/session behavior;
- LCD orientation/offset, reset, backlight and reliable input/soft keys;
- real battery readings, low/critical policy, safe power-off and power-fail behavior;
- Wi-Fi, Bluetooth, Ethernet USB identity/mode and USB host/gadget constraints;
- audio codec/routes, RTC, sensors/camera/IR where present and expansion port safe states;
- actual elogind brightness/suspend operations, kernel/firmware sleep-wake and recovery of screen/input/audio;
- device PSS/CPU and energy measurements with stated conditions;
- compatible kernel/BSP and BOOT update/recovery tests before calling the updater supported.

A v5 software result does not accept an untested v3 board; missing hardware on Lite is handled by the profile, not by fabricated test success. Alpine x86_64 is a separate software acceptance target and cannot substitute for Cardputer hardware.

## Deferred scope decisions

After the settings foundation, agree on the initial base-application set and UX before implementing it. Factory reset and changing an installed system's session/package mode require their own state/recovery design. User-installed applications, possible manufacturer-app ports, home encryption through standard Linux facilities and optional expansion modules remain separate proposals. Implement each only after its scope and acceptance criteria are agreed; none is a dependency of the current desktop.

Update this roadmap when code or verification changes. Remove completed tasks, update current status and retain the applicable acceptance boundary. Keep one current plan.
