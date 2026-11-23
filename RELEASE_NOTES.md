# Alcard — initial source preview

**Experimental source publication only. No supported device image or stable release.**

Alcard is a compact Alpine Linux project for M5Stack Cardputer Zero using OpenRC, APK, Rust/Ratatui Hello and Settings, and a keyboard-driven Wayland Shell.

## Source contents

- v5/v5-lite board profiles, kernel/BSP inputs and image build scripts.
- Hello with Graphics, Graphics Minimal and Console options.
- TUI Settings, shared operations and NetworkManager integration.
- Alcard Shell, polkit agent, PAM locker, PipeWire/WirePlumber integration.
- English and Russian technical documentation and EN/RU/ES UI strings.

## Software acceptance

[Alpine ARM64 acceptance](https://github.com/IncubusElster/alcard/actions/runs/45096681285) passed on `164878f686511ed63d6471da30f2cd818e667829`: nine freshly signed APKs, installed runtime, three VM login modes, interactive Hello, 26 Settings scenarios and reboot persistence. The private D-Bus NetworkManager/BlueZ peers and hardware fixtures were simulated. Installed Settings used real NetworkManager on virtual Ethernet.

Physical display, keyboard, Wi-Fi, Bluetooth, audio codec, battery, suspend and power measurements are **not accepted**. No supported image, Alcard update repository, OTA rollback or standalone GUI Settings is supplied. Binary dependency notices and release-image security verification remain release work.

Start with [README](README.md), [building](docs/en/building.md), [status](docs/en/status.md) and [roadmap](docs/en/roadmap.md). Original Alcard work is GPL-3.0-or-later; see [NOTICE](NOTICE) and [third-party licenses](THIRD_PARTY_NOTICES.md).

These notes are not a GitHub tag or published binary release.
