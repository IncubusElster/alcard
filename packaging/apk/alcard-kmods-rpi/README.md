# alcard-kmods-rpi

## English

Builds the twelve external vendor modules against the pinned Alpine linux-rpi ABI from M5Stack revision 4f7aeee9fc103ebb03d22cac476bb191ecfb321e, with the retained local patch. It does not replace upstream DRM with the vendor fbdev display driver. Kernel/BSP updates must stay coordinated, and the BQ27xxx/BQ27220 binding conflict requires the checks described in the hardware guide.

- [Current component guide](https://github.com/IncubusElster/alcard/blob/main/docs/en/hardware.md).
- [Related contract](https://github.com/IncubusElster/alcard/blob/main/docs/en/building.md).
- [Authoritative recipe](https://github.com/IncubusElster/alcard/blob/main/packaging/apk/alcard-kmods-rpi/APKBUILD) and [current package matrix](https://github.com/IncubusElster/alcard/blob/main/docs/en/packaging.md).
- [Current readiness and verification scope](https://github.com/IncubusElster/alcard/blob/main/docs/en/status.md).

## Русский

Собирает двенадцать внешних модулей производителя под зафиксированный ABI Alpine linux-rpi из M5Stack revision 4f7aeee9fc103ebb03d22cac476bb191ecfb321e с сохранённым локальным патчем. Не заменяет upstream DRM драйвером fbdev производителя. Обновления ядра/BSP должны оставаться согласованными; конфликт binding BQ27xxx/BQ27220 требует проверок из аппаратного руководства.

- [Действующее руководство компонента](https://github.com/IncubusElster/alcard/blob/main/docs/ru/hardware.md).
- [Связанный контракт](https://github.com/IncubusElster/alcard/blob/main/docs/ru/building.md).
- [Исходный рецепт](https://github.com/IncubusElster/alcard/blob/main/packaging/apk/alcard-kmods-rpi/APKBUILD) и [текущая матрица пакетов](https://github.com/IncubusElster/alcard/blob/main/docs/ru/packaging.md).
- [Текущая готовность и область проверок](https://github.com/IncubusElster/alcard/blob/main/docs/ru/status.md).

Package versions come from APKBUILD; verify the actual built and installed artifacts. / Версии пакетов берутся из APKBUILD; проверяйте фактически собранные и установленные артефакты.
