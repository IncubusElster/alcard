# alcard-firmware-zero

## English

Data-only firmware APK: generated cardputerzero,st7789v.bin display initialization and the exact 8192-byte BMI270 configuration blob. The LCD table is adapted from M5Stack code under GPL-2.0-or-later; the BMI270 bytes match the Bosch configuration under BSD-3-Clause. The recipe verifies SHA-256 2d75e68e343a13ff99be98261dfbd99d9e8c6f267da9e3c5aeb883276ad178db and installs the Bosch notice in /usr/share/licenses/alcard-firmware-zero/. This identity check does not certify other vendor blobs or physical display orientation, colors and offset.

- [Current component guide](https://github.com/IncubusElster/alcard/blob/main/docs/en/hardware.md).
- [Related contract](https://github.com/IncubusElster/alcard/blob/main/docs/en/licensing.md).
- [Authoritative recipe](https://github.com/IncubusElster/alcard/blob/main/packaging/apk/alcard-firmware-zero/APKBUILD) and [current package matrix](https://github.com/IncubusElster/alcard/blob/main/docs/en/packaging.md).
- [Current readiness and verification scope](https://github.com/IncubusElster/alcard/blob/main/docs/en/status.md).

## Русский

Firmware APK только с данными: генерируемая инициализация дисплея cardputerzero,st7789v.bin и конкретный 8192-байтный конфигурационный blob BMI270. LCD-таблица адаптирована из M5Stack под GPL-2.0-or-later; байты BMI270 совпадают с конфигурацией Bosch под BSD-3-Clause. Рецепт проверяет SHA-256 2d75e68e343a13ff99be98261dfbd99d9e8c6f267da9e3c5aeb883276ad178db и устанавливает уведомление Bosch в /usr/share/licenses/alcard-firmware-zero/. Это совпадение не подтверждает другие blobs производителя или ориентацию, цвета и смещение физического дисплея.

- [Действующее руководство компонента](https://github.com/IncubusElster/alcard/blob/main/docs/ru/hardware.md).
- [Связанный контракт](https://github.com/IncubusElster/alcard/blob/main/docs/ru/licensing.md).
- [Исходный рецепт](https://github.com/IncubusElster/alcard/blob/main/packaging/apk/alcard-firmware-zero/APKBUILD) и [текущая матрица пакетов](https://github.com/IncubusElster/alcard/blob/main/docs/ru/packaging.md).
- [Текущая готовность и область проверок](https://github.com/IncubusElster/alcard/blob/main/docs/ru/status.md).

Package versions come from APKBUILD; verify the actual built and installed artifacts. / Версии пакетов берутся из APKBUILD; проверяйте фактически собранные и установленные артефакты.
