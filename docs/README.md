# Alcard documentation / Документация Alcard

**English is the primary edition. Русский — полный поддерживаемый перевод.**

Each topic has the same filename in `en/` and `ru/`, a reciprocal language link and an update date. Commands, package identities and validation boundaries must agree across both editions. Runtime UI localization is separate: the software currently includes EN/RU/ES strings.

У каждой темы одинаковое имя файла в `en/` и `ru/`, ссылка на другую языковую редакцию и дата обновления. Команды, версии пакетов и границы подтверждённых проверок должны совпадать. Локализация интерфейса — отдельная часть: программы сейчас содержат строки EN/RU/ES.

| Topic / Тема | English | Русский |
| --- | --- | --- |
| Scope, architecture and current state / Область, архитектура и состояние | [Overview](en/overview.md) | [Обзор](ru/overview.md) |
| Complete package/image build route / Полная сборка пакетов и образов | [Building](en/building.md) | [Сборка](ru/building.md) |
| Hello, session choices and first login / Hello, режимы и первый вход | [Installation](en/installation.md) | [Установка](ru/installation.md) |
| Everyday TUI settings / Повседневные TUI-настройки | [Settings](en/settings.md) | [Настройки](ru/settings.md) |
| Shell, applications, windows and notifications / Shell, приложения, окна и уведомления | [Desktop](en/desktop.md) | [Рабочий стол](ru/desktop.md) |
| Authentication, locking and recovery / Авторизация, блокировка и восстановление | [Security](en/security.md) | [Безопасность](ru/security.md) |
| Display, audio, idle, sleep and battery / Экран, звук, простой, сон и батарея | [Power and audio](en/power-audio.md) | [Питание и звук](ru/power-audio.md) |
| Board variants, pins, display and drivers / Ревизии, GPIO, дисплей и драйверы | [Hardware](en/hardware.md) | [Аппаратная часть](ru/hardware.md) |
| Boot layout, A/B design and update limits / Загрузка, A/B и ограничения обновлений | [Boot and updates](en/boot-updates.md) | [Загрузка и обновления](ru/boot-updates.md) |
| APK versions, ownership and dependencies / Версии APK, владение файлами и зависимости | [Packaging](en/packaging.md) | [Упаковка](ru/packaging.md) |
| Fonts, glyph coverage and input layouts / Шрифты, глифы и раскладки | [Fonts and keyboard](en/fonts-keyboard.md) | [Шрифты и клавиатура](ru/fonts-keyboard.md) |
| Source, package, VM and hardware checks / Проверки исходников, APK, VM и устройства | [Testing](en/testing.md) | [Проверки](ru/testing.md) |
| Active work and completion criteria / Активные задачи и критерии завершения | [Roadmap](en/roadmap.md) | [План](ru/roadmap.md) |
| Current readiness and validation boundaries / Текущая готовность и границы проверок | [Current status](en/status.md) | [Текущий статус](ru/status.md) |
| License scope and third-party provenance / Лицензии и происхождение компонентов | [Licensing](en/licensing.md) | [Лицензирование](ru/licensing.md) |

## Contributing / Участие

[Contributor guide](../CONTRIBUTING.md) · [Руководство участника](../CONTRIBUTING.ru.md) · [Repository instructions](../AGENTS.md)

Keep the guides focused on current behavior and commands. The status page distinguishes implemented features, checks and pending acceptance. CI logs/results belong to run artifacts; distributed releases carry their own source and package provenance.

Описывайте в руководствах текущее поведение и команды. Страница статуса разделяет реализованные функции, проверки и предстоящую приёмку. Логи/результаты CI относятся к артефактам прогонов; распространяемые релизы содержат собственные сведения об исходниках и пакетах.
