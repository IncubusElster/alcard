# Third-party notices and provenance

[English licensing guide](docs/en/licensing.md) · [Лицензирование на русском](docs/ru/licensing.md)

The project's default **GPL-3.0-or-later** license covers original Alcard work as described in [NOTICE](NOTICE). The separately identified materials below retain their own terms. This registry records the inputs used by the current source and package recipes; it does not relicense upstream projects or their existing releases.

## Material kept in the source tree

| Material | Source and modifications | Applicable terms |
| --- | --- | --- |
| [Cardputer Zero DTS files](board/cardputer-zero/device-tree) | Adapted from M5Stack `m5stack-linux-dtoverlays` at `4f7aeee9fc103ebb03d22cac476bb191ecfb321e`; each file records its upstream path and board changes | GPL-2.0-only; [license text](licenses/GPL-2.0.txt) and the retained file notices |
| [ST7789v3 initialization table](board/cardputer-zero/firmware/st7789v3.toml) | Adapted from `modules/st7789v-1.0/st7789v_m5stack.c` at the same M5Stack revision, using `HSD20_IPS`, `init_display()`, `set_var()` and the IPS gamma table | GPL-2.0-or-later; upstream SPDX is `GPL-2.0+`, copyright Dennis Menschel, 2015. [GPL v2 text](licenses/GPL-2.0.txt) |
| [Alcard Pixel Bold](fonts/alcard-pixel/source/AlcardPixel-Bold.hex) | Byte-identical to Viznut UNSCII `fontfiles/unscii-8.hex` at `257b4dfeb50651510fe8b9cf4f151a148a4490e4`; renamed | Public Domain / CC0-1.0; [upstream declaration and Alcard notice](licenses/UNSCII-CC0-notice.txt), [CC0 text](licenses/CC0-1.0.txt) |
| [Alcard Pixel Regular](fonts/alcard-pixel/source/AlcardPixel-Regular.hex) | Alcard contributor's edits of UNSCII `fontfiles/unscii-8-thin.hex` at the same reviewed revision; 239 changed bitmaps, identical codepoint set | CC0-1.0 for this font data, including the existing Alcard edits; same notice and text as Bold |
| License documents | Original GNU, Creative Commons and Bosch license/notice texts | Their own copying terms; the project GPL default does not rewrite them |

The font generator is original Alcard software covered by the project default. Generated font data keeps the input font's CC0 designation. Shell embeds the Regular font data; this does not turn the entire software project into CC0. The project does not include `unscii-16-full`, GNU Unifont font data, or proprietary Sony Ericsson font files.

## Material fetched by the builders

| Input | Pinned origin | Handling |
| --- | --- | --- |
| M5Stack external kernel drivers and board reference | [m5stack-linux-dtoverlays @ 4f7aeee9](https://github.com/m5stack/m5stack-linux-dtoverlays/tree/4f7aeee9fc103ebb03d22cac476bb191ecfb321e) | Fetched into ignored local build storage; upstream [COPYING](https://github.com/m5stack/m5stack-linux-dtoverlays/blob/4f7aeee9fc103ebb03d22cac476bb191ecfb321e/COPYING) and per-file notices apply. The module APK declares GPL-2.0-only; local patches remain part of the corresponding source. |
| BMI270 configuration firmware | `modules/imu-1.0/bmi270-init-data.fw` in the pinned M5Stack tree | The exact 8192-byte blob matches Bosch `bmi270_config_file[]`; BSD-3-Clause notice is installed by the firmware APK. This verification applies only to this blob. |
| Linux and Alpine packages | `linux-rpi=6.18.60-r0`, Alpine `v3.24`, plus declared runtime/build dependencies | Separate upstream packages retain their own licenses and notices. A release must record the actual resolved APK inventory; the branch URL does not freeze every dependency version. |
| Rust dependencies | The five committed Cargo lockfiles | Not vendored into this source tree. Their individual licenses remain applicable; generated binary distributions must include the required dependency notices. A complete binary dependency-notice bundle remains a release task. |

## Verified font and firmware identity

| Data | SHA-256 |
| --- | --- |
| Alcard Pixel Bold / upstream UNSCII 8 | `03094f7fbab7085cf6a6b624cee61e47e71ce5d0c2f308c2f4436afdc17f776c` |
| BMI270 configuration, 8192 bytes | `2d75e68e343a13ff99be98261dfbd99d9e8c6f267da9e3c5aeb883276ad178db` |

The BMI270 reference is Bosch Sensortec [bmi270.c](https://github.com/boschsensortec/BMI270_SensorAPI/blob/41129fcfe39c583ee5462d79195741945d51c1fe/bmi270.c) at `41129fcfe39c583ee5462d79195741945d51c1fe`. The retained [Bosch notice](licenses/Bosch-BMI270-BSD-3-Clause.txt) matches [upstream LICENSE](https://github.com/boschsensortec/BMI270_SensorAPI/blob/41129fcfe39c583ee5462d79195741945d51c1fe/LICENSE). The [firmware recipe](packaging/apk/alcard-firmware-zero/APKBUILD) checks the hash and installs the notice under `/usr/share/licenses/alcard-firmware-zero/`.

The upstream font declaration is in [UNSCII README](https://github.com/viznut/unscii/blob/257b4dfeb50651510fe8b9cf4f151a148a4490e4/README.md). The two Alcard inputs and the generator remain in the repository, so font outputs can be regenerated without retrieving an unversioned external font.

## Release obligations still to implement

Keep the exact corresponding source and patches available for distributed GPL components. Preserve upstream copyright notices and required notices for dependencies. Include the required license material in the distributed source/package bundle, not only in a link to the current branch. Package builders currently install the Bosch notice but do not yet assemble a complete license bundle for every Alcard APK. Documenting this limitation does not mark binary-release compliance work complete.

The GPL-2.0-only hardware material is not relabeled GPL-3.0-or-later by this registry. Any future source-code combination or new imported component requires checking its own terms before integration.
