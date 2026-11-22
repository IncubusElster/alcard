# Alcard session locker

[English security guide](../../../docs/en/security.md) · [Безопасность и блокировка на русском](../../../docs/ru/security.md)

`alcard-lock` is the independent `ext-session-lock` Wayland client. It uses the current user's real PAM conversation; Shell presence, a recovery journal or a successful renderer test is not an unlock authority. The installed Graphics supervisor keeps the locker independent of Shell restarts and handles recovery/service-loss policy.

`alcard-lock` — самостоятельный клиент Wayland `ext-session-lock`. Он использует настоящую PAM conversation текущего пользователя; присутствие Shell, журнал восстановления или успешная проверка отрисовки не дают права разблокировать сессию. Supervisor установленной Graphics-сессии отделяет locker от перезапусков Shell и реализует политику восстановления/потери служб.

- [Authentication and recovery contract (EN)](../../../docs/en/security.md) / [Контракт авторизации и восстановления (RU)](../../../docs/ru/security.md).
- [Idle, explicit lock and suspend ordering (EN)](../../../docs/en/power-audio.md) / [Простой, блокировка и порядок сна (RU)](../../../docs/ru/power-audio.md).
- [Native PAM, compositor and VM acceptance (EN)](../../../docs/en/testing.md) / [Проверки PAM, композитора и VM (RU)](../../../docs/ru/testing.md).

This graphical application has an independent Cargo workspace/lockfile. Integration fixtures are compiled only for tests and are not installed in release APKs. / У графического приложения собственные Cargo workspace/lockfile; integration fixtures собираются только для тестов и не устанавливаются в release APK.
