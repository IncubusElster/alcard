# Alcard — первое открытие исходников

**Экспериментальная публикация только исходников. Без поддерживаемого образа устройства и стабильного релиза.**

Alcard — компактный проект Alpine Linux для M5Stack Cardputer Zero с OpenRC, APK, Hello и Settings на Rust/Ratatui и управляемой клавиатурой Wayland Shell.

## Состав исходников

- Профили v5/v5-lite, файлы kernel/BSP и инструменты сборки образа.
- Hello с выбором Graphics, Graphics Minimal и Console.
- TUI Settings, общие операции, NetworkManager.
- Alcard Shell, polkit-agent, PAM-блокировщик, PipeWire/WirePlumber.
- Техническая документация EN/RU и локализация интерфейса EN/RU/ES.

## Программная приёмка

[Полный Alpine ARM64 acceptance](https://github.com/IncubusElster/alcard/actions/runs/45096681285) успешно прошёл на `164878f686511ed63d6471da30f2cd818e667829`: девять подписанных APK, установленный runtime, три режима входа в VM, интерактивный Hello, 26 сценариев Settings и сохранение после перезагрузки. Службы NetworkManager/BlueZ на приватной D-Bus и аппаратные фикстуры имитировались. Установленные Settings использовали настоящий NetworkManager на виртуальном Ethernet.

Физические дисплей, клавиатура, Wi-Fi, Bluetooth, аудиокодек, батарея, сон и энергопотребление **не приняты**. Поддерживаемого образа, APK-канала Alcard, OTA-отката и отдельного GUI Settings пока нет. Уведомления бинарных зависимостей и проверки безопасности релизного образа ещё предстоят.

См. [README](README.ru.md), [сборку](docs/ru/building.md), [статус](docs/ru/status.md) и [план](docs/ru/roadmap.md). Собственный код Alcard использует GPL-3.0-or-later; см. [NOTICE](NOTICE) и [лицензии сторонних компонентов](THIRD_PARTY_NOTICES.md).

Эти заметки не создают GitHub tag или бинарный release.
