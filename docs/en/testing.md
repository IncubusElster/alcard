# Testing and acceptance

**Language:** English · [Русский](../ru/testing.md)

> Scope: current source tree · Updated: 2026-11-22

Alcard records what a check actually exercised. Source tests, installed packages, a real login in a VM and a physical device are separate levels of evidence. A green result at one level does not imply success at the next.

Read [current status](status.md) for readiness boundaries and the current CI run for the exact source, artifacts and result. This guide describes the available verification routes; a documented check is not an assertion that it has run.

## Choose the smallest relevant check

| Level | What it verifies | Environment and effects | What it does not establish |
| --- | --- | --- | --- |
| Static source | Script syntax, package relationships, declared assets, localization keys and configuration structure | Local Bash/sh/Python; no package installation, cargo build, privilege escalation or network in `audit-build.sh` | Compilation or actual APK dependency resolution |
| Rust source | Formatting, unit tests, Clippy, compilation and dependency boundaries | Native toolchain; writes build/cache files and may fetch dependencies unless offline | Installed package content, real login or device behavior |
| Process fixtures | Supervisor, dispatcher, worker and failure behavior with controlled child processes | Temporary files/processes and isolated test paths | A production login or real system service policy |
| Private D-Bus integration | Production NetworkManager/BlueZ client requests, ownership, replies and cancellation | A private `dbus-daemon` and explicitly simulated service peers; no system bus or radio | Real daemon policy, Wi-Fi authentication or physical Bluetooth interoperability |
| Native installed session | Fresh signed APKs, installed binaries/assets, actual compositor/PAM or isolated audio as defined by each scenario | Disposable native container and dedicated test users/services | Physical GPIO, radio, battery, kernel sleep or an entire booted system |
| Login VM | First-run provisioning, PAM/elogind session, policy and recovery across three installation modes | One isolated aarch64 QEMU base, sequential modes and disposable credentials | CM0 boot firmware, board drivers, physical input or energy use |
| Physical Cardputer | The actual board, display, keyboard, interfaces, power and update path | Recorded board/SKU, kernel, packages and manual observations | Other hardware revisions or untested kernel versions |

## Static preflight

From the repository root:

```sh
bash build.sh audit
python3 tools/check-docs.py
```

This dispatches to [audit-build.sh](../../scripts/audit-build.sh). It checks Bash and selected POSIX scripts, all APKBUILD syntax, Python parsing, the full-image package contract, required assets, keyboard layouts and EN/RU/ES catalog consistency. It does not install dependencies or build packages.

[package_metadata.py](../../tools/package_metadata.py) reads current recipe versions and kernel identity without executing APKBUILD. The audit checks exact own-package/kernel dependency pins for consistency. The recipes remain the source of version choices; a stale dependency pin fails rather than being silently accepted.

For a documentation-only change, check both language copies, local links/anchors, commands, identifiers and validation boundaries. The documentation checker compares all topic pairs and the root entry points. Generated CI results remain run artifacts.

## Rust source checks

There are five independent manifests/lockfiles to check:

| Manifest | Scope |
| --- | --- |
| [`rust/Cargo.toml`](../../rust/Cargo.toml) | Console Settings/Hello, setup backend and shared non-GUI crates |
| [`rust/crates/alcard-ui/Cargo.toml`](../../rust/crates/alcard-ui/Cargo.toml) | Shared graphical presentation |
| [`rust/apps/alcard-shell/Cargo.toml`](../../rust/apps/alcard-shell/Cargo.toml) | Shell and radio helper |
| [`rust/apps/alcard-polkit-agent/Cargo.toml`](../../rust/apps/alcard-polkit-agent/Cargo.toml) | Graphical polkit agent |
| [`rust/apps/alcard-lock/Cargo.toml`](../../rust/apps/alcard-lock/Cargo.toml) | Independent session locker |

Use the native dependencies from [building](building.md). The native source preflight requires Alpine Rust **1.96.1 or newer** and a musl host matching `apk --print-arch`.

A full source pass in an already prepared native environment is:

```sh
for project in rust rust/crates/alcard-ui rust/apps/alcard-shell rust/apps/alcard-polkit-agent rust/apps/alcard-lock; do
  cargo fmt --manifest-path "$project/Cargo.toml" --all --check
  cargo test --manifest-path "$project/Cargo.toml" --locked --all-targets
  cargo clippy --manifest-path "$project/Cargo.toml" --locked --all-targets -- -D warnings
  cargo build --manifest-path "$project/Cargo.toml" --locked
done
python3 tools/audit-shell.py
```

`audit-shell.py` calls `cargo metadata --locked` to inspect resolved dependency graphs. It therefore requires Cargo and available dependencies; it is not a Python-only offline syntax check. It verifies, among other boundaries, that Console remains GUI-free and that Shell does not absorb authentication dependencies.

