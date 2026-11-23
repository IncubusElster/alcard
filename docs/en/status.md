# Current project status

**Language:** English · [Русский](../ru/status.md)

> Scope: current source tree · Updated: 2026-11-23

**Alcard is an experimental project at the hardware bring-up stage.** The source contains the system components described below. A supported public device image and Alcard APK update channel are not available yet. Current verification results belong to the exact source and artifacts checked by [Alcard CI](https://github.com/IncubusElster/alcard/actions/workflows/ci.yml).

## Implemented capabilities

| Area | Current implementation | Remaining acceptance or feature boundary |
| --- | --- | --- |
| Board and image tooling | v5/v5-lite profiles, pinned kernel/BSP, firmware, signed APK builders and regular-file image construction | Complete physical Cardputer boot/device acceptance is open |
| Installation | Hello, administrative account policy and Graphics, Graphics Minimal, Console choices | All three VM login modes and interactive Hello passed on the same signed APK set; physical boot and input remain open |
| Settings and networking | Eight Ratatui sections, shared settings core, NetworkManager profiles, BlueZ and one system-language setting | Installed doas/su, Ethernet NetworkManager profile changes, locale and reboot persistence passed in VM; physical Wi-Fi, bluetoothd and real pairing remain open |
| Desktop | Application menu, Recent, notifications, themes, search and quick settings | Installed runtime, login modes, live locale/theme settings and persistence passed in VM; Cardputer display/input and real hardware behavior remain open |
| Authentication and recovery | Separate graphical polkit agent, native PAM session lock, login/session supervision and recovery | Real PAM/compositor/VM checks and physical sleep/wake have different scopes |
| Display, audio and power | Idle/output-power policy, brightness, PipeWire/WirePlumber volume/mute, sleep coordination and battery policy | Physical screen, codec, radio, battery and energy behavior remain device work |
| Boot updates | Persistent ext4 ROOT and three FAT partitions with the same initial payload | Automatic A/B activation, health confirmation and rollback are not implemented |
| GUI Settings | Shared operations and a working terminal frontend | A separate graphical Settings application is planned |
| Documentation and licensing | Primary English guides, full Russian translations and GPL-3.0-or-later with identified upstream exceptions | Distributed binaries still need a complete dependency notice/source bundle |

The feature contracts are in [overview](overview.md), [settings](settings.md), [desktop](desktop.md), [security](security.md), [power and audio](power-audio.md) and [hardware](hardware.md). This page records readiness boundaries; it is not a second specification for those components.

## Accepted software baseline

The last full sequential software acceptance is [Alcard CI run 45096681285](https://github.com/IncubusElster/alcard/actions/runs/45096681285) for `164878f686511ed63d6471da30f2cd818e667829`. Its machine-readable `result.json` records all seven stages with exit code 0 and unchanged inputs. Nine signed APKs, five installed runtime groups, three login-mode VMs, Hello and 26 installed Settings scenarios passed. The VM checked ordinary-user doas/su, failed/cancelled authorization and terminal restoration, real NetworkManager virtual Ethernet DHCP/static IPv4, service recovery, EN/ES/RU locale, Shell theme and reboot persistence. The source is an **accepted software baseline**; subsequent documentation changes do not require claiming a new image or device acceptance. Keep the original run artifacts as evidence; Actions artifact retention is limited.

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

- The [full Alpine aarch64 acceptance on source commit `164878f`](https://github.com/IncubusElster/alcard/actions/runs/45096681285) passed: nine signed APKs, five runtime groups, all three login modes, interactive Hello, 26 installed Settings scenarios and reboot persistence. This is a software acceptance, not a physical Cardputer certification. Private-bus BlueZ/NetworkManager tests use simulated service peers; installed Settings uses real NetworkManager on isolated virtual Ethernet. Physical Wi-Fi and Bluetooth pairing and real bluetoothd service policy are untested.
- The image verifier does not yet enforce a strict locked-root release policy. Explicit developer-root images share the ordinary output filename and do not record that mode in the manifest.
- There is no configured public Alcard APK channel or implemented automatic BOOT update/recovery mechanism.
- Physical display/input, interfaces, audio, charging, sleep/wake and energy measurements remain open for the intended board revision.

Opening the source repository as experimental work and publishing a supported device image are separate decisions. The [active roadmap](roadmap.md) states the remaining work and completion criteria.

## Keeping status current

Keep current contracts in these guides. CI logs and machine-readable outputs belong to the run artifacts; a distributed release must carry its own durable source/package/image provenance. Update this page when a readiness boundary changes, and link the applicable current workflow or release. Do not copy development checkpoint narratives, old run IDs or obsolete plans back into the maintained source tree.
