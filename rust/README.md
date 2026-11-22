# Alcard Rust components

[English documentation](../docs/en/overview.md) · [Документация на русском](../docs/ru/overview.md)

The console workspace contains Ratatui Settings/Hello, the local setup backend and GUI-free shared crates. `alcard-ui`, Shell, polkit-agent and locker have independent graphical workspaces and lockfiles. Preserve this boundary so Console does not acquire Wayland/GLib dependencies.

Manifests require Rust 1.96 or newer; the native Alpine source preflight requires at least Rust 1.96.1. Use committed lockfiles and the native libraries described in the build guide. Ordinary `alcard-settings` opens eight settings sections; `alcard-settings --hello` is restricted to first-run setup. NetworkManager is the common networking owner.

| Work | English | Русский |
| --- | --- | --- |
| Build environment and all package inputs | [Building](../docs/en/building.md) | [Сборка](../docs/ru/building.md) |
| Hello and session selection | [Installation](../docs/en/installation.md) | [Установка](../docs/ru/installation.md) |
| Shared settings and TUI | [Settings](../docs/en/settings.md) | [Настройки](../docs/ru/settings.md) |
| Authentication and recovery | [Security](../docs/en/security.md) | [Безопасность](../docs/ru/security.md) |
| Checks and acceptance scope | [Testing](../docs/en/testing.md) | [Проверки](../docs/ru/testing.md) |

Консольный workspace содержит Ratatui Settings/Hello, локальный setup backend и общие crates без GUI. У `alcard-ui`, Shell, polkit-agent и locker собственные графические workspaces и lockfiles. Это разделение сохраняет Console без зависимостей Wayland/GLib.

Manifests требуют Rust 1.96 или новее; предварительная проверка native-исходников Alpine требует как минимум Rust 1.96.1. Используйте committed lockfiles и native-библиотеки из руководства по сборке. Обычный `alcard-settings` открывает восемь разделов; `alcard-settings --hello` ограничен первоначальной настройкой. Общий владелец сети — NetworkManager.

Original Alcard code uses [GPL-3.0-or-later](../LICENSE); separately licensed inputs are listed in [third-party notices](../THIRD_PARTY_NOTICES.md). / Собственный код использует GPL-3.0-or-later; сторонние материалы сохраняют отдельные лицензии.