The existing host-side shortcuts use the prepared Alpine aarch64 Distrobox:

```sh
bash build.sh polkit-check
bash build.sh shell-check
bash build.sh lock-check
```

These commands select distribution `/usr/bin` Rust tools and isolated Cargo directories to avoid shared-home host shims. They do not replace the separate console workspace check. They also do not prove that the graphical agent or locker has authenticated a real login.

### Offline native Alpine source pass

[check-alpine-source.sh](../../scripts/check-alpine-source.sh) supports Alpine 3.24 **x86_64 or aarch64** and does not create the environment itself:

```sh
bash scripts/check-alpine-source.sh preflight
bash scripts/check-alpine-source.sh source
```

Populate its native Cargo cache first, or point `ALCARD_CARGO_HOME` at an existing compatible cache with all five projects' dependencies. Source mode uses `--offline --locked`; it does not silently install missing prerequisites. Preflight checks tools, native libraries, PAM development files, the cache and AF_UNIX socket availability. Exit status 2 means blocked prerequisites, not a failed application test. Source mode creates a new evidence directory and fingerprints inputs before/after the run.

An aarch64 success does not establish x86_64 acceptance. Record each actually executed architecture separately.

## Process fixtures and production package checks

The following source tests exercise session supervision with controlled subprocesses rather than touching an existing desktop session:

```sh
python3 tools/test-session-client.py --shell /bin/sh
python3 tools/test-audio-session.py --shell /bin/sh
python3 tools/test-tty1-dispatch.py --shell /bin/sh
```

Use `/bin/ash` in Alpine to validate its actual POSIX shell. Temporary fixture directories and child processes are part of these tests. Their success is not evidence that a device's audio codec, PAM configuration or radio works.

Production package builders use `--release --locked --bins`. [verify-production-build.py](../../tools/verify-production-build.py) rejects test, example and benchmark compiler artifacts, checks expected release executables, and compares installed ELF hashes and exact package file lists against those builds. Tests remain in separate source files under `#[cfg(test)]`; removing them from the repository is not a release requirement.

After building the required signed session APKs, the supported local package installation check is:

```sh
bash build.sh shell-install-verify
```

It uses a disposable rootfs under `.alcard-local/`. Read [packaging](packaging.md) for the exact package set and install behavior. A package-content success is still distinct from login/VM acceptance.

## One workflow and one runner

[Alcard CI](../../.github/workflows/ci.yml) is the only workflow. Each run has one `ubuntu-24.04-arm` job. A repository-wide concurrency group prevents overlapping runs; stages inside the job are sequential. There is no architecture or login-mode job matrix.

| Trigger/mode | Work performed |
| --- | --- |
| Documentation/license-only push or pull request | Static source audit, documentation/link checks and inexpensive CI/metadata fixtures |
| Code, build input or workflow change | The same preflight plus native Alpine source checks |
| Manual `source` (default) | Preflight and all five native Alpine workspace checks |
| Manual `acceptance` | Preflight, source checks once, private D-Bus cases, fresh current APKs, installed runtime, three login modes, interactive Hello and installed Settings |

Path selection includes board inputs, patches, package recipes, scripts, tools, keyboard/font data, Rust and the workflow itself. Unknown or incompletely compared paths select the source gate conservatively. Choosing a manual mode is an explicit cost decision; ordinary documentation work does not build APKs or boot a VM.

The host orchestrator [run.sh](../../scripts/ci/run.sh) requires one disposable non-root Actions runner; it deliberately refuses an ordinary workstation invocation. [container.sh](../../scripts/ci/container.sh) installs dependencies and isolated test services only inside its disposable Alpine container, with source/package builds under a normal build user. Do not run fixture root-setup portions on a workstation or installed Cardputer.

Source mode has a 25-minute internal budget and a 35-minute workflow limit. Acceptance has a 55-minute internal budget and a 65-minute workflow limit; the additional workflow allowance covers setup, cache and artifact handling. A timeout is a failed or cancelled check, not partial acceptance.

### Source mode

The source gate runs static preflight, formatting, tests, Clippy and compilation across all five workspaces, then the dependency architecture audit and session/audio/tty dispatcher fixtures. The native source helper uses the prepared Alpine toolchain and locked dependencies. Report the actual architecture; ARM64 success does not imply an x86_64 run.

The inexpensive orchestration fixtures can also run locally:

```sh
python3 scripts/ci/test-select.py
python3 tools/test-package-metadata.py
python3 scripts/ci/settings/test-observer.py
```

They test path/mode selection, metadata parsing/drift and the private terminal observer's framing and redaction. They do not replace the application source or runtime gates.

