# Fonts and keyboard layouts

**Language:** English · [Русский](../ru/fonts-keyboard.md)

> Scope: current source tree · Updated: 2026-11-22

Alcard keeps physical key handling, language/layout choice and text rendering separate. The console remains usable without the graphical font or Wayland packages. System language is a single setting; supported keyboard layouts are a separate selection, with Original US always retained.

## Alcard Pixel sources and provenance

The [font source directory](../../fonts/alcard-pixel/source/) contains two Unicode HEX files:

| File | Origin and purpose |
| --- | --- |
| `AlcardPixel-Regular.hex` | User-modified UNSCII 8×8 thin bitmap; the normal compact UI face |
| `AlcardPixel-Bold.hex` | Original denser UNSCII 8×8 bitmap, used as the Bold face |

Each source contains 3,191 Unicode records. U+0000 is a special 16-byte zero bitmap; the remaining 3,190 records are usable 8×8 glyphs. Regular and Bold differ in 518 bitmaps. These are source properties, not a claim that a Linux VT can load the whole repertoire at once.

The provenance review matched Bold byte-for-byte with upstream `unscii-8.hex` at commit `257b4dfeb50651510fe8b9cf4f151a148a4490e4`. Regular has the same codepoint set as upstream `unscii-8-thin.hex`, with 239 modified glyphs and no added or removed codepoints. The different counts describe different comparisons. The generator attributes UNSCII/Viznut and the Regular modifications; Alcard does not claim original authorship of every pixel.

Ordinary UNSCII-8/thin has the upstream Public Domain/CC0 declaration. The separate `unscii-16-full`/Unifont material is not the source of these files. See [licensing](licensing.md) for the exact scope and retained notices.

## Rendering formats and ownership

| Representation | Consumer | Installed/runtime boundary |
| --- | --- | --- |
| Unicode HEX | Build tools and embedded bitmap reader | Canonical source pixels |
| PSF2 8×8 with Unicode table | Linux VT/fbcon and `setfont` | `alcard-fonts-console`, `/usr/share/kbd/consolefonts/` |
| Pixel-aligned vector TTF | Foot/fontconfig and other fontconfig applications | `alcard-fonts-gui`, `/usr/share/fonts/alcard/` |
| Embedded Regular HEX → cached tiny-skia glyph paths | Shell, locker and polkit-agent through `alcard-ui` | No TTF/fontconfig requirement in the custom renderer |

[The generator](../../tools/build-alcard-fonts.py) creates a 256-glyph `core` and a 512-glyph `extended` PSF2 for each weight. Console packaging checks headers, Unicode maps, required character coverage and pixel equality with the HEX source. ASCII occupies the expected glyph slots, including the blank space used by VT erase/scroll paths; merely having a Unicode mapping is insufficient for those operations.

The optional `--ttf` conversion produces `AlcardPixel-Regular.ttf` and `AlcardPixel-Bold.ttf`, with a fixed advance and 1,024 units per em. GUI packaging checks cmap, coverage and monospaced metrics. Generated PSF/TTF files belong in build outputs and APKs, not in the source directory.

## Character coverage and display limits

The source contains English, all Russian letters including Ё/ё, Spanish accents/ñ/¿/¡, and the required pinyin tone letters. The PSF subsets must retain the mandatory Latin/Russian/Spanish/pinyin repertoire and the TUI drawing characters selected by the generator. An APK check fails when required coverage or bitmap integrity is lost.

The display target is 320×170 pixels: an 8×8 cell gives **40 columns × 21 rows**, leaving two pixel rows. Keep labels, focus and line wrapping valid at this size. Use Unicode display width rather than byte count when measuring strings.

Linux VT loads one PSF font at a time. SGR bold does not reliably switch between the separate Regular and Bold files; they are alternative console fonts. A 512-glyph VT font can reduce the available bright-colour space from 16 to 8 colours. Focus, selection and essential information must therefore work without bold or a particular bright palette.

The custom renderer caches Regular glyph geometry and draws it without antialiasing. Missing glyphs fall back to `?`; complete shaping and font fallback are not implemented. The available 8×8 repertoire and 256/512-glyph console limits do not provide general Chinese/Japanese support. Emoji, Nerd Font symbols and arbitrary double-width characters are not part of the compact UI contract.

VM/Foot tests and font-generation checks do not establish physical readability, full language coverage on the panel, or integer-scale rasterization on every display. These remain device/display acceptance items.

## Physical keyboard and Linux layout data

