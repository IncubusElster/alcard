# Working on Alcard

Read [README.md](README.md), the relevant current topic in [docs/en](docs/en), and [CONTRIBUTING.md](CONTRIBUTING.md). The Russian documentation is in [docs/ru](docs/ru). The active plan is [docs/en/roadmap.md](docs/en/roadmap.md).

## Architecture

- Target Alpine aarch64, OpenRC, APK and Cardputer Zero. Preserve Graphics, Graphics Minimal and Console boundaries.
- Keep the console workspace free of Wayland/GLib dependencies. The five Cargo workspaces and lockfiles have distinct roles.
- Retain `alcard-settings --hello` for initial setup and ordinary `alcard-settings` for the Ratatui settings frontend. Future GUI Settings uses shared operations from `alcard-settings-core`.
- NetworkManager owns networking. Graphics audio uses PipeWire/WirePlumber. Preserve the separate Shell, PAM locker and graphical polkit agent.
- Keep English as a required keyboard layout and one system-language setting. Respect small-screen text and contextual softkeys.
- Keep tests in separate Rust files behind test configuration. Production APKs use release binaries and must exclude tests/examples/fixtures.

## Changes and validation

- Inspect current source, recipe metadata and the relevant CI result before changing a contract or readiness claim.
- Use `bash build.sh audit` and `python3 tools/check-docs.py` for documentation/license edits. Runtime changes need the relevant source and integration checks described in [testing](docs/en/testing.md).
- Do not invoke package builders or native-auth/VM scripts as harmless read-only checks; inspect their prerequisites and isolated environment requirements.
- Keep CI economical: one workflow, one runner and sequential stages. Documentation-only changes use static preflight; code/build changes add Alpine source checks. Full package/VM/Hello acceptance is an explicit manual mode; consult the testing guide before dispatching it.
- Do not automatically regenerate all lockfiles or migrate toolkits during unrelated work. Do not turn a VM result into a claim of physical hardware support.
- Store generated artifacts and local credentials under ignored `.alcard-local/`; never commit signing private keys, live secrets or developer password hashes.

## Documentation and licensing

- English is the primary documentation. Update the matching Russian page in the same change, preserving commands, identifiers, caveats and acceptance scope.
- Keep topical guides and the current status page focused on this source tree. Store generated CI results as run artifacts and release provenance with the release; do not commit checkpoint narratives, old run receipts or migration maps. Preserve immutable upstream provenance and license notices.
- Every documentation status must distinguish implemented code, source checks, installed-package/VM acceptance and physical-device verification.
- Original Alcard work uses GPL-3.0-or-later. Preserve the separate upstream terms in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md); do not relabel GPL-2.0-only hardware material or CC0 fonts with the project default.
