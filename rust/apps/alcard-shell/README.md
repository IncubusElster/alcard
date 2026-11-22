# Alcard Shell

[English desktop guide](../../../docs/en/desktop.md) · [Руководство Shell на русском](../../../docs/ru/desktop.md)

Alcard Shell is a keyboard-operated Wayland client using SCTK, tiny-skia and `wl_shm`. It owns Home, application/recent-window navigation, notifications and quick settings. The graphical polkit agent and session locker are separate processes. Graphics uses a supervised Shell session; Graphics Minimal remains the independent Foot profile.

| Topic | English | Русский |
| --- | --- | --- |
| Screens, shortcuts, themes and application discovery | [Desktop](../../../docs/en/desktop.md) | [Рабочий стол](../../../docs/ru/desktop.md) |
| Idle, display, audio, suspend and battery | [Power and audio](../../../docs/en/power-audio.md) | [Питание и звук](../../../docs/ru/power-audio.md) |
| Locking, polkit and service-loss recovery | [Security](../../../docs/en/security.md) | [Безопасность](../../../docs/ru/security.md) |
| Package installation and mode boundaries | [Installation](../../../docs/en/installation.md) | [Установка](../../../docs/ru/installation.md) |
| Source and runtime checks | [Testing](../../../docs/en/testing.md) | [Проверки](../../../docs/ru/testing.md) |

Alcard Shell — управляемый клавиатурой клиент Wayland на SCTK, tiny-skia и `wl_shm`. Он отвечает за Home, приложения/недавние окна, уведомления и быстрые настройки. Графический polkit-agent и блокировщик сессии работают отдельными процессами. Graphics использует контролируемую Shell-сессию; Graphics Minimal сохраняет отдельный профиль Foot.

This application has its own Cargo workspace/lockfile. Tests are separate source modules; production packages use release binaries. Read [current status](../../../docs/en/status.md) for verification scope and pending release work.

У приложения собственные Cargo workspace/lockfile. Тесты находятся в отдельных исходных модулях; production-пакеты используют release-бинарники. Область проверок и предстоящие работы для выпуска описаны в [текущем статусе](../../../docs/ru/status.md).
