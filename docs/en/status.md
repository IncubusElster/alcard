# Current project status

**Language:** English · [Русский](../ru/status.md)

> Scope: current source tree · Updated: 2026-11-22

**Alcard is an experimental project at the hardware bring-up stage.** The source contains the system components described below. A supported public device image and Alcard APK update channel are not available yet. Current verification results belong to the exact source and artifacts checked by [Alcard CI](https://github.com/IncubusElster/alcard/actions/workflows/ci.yml).

## Implemented capabilities

| Area | Current implementation | Remaining acceptance or feature boundary |
| --- | --- | --- |
| Board and image tooling | v5/v5-lite profiles, pinned kernel/BSP, firmware, signed APK builders and regular-file image construction | Complete physical Cardputer boot/device acceptance is open |
| Installation | Hello, administrative account policy and Graphics, Graphics Minimal, Console choices | All three login-mode VM scenarios passed with the same signed APKs; interactive Hello/Settings acceptance remains open |
| Settings and networking | Eight Ratatui sections, shared settings core, NetworkManager profiles, BlueZ and one system-language setting | Private D-Bus lifecycle cases passed; installed authorization, real NetworkManager operations and reboot persistence still need acceptance |
| Desktop | Application menu, Recent, notifications, themes, search and quick settings | Fresh APK runtime and the exercised login-VM paths passed; Hello language/theme/persistence scenarios and physical behavior remain separate |
| Authentication and recovery | Separate graphical polkit agent, native PAM session lock, login/session supervision and recovery | Real PAM/compositor/VM checks and physical sleep/wake have different scopes |
| Display, audio and power | Idle/output-power policy, brightness, PipeWire/WirePlumber volume/mute, sleep coordination and battery policy | Physical screen, codec, radio, battery and energy behavior remain device work |
| Boot updates | Persistent ext4 ROOT and three FAT partitions with the same initial payload | Automatic A/B activation, health confirmation and rollback are not implemented |
| GUI Settings | Shared operations and a working terminal frontend | A separate graphical Settings application is planned |
| Documentation and licensing | Primary English guides, full Russian translations and GPL-3.0-or-later with identified upstream exceptions | Distributed binaries still need a complete dependency notice/source bundle |

The feature contracts are in [overview](overview.md), [settings](settings.md), [desktop](desktop.md), [security](security.md), [power and audio](power-audio.md) and [hardware](hardware.md). This page records readiness boundaries; it is not a second specification for those components.

## How to read verification results

| Level | Evidence to inspect | Limit of a successful result |
| --- | --- | --- |
| Static preflight | `bash build.sh audit` and `python3 tools/check-docs.py` | Syntax, declared inputs, package relationships and documentation consistency; no compilation or package installation |
| Source checks | The source-check result for the selected commit in CI | Formatting, tests, lint and dependency boundaries on the architecture actually run |
| Private D-Bus | Named NetworkManager/BlueZ protocol receipts from the exact compiled client/agent | Ownership, prompts and cancellation against simulated service peers; no real daemon policy or radio |
| Installed packages | A fresh APK set, exact package versions/hashes and native scenarios from a manual acceptance run | Installed payload and the exercised real or simulated services; no physical device guarantee |
| Login VM | Sequential Graphics, Graphics Minimal and Console results using the same APK set | Alpine userland provisioning/login/recovery on the virtual platform |
| Physical device | Board revision, image/package hashes, kernel/firmware and observed hardware results | Only the tested device and operations |

Use the run summary and artifacts to distinguish PASS, FAIL, BLOCKED, NOT RUN and CANCELLED. A source-only success does not mark installed packages, VM login or the physical device accepted. An ignored test is not a passing integration scenario. The [testing guide](testing.md) describes how to obtain each level with one CI runner.

## Release limitations

- Interactive Hello and the installed NetworkManager/Settings contract still need acceptance, including doas/su, denied or cancelled operations, profile changes and state after reboot. BlueZ protocol cases passed on a private bus; real service policy and physical pairing remain separate work.
- The image verifier does not yet enforce a strict locked-root release policy. Explicit developer-root images share the ordinary output filename and do not record that mode in the manifest.
- There is no configured public Alcard APK channel or implemented automatic BOOT update/recovery mechanism.
- Physical display/input, interfaces, audio, charging, sleep/wake and energy measurements remain open for the intended board revision.

Opening the source repository as experimental work and publishing a supported device image are separate decisions. The [active roadmap](roadmap.md) states the remaining work and completion criteria.

## Keeping status current

Keep current contracts in these guides. CI logs and machine-readable outputs belong to the run artifacts; a distributed release must carry its own durable source/package/image provenance. Update this page when a readiness boundary changes, and link the applicable current workflow or release. Do not copy development checkpoint narratives, old run IDs or obsolete plans back into the maintained source tree.