The 46-key TCA8418 driver and board DT implement physical Fn/Sym/Shift state. The data-only `alcard-keyboard-zero` APK does not implement another keyboard daemon or replace the kernel layer state machine.

The source catalog is [layouts.json](../../keyboard-zero/layouts.json). It names each physical letter key, its Linux keycode/XKB identifier and the manufacturer Sym symbols. [XKB definitions](../../keyboard-zero/symbols/alcard) and [console maps](../../keyboard-zero/console/) are checked against the same catalog by [verify-keyboard.py](../../keyboard-zero/verify-keyboard.py).

Two intentional board-map changes must remain aligned with the data package:

- **Fn+Esc → `KEY_LEFTMETA`**, without delaying ordinary Esc. A lone Super release returns to the existing Shell Home; application input is not a global Shell shortcut layer.
- The vendor Sym placeholder `KEY_ZENKAKUHANKAKU` is replaced with **`KEY_YEN`**, giving the original `>` symbol a standard XKB evdev identifier (`AE13`). The printed symbol remains `>`; the other manufacturer Sym characters are preserved.

The package installs `alcard(us)`, `alcard(ru)`, `alcard(es)` and `alcard(de)` under `/usr/share/X11/xkb/symbols/alcard`, without overwriting system XKB definitions. Console maps are compiled with `loadkeys --unicode --bkeymap`, then gzip, into `/usr/share/alcard/keyboard/console/*.bmap.gz`. Source maps and the catalog are installed as well. Compilation does not apply a map to the live build console.

## Language and layout selection

Hello starts in English with Original US. US stays first in the stored layout set; optional RU, ES and DE affect letters while preserving the manufacturer symbol layer. Selecting a future layout does not change the active password/account-entry layout midway through Hello.

| Layout | Main letter behaviour | Additional letters on AltGr levels |
| --- | --- | --- |
| US | Original letters | None |
| RU | Russian letters on the 26 physical letter positions | `q→ё`, `j→ж`, `h→х`, `p→ъ`, `e→э`, `b→б`, `u→ю` |
| ES | Original Latin letter positions | `n→ñ`, `a→á`, `e→é`, `i→í`, `o→ó`, `u→ú`, `y→ü` |
| DE | Y/Z exchanged | `a→ä`, `o→ö`, `u→ü`, `s→ß` |

Shift provides the defined upper-case levels; the German `s` key is an exception: AltGr gives `ß`, while Shift+AltGr currently gives `S`, not `ẞ`. AltGr characters are currently accessible through an external keyboard's RightAlt. **The built-in Cardputer keyboard has no RightAlt**, so complete access to these extra letters from its own keys remains an explicit input-design task. Fn or Sym must not silently acquire another meaning to hide this gap.

The setup backend records the selected set in `/etc/alcard-keyboards` and creates `/etc/alcard/keyboard-xkb.env`. The XKB group list uses `alcard` with the selected variants; when more than one layout is selected, the generated option is `grp:alt_shift_toggle`. Changes take effect in a newly started graphical session. This defines configured desktop switching; it does not prove every combination on the physical keypad.

Hello configures the console boot map as US through `/etc/keymap/alcard.bmap.gz` and OpenRC `loadkmap`. Ordinary Settings also exposes a TTY-layout action. A Linux VT still has one active console layout rather than XKB groups; do not describe a graphical group change as an automatic simultaneous change of every existing VT. See [Settings](settings.md) for the actual actions.

## Build and acceptance

With the environment from [building](building.md):

```sh
bash build.sh keyboard-check
bash build.sh keyboard
bash build.sh fonts-check
bash build.sh fonts
bash build.sh fonts-gui-check
bash build.sh fonts-gui
```

The console font build needs Python and kbd runtime support; GUI font generation additionally needs `py3-fonttools` in the builder. The keyboard build requires Alpine `kbd` with `loadkeys`. A missing `loadkeys` is a missing build prerequisite, not a reason to generate unverified binary maps.

Acceptance covers three different things:

1. **Source/format:** Unicode records, exact bitmap preservation, PSF2 table/slot structure, TTF cmap/metrics, agreement between console and XKB maps.
2. **Installed software:** the correct packages remain available in the chosen mode; VT/login, Foot and custom UI use their intended font paths and language configuration.
3. **Physical device:** readability, Fn/Sym/Shift/Meta, LEDs, every supported input combination, group switching and absence of missing glyphs on the real panel.

Verify generated files and installed font/layout behavior separately. Physical acceptance remains open; current package inputs and readiness are described in [packaging](packaging.md) and [current status](status.md).