### Manual acceptance mode

Select the desired ref in GitHub Actions and choose `acceptance`. All stages use the current checkout and run sequentially on the same runner:

1. Perform the native Alpine source gate once, then explicitly run the NetworkManager/BlueZ private-bus integration cases as the ordinary build user.
2. Build a fresh signed set of all nine user/session APKs and verify production binaries and payloads.
3. Install that exact set into the disposable native environment and exercise [session runtime](../../scripts/check-session-runtime.sh).
4. Stage the same signed APKs for one login guest base; do not rebuild a different set between levels.
5. Exercise Graphics, Graphics Minimal and Console sequentially.
6. Run interactive Hello, installed Settings, reboot/persistence checks and the bounded idle-measurement scenario with those same APKs through the [Hello harness](../../scripts/ci/hello). The [Settings harness](../../scripts/ci/settings) reuses this persistent guest and its provisioned ordinary account.

Native cases include real compositor/PAM and isolated PipeWire where specified; battery/backlight and mutating login1 operations use controlled fixtures. Virtual output hotplug does not establish SPI display hotplug or physical power savings. The VM uses Alpine userland on its virtual kernel, not the Cardputer firmware/BSP. See [power and audio](power-audio.md) and [security](security.md) for feature boundaries.

The workflow caches dependencies and compiler output under `.alcard-local/ci-cache/`; it does not substitute a cached accepted APK set. Packages use ephemeral CI signing keys, not a public release key. Private guest credentials, full guest disks and signing private keys are excluded from uploaded evidence and cleaned by the harness.

### Results and artifacts

Public output is collected under `.alcard-local/ci/`, `.alcard-local/ci-packages/` and `.alcard-local/ci-login/evidence/`. Native stage receipts are under `.alcard-local/ci/source/` or `.alcard-local/ci/acceptance/`. The workflow uploads `alcard-CI-ARM64-${run_id}-${run_attempt}` with three-day retention. Inspect both the stage result and the tested source/package identity.

CI output belongs to the run, not to a growing collection of committed checkpoint files. A distributed release must retain its own durable source SHA, toolchain/container identity, package inventory, binary/image hashes, notices and required source. Short-lived CI artifact retention is not a release archive.

## NetworkManager and Settings acceptance boundaries

The source suite retains parser, IPv4 argument and TUI rendering checks. The acceptance mode adds two separate integration boundaries; inspect their individual receipts before claiming a pass.

The persistent Hello guest uses an isolated virtual Ethernet interface and the actual installed NetworkManager service. Settings runs as the provisioned ordinary user on a controlling terminal. The harness prepares conditions and reads back state; the production TUI performs the requested mutations. Scenarios cover a newly created profile, activation, static IPv4/DHCP transitions, service unavailability, doas and root-password/su, refusal/Ctrl+C and terminal restoration, the shared system language, and reboot persistence. The normal three-mode login pass separately checks Console after removal of graphical packages.

The [private-bus runner](../../scripts/ci/settings-services.py) executes the production client/agent code against simulated NetworkManager and BlueZ peers. The cases check operation ownership, delayed replies, service/device disappearance and Bluetooth prompt/cancel lifecycles without depending on radio hardware. Their result must name the simulated participant. Passing these cases does not prove that real NetworkManager accepts a Wi-Fi password or that a physical Bluetooth device pairs.

Open/protected Wi-Fi, wrong credentials, the actual adapter's saved-profile reconnect behavior, RF conditions and physical Bluetooth interoperability remain separate acceptance work. An Ethernet DHCP result is evidence for Ethernet and the exercised Settings path. It is not Wi-Fi authentication evidence. Physical adapters and platform behavior require the intended device or a separately identified radio-capable test platform.

## Physical acceptance and release evidence

For each board test, record SKU/revision, source SHA, image/package hashes, kernel release/ABI and relevant firmware. Start with cold boot, persistent ROOT, built-in DRM/TTY and keyboard; then verify the hardware matrix, login/UI, network/audio/power behavior and recovery. The [hardware guide](hardware.md) and [boot/update guide](boot-updates.md) contain the acceptance details.

A release receipt should distinguish PASS, FAIL, BLOCKED, NOT RUN and CANCELLED, identify the tested SHA and package set, and retain failures instead of editing them away. An ignored integration test is not a pass. A composite result must name the successful subsets and prove the package/source relationship; it must not change the overall result of an earlier failed run.

Resource measurements must state VM/device, RAM/vCPU configuration, settling/sample interval, PSS versus RSS, CPU denominator and instrumentation overhead. Use summed PSS for process groups, not summed RSS. Two memory endpoints are not a peak or leak test. A VM measurement does not establish Cardputer resource use or a controlled comparison between two revisions.
