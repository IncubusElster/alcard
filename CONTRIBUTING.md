# Contributing to Alcard

**English** · [Русский](CONTRIBUTING.ru.md)

Start with the [overview](docs/en/overview.md), [active roadmap](docs/en/roadmap.md) and [build guide](docs/en/building.md). Alcard targets Alpine aarch64 and Cardputer Zero. Base each change on the current source, documented contracts and acceptance scope.

## Code boundaries

| Area | Responsibility |
| --- | --- |
| `rust/` workspace | Ratatui Settings/Hello, setup backend and GUI-free shared crates |
| `rust/crates/alcard-ui` | Shared graphical presentation; independent workspace |
| `rust/apps/alcard-shell` | Keyboard-driven Shell and constrained radio helper; independent workspace |
| `rust/apps/alcard-polkit-agent` | Graphical polkit authentication; independent workspace |
| `rust/apps/alcard-lock` | Native session lock and PAM conversation; independent workspace |
| `packaging/apk`, `scripts`, `tools` | Package inputs, orchestration and verification |
| `board`, `patches`, `keyboard-zero`, `fonts` | Hardware-specific source, layout/font data and upstream adaptations |

Keep all five Cargo lockfiles. Console must not acquire Wayland or GLib dependencies through a shared crate. New settings operations belong in common functionality; UI-specific authentication and rendering remain frontend responsibilities. The existing `--hello` first-run contract and three installation modes are intentional.

## Checks appropriate to a change

Documentation and license changes start with:

```sh
bash build.sh audit
python3 tools/check-docs.py
```

For Rust changes, use the relevant locked workspace with the supported toolchain and native libraries. A typical console source check is:

```sh
cargo fmt --manifest-path rust/Cargo.toml --all --check
cargo test --manifest-path rust/Cargo.toml --locked --workspace --all-targets
cargo clippy --manifest-path rust/Cargo.toml --locked --workspace --all-targets -- -D warnings
```

Graphical source, native authentication, installed APKs and real VT/VM checks have additional prerequisites; follow [testing](docs/en/testing.md). Native fixture scripts can create users, modify isolated service configuration and boot guests. Run them only in their documented container/VM context.

Keep tests in separate `unit_tests`/integration files enabled only for testing. Production package builds use release binaries and must exclude test harnesses, examples and fixtures. Verify behavior at the boundary being changed; a text-pattern check is not a substitute for a PAM, NetworkManager, BlueZ or compositor scenario.

Use the single CI workflow with one runner and sequential stages. Documentation-only changes use static preflight; code/build changes add Alpine source checks. Full installed-package, VM and Hello acceptance is a manual mode. Inspect the [testing guide](docs/en/testing.md) before selecting a remote run; run only the level needed for the changed boundary.

## Package and release changes

APKBUILD is the source of package metadata. Builders and verifiers read package versions, filenames and kernel identity through [package_metadata.py](tools/package_metadata.py), which parses declarations without executing the recipe. Change recipe declarations and their exact dependency pins together; do not add another version list to a script. A new source declaration is not a newly accepted binary.

Record the source commit, toolchain, package versions and output hashes for acceptance. Distinguish source checks, installed-package checks, VM checks and physical-device verification. Preserve private signing keys and developer credentials outside tracked files; local build state belongs under `.alcard-local/`.

## Documentation and translation

English is primary. Every topic in `docs/en` has the same filename in `docs/ru`. Change both editions together, including the reciprocal language link, update date, commands, identifiers and limitations. Russian is a full technical translation, not a shortened summary. Root README and contributor/security guides also have Russian counterparts.

Keep machine identifiers unchanged: CLI flags, package names, file paths, configuration keys, hashes and the literal Hello choices `Graphics`, `Graphics Minimal`, `Console`. Translate explanatory prose naturally. Distinguish implementation status from acceptance status in both languages.

Current contracts belong in topical guides; readiness boundaries belong in [current status](docs/en/status.md). CI logs and machine-readable results are run artifacts. Keep release provenance with the distributed release. Maintain one active roadmap and do not add development diaries, migration maps or superseded specifications to the current source tree.

## Licensing and security reports

Original contributions use GPL-3.0-or-later under [NOTICE](NOTICE) and [LICENSE](LICENSE). Preserve upstream notices and record imported materials in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md); hardware and font exceptions keep their own terms. See [licensing](docs/en/licensing.md) for binary-distribution work still pending.

For security-sensitive reports, follow [SECURITY.md](SECURITY.md). Ordinary bug reports should include the source/package version, installation mode, device or VM, reproduction steps, expected behavior and a sanitized log. Remove passwords, private keys, tokens and unnecessary personal/network identifiers before sharing logs.
