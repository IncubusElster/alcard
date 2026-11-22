# Licensing and third-party provenance

**Language:** English · [Русский](../ru/licensing.md)

> Scope: current source tree · Updated: 2026-11-22

## Project license

Original Alcard software, build tools, configuration and documentation are licensed under **GNU GPL version 3 or any later version**, SPDX `GPL-3.0-or-later`. The complete English license is in [LICENSE](../../LICENSE); [NOTICE](../../NOTICE) states its scope and warranty disclaimer. Cargo manifests and Alcard APK recipes declare the applicable license; inherited Cargo declarations use the workspace value.

The package versions described under [packaging](packaging.md) are source recipe values. A license declaration applies to the identified material; changing metadata alone does not produce or accept a new binary distribution.

The default license does not override separate upstream licenses or license documents. Use the [third-party registry](../../THIRD_PARTY_NOTICES.md) when copying, modifying or distributing a specific component.

## Exceptions and source identity

| Material | License and retained provenance |
| --- | --- |
| M5Stack-derived DTS and external kernel-module source | GPL-2.0-only, with original notices; M5Stack commit `4f7aeee9fc103ebb03d22cac476bb191ecfb321e`. Alcard's GPL v3 default does not relicense this material. |
| ST7789v3 initialization data | GPL-2.0-or-later; adapted from the pinned M5Stack `st7789v_m5stack.c`, upstream SPDX `GPL-2.0+`, copyright Dennis Menschel, 2015. |
| BMI270 configuration firmware | BSD-3-Clause for the exact Bosch-derived blob, 8192 bytes; the [Bosch notice](../../licenses/Bosch-BMI270-BSD-3-Clause.txt) is retained and installed by its APK. |
| Alcard Pixel Regular and Bold font data | CC0-1.0, including the existing Regular edits; retained [UNSCII declaration](../../licenses/UNSCII-CC0-notice.txt) and [CC0 text](../../licenses/CC0-1.0.txt). |
| Alpine packages, Rust crates and other external tools | Their respective upstream licenses; a source reference or runtime dependency is not a transfer of ownership to Alcard. |

The [GPL v2 text](../../licenses/GPL-2.0.txt) is included for the hardware material. The firmware APK's combined declaration `GPL-2.0-or-later AND BSD-3-Clause` reflects its two different data inputs. It does not imply that all firmware bytes have a single interchangeable license.

### Fonts

The reviewed UNSCII revision is `257b4dfeb50651510fe8b9cf4f151a148a4490e4`. Its [README declaration](https://github.com/viznut/unscii/blob/257b4dfeb50651510fe8b9cf4f151a148a4490e4/README.md) allows Public Domain/CC0 treatment of the normal font variants and separately identifies the Unifont-derived exceptions. Alcard's inputs are the 8-pixel normal and thin variants, not `unscii-16-full`.

Bold is byte-identical to upstream `fontfiles/unscii-8.hex`, SHA-256 `03094f7fbab7085cf6a6b624cee61e47e71ce5d0c2f308c2f4436afdc17f776c`. Regular retains the 3191-record codepoint set of `unscii-8-thin.hex` with 239 changed bitmaps. The count includes U+0000; the renderer exposes 3190 usable Unicode glyphs. The 518 differences between Alcard Regular and Bold measure a different comparison. See [fonts and keyboard](fonts-keyboard.md) for generation, coverage and display limits.

The generator's GPL license covers the software; it does not replace the CC0 designation of the input/output font data. Proprietary Sony Ericsson font files are not distributed by this repository.

### BMI270 and LCD data

The BMI270 bytes fetched from the pinned M5Stack tree match `bmi270_config_file[]` in Bosch [bmi270.c](https://github.com/boschsensortec/BMI270_SensorAPI/blob/41129fcfe39c583ee5462d79195741945d51c1fe/bmi270.c) at `41129fcfe39c583ee5462d79195741945d51c1fe`. Their SHA-256 is `2d75e68e343a13ff99be98261dfbd99d9e8c6f267da9e3c5aeb883276ad178db`. The recipe checks this exact identity; the verification does not cover arbitrary vendor firmware.

The display table is adapted from M5Stack and converted to the Linux MIPI DBI initialization format. Its original source and selected panel functions are recorded in [st7789v3.toml](../../board/cardputer-zero/firmware/st7789v3.toml). Source attribution and successful conversion are independent from the still-open physical display acceptance.

## Publishing source and distributing binaries

For source distributions, include the license texts, notices, corresponding Alcard source and local patches. Keep provenance pinned to immutable revisions. Review newly imported code or data before assigning a license to it; the repository default does not replace an imported file's terms.

For binary distributions, preserve the required dependency notices and provide the corresponding source for the distributed GPL components under their applicable terms. Record the source commit, Rust lockfiles, toolchain, actual Alpine package inventory and artifact hashes. The current builders install the Bosch notice, but a complete license/notice bundle for all Alcard binaries and dependencies has not yet been assembled. That remains a [release task](roadmap.md).

The full license texts remain in their original language. The Russian guide translates the project's scope, component map and release process; it does not replace the legal texts.

## Primary references

- [GNU GPL v3](https://www.gnu.org/licenses/gpl-3.0.html) and [applying GNU licenses](https://www.gnu.org/licenses/gpl-howto.html).
- [M5Stack COPYING at the pinned revision](https://github.com/m5stack/m5stack-linux-dtoverlays/blob/4f7aeee9fc103ebb03d22cac476bb191ecfb321e/COPYING).
- [Bosch BSD-3-Clause license at the verified revision](https://github.com/boschsensortec/BMI270_SensorAPI/blob/41129fcfe39c583ee5462d79195741945d51c1fe/LICENSE).
- [UNSCII declaration at the reviewed revision](https://github.com/viznut/unscii/blob/257b4dfeb50651510fe8b9cf4f151a148a4490e4/README.md).
