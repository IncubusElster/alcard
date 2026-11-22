# Alcard graphical polkit agent

[English authentication guide](../../../docs/en/security.md) · [Авторизация на русском](../../../docs/ru/security.md)

`alcard-polkit-agent` is a standalone graphical authentication agent on SCTK/tiny-skia with native GLib/GIO and libpolkit integration. It presents the real polkit/PAM challenge; it does not grant system actions itself. The agent stays separate from Shell and the session locker. Ordinary TUI Settings currently uses terminal doas/su where required; the future GUI Settings transport must use the shared operation model with an appropriate graphical authorization adapter.

`alcard-polkit-agent` — самостоятельный графический агент авторизации на SCTK/tiny-skia с native GLib/GIO и интеграцией libpolkit. Он показывает настоящий запрос polkit/PAM и сам не разрешает системные действия. Агент отделён от Shell и блокировщика сессии. Обычные TUI-настройки сейчас при необходимости используют терминальные doas/su; будущему GUI Settings нужен общий контракт операций с подходящим графическим адаптером авторизации.

- [Agent lifecycle, identities and recovery (EN)](../../../docs/en/security.md) / [Жизненный цикл, identities и восстановление (RU)](../../../docs/ru/security.md).
- [Settings/backend boundary (EN)](../../../docs/en/settings.md) / [Граница настроек и backend (RU)](../../../docs/ru/settings.md).
- [Native authentication acceptance (EN)](../../../docs/en/testing.md) / [Приёмка native-авторизации (RU)](../../../docs/ru/testing.md).

The agent has its own workspace/lockfile so the console and Shell do not inherit its GLib dependencies. Real authentication fixtures are test-only. / У агента собственные workspace/lockfile, чтобы Console и Shell не наследовали GLib; fixtures настоящей авторизации подключаются только при тестировании.
