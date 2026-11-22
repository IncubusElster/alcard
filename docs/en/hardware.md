# Cardputer Zero hardware and BSP

**Language:** English · [Русский](../ru/hardware.md)

> Scope: current source tree · Updated: 2026-11-22

The initial hardware target is **M5Stack Cardputer Zero v5, ARM64, Compute Module 0 / RP3A0 (BCM2837)**. The selected software base is Alpine `v3.24`, `linux-rpi=6.18.60-r0`, ABI `6.18.60-0-rpi`. Driver provenance and package/DT contracts are described below. Physical device acceptance remains open.

A driver existing in Linux source, an enabled Alpine Kconfig option, a successful package build and a working physical device are four different claims. This page identifies the implementation and the remaining checks without treating one as evidence for another.

## Variants and source locks

| Variant | Current support boundary |
| --- | --- |
| Standard Cardputer Zero v5 | Shipped profile `alcard-board-zero-v5`; full-model sensor overlay |
| Cardputer Zero v5 Lite | Shipped profile `alcard-board-zero-v5-lite`; camera and BMI270/BMM150 are absent in the documented Lite configuration |
| M5Stack v3 | Upstream has a different base DTS; Alcard has no shipping v3 APK/profile |

The two shipped profiles are mutually exclusive. Revision labels v3/v5 describe board/DT revisions, not Linux versions. Confirm the actual SKU, board marking and probe results before enabling optional nodes: public product descriptions and production batches may differ. There is no automatic board-revision detector.

The fixed upstream input is [M5Stack overlays/modules at `4f7aeee9`](https://github.com/m5stack/m5stack-linux-dtoverlays/tree/4f7aeee9fc103ebb03d22cac476bb191ecfb321e). The original image-build reference is [CardputerZero pi-gen at `791a985d`](https://github.com/CardputerZero/pi-gen/tree/791a985d4efca7327ae1fdb6c48230ac0d1ee1bc/stage2/05-cardputerzero); product wiring is described in [M5Stack's hardware documentation](https://docs.m5stack.com/en/CardputerZero). These sources guide integration; their Debian binaries/services are not copied into Alpine.

## Complete device/provider matrix

| Device or function | Selected implementation | Device acceptance |
| --- | --- | --- |
| CM0 CPU, clocks, interrupts, ARM64/SMP | Alpine Raspberry Pi kernel and matching base DTB | Correct model, architecture, kernel release and four CPUs |
| microSD/MMC and ext4 ROOT | Standard MMC/SDHCI and ext4 | Cold boot, writable persistent root and reboot |
| Raspberry Pi boot chain | Alpine boot firmware, kernel and mkinitfs | Exact CM0 firmware/file search and partition behaviour |
| ST7789v3 320×170 SPI LCD | Upstream `panel-mipi-dbi`, Alcard DT and adapted MIPI DBI firmware | Visible VT/login, correct orientation/offset/colour and blank/unblank |
| LCD PWM/backlight | M5Stack `pwm_bl_m5stack` and M5IOE1 | Backlight class device, stable brightness and safe power state |
| HDMI image/audio | VC4 DRM/KMS and standard ALSA support | HDMI attachment does not unexpectedly steal the internal console |
| H.264/media acceleration | Raspberry Pi media/codec interfaces where enabled | Actual kernel/firmware API; not a basic BSP blocker |
| TCA8418 46-key matrix | M5Stack `tca8418_keypad_m5stack` with Fn/Sym state machine | Built-in text input, modifiers, Meta and repeat |
| Keyboard LEDs | Keyboard state plus M5IOE1 GPIO | LED state follows the actual key state |
| M5IOE1 GPIO/PWM | M5Stack `m5ioe1` | GPIO/PWM enumeration, shared reset and safe power defaults |
| Extra SPI chip select | M5Stack `gpio-forwarder` | External SPI does not conflict with LCD CS0 |
| BQ27220 battery gauge | M5Stack BQ27xxx additions under one coordinated provider | Voltage, capacity and charging state through power_supply |
| Physical power-switch signal | M5Stack `powerfail_suo` source studied; module omitted and DT node disabled | Graceful sync/readonly/power-cut and bounce behaviour before activation |
| RX8130CE RTC | Standard `rtc-ds1307` family | RTC device and valid clock read/write |
| IR RX/TX | Standard gpio-ir/rc-core | Receive events and verified transmit waveform/interface |
| ES8389 codec, `everest,es8390` compatible | M5Stack `es8389_m5stack` | ALSA sound card, clocks, playback and capture |
| Cardputer audio routing/headphone detect | M5Stack `cardputerzero-audio` | Speaker/headphone transitions and microphone route |
| AW8737A speaker amplifier | Standard `snd-soc-simple-amplifier` when enabled; conditional vendor fallback | GPIO/DAPM power and clean speaker output |
| MEMS mic and CTIA/OMTP switching | Codec/ASoC and board routing | Capture plus correct headset microphone selection |
| BMI270 IMU | Stock core/I²C if compatible and enabled, otherwise coherent M5Stack family; separate firmware | IIO acceleration/gyro/temperature without bus errors |
| BMM150 through BMI270 AUX | M5Stack `bmc150_magn_bmi270` bridge with compatible BMC150 core | Magnetometer IIO axes; not assumed to be an independent SoC I²C child |
| IMX219 CSI camera | Standard IMX219/Raspberry Pi CSI/media support | Correct sensor power, media graph and capture on equipped boards |
| CYW43439 Wi-Fi | `brcmfmac`, platform firmware/NVRAM/regulatory data | Enumeration, scans, stable traffic and recovery |
| CYW43439 Bluetooth | `hci_uart`, `btbcm`, firmware and BlueZ | UART attachment, controller and real pairing |
| SR9900A USB Ethernet, `0fe6:9900` | Raspberry Pi `r8152` route identified in source | Confirm ID and config in the exact Alpine kernel; test ZeroCD/modeswitch if needed |
| GL852G USB hub | Standard USB hub stack with M5IOE1 reset/power | Stable topology and reset behaviour |
| USB-A/C/HAT and OTG | USB core, DWC2 and optional gadget configuration | Physical Host/Slave routing; no assumed simultaneous host/gadget |
| Grove I²C/UART and 5V | Standard buses, GPIO mux and M5IOE1 | Mode selected before use, correct power/current limit |
| EXT 2×7 SPI/I²C/UART/GPIO/USB | Standard buses plus board mux/forwarder | Pin ownership and concurrent use without panel conflict |
| LoRa/NFC/CC1101 and other external modules | Optional device-specific integration over standard buses | Separate module acceptance, not a base-image promise |

Wi-Fi/Ethernet ownership in userland is now NetworkManager; BlueZ manages Bluetooth. This does not replace the kernel, firmware and wiring checks above. See [Settings](settings.md).

## Pins, addresses and shared resources

Physical M5IOE1 `PYG*` labels are not automatically Linux gpiochip indexes. Verify the driver's mapping and DTS rather than substituting a physical suffix as a GPIO number.

| Resource | Wiring / address | Constraint |
| --- | --- | --- |
| LCD SPI0 | G10 MOSI, G11 SCLK, G8 CS0, G25 D/C; G5 TE | One active device on SPI0 CS0; TE remains optional |
| LCD reset/backlight | M5IOE1 `PYG12_LCD_RST` / `PYG10_BL_PWM` | Reset polarity/mapping must be confirmed; no invented reset binding |
| Keyboard | I²C1 `0x34`, G27 IRQ | Reset shares the expander's hub/keyboard line |
| M5IOE1 | I²C1 `0x4f`, G16 reset | Controls several independent peripherals |
| BQ27220 | I²C1 `0x55` | Capacity/NTC parameters, including nominal 1750 mAh, need physical confirmation |
| RX8130 RTC | I²C1 `0x32` | Exactly one active node |
| Audio codec | I²C1 `0x10`; I²S G18 BCLK, G19 LRCK, G20/G21 data | Verify data direction and 12.288 MHz clock/routing against the board |
| Speaker/headphone detect | G24 enable / G17 detect | AW8737A is not an AW882xx device |
| BMI270/BMM150 | SoC I²C1 `0x68` / AUX address `0x10` behind BMI270 | The magnetometer address does not conflict with the codec on another bus |
| IR | G12 TX, G13 RX | Avoid competing alternate functions on the same pins |
| Power detect | G6 | No unreviewed force-off on boot or contact bounce |
| Grove mux | G4: LOW UART, HIGH I²C1 | Initial UART selection; coordinate shared I²C/UART resources |
| Grove 5V | M5IOE1 `PYG4_GROVE_EN` | Enable deliberately; documented limit below 500 mA |
| HAT P0/P1 mux | M5IOE1 `PYG1_HAT_SW` | GPIO G26/G23 by default; USB shares the hub path |
| HAT 5V | M5IOE1 `PYG13_HAT_EN` | Off by default; verify safe polarity |
| CTIA/OMTP | M5IOE1 `PYG6_MIC_SW` | LOW CTIA, HIGH OMTP according to the hardware reference |
| Keyboard/hub reset | M5IOE1 `PYG3_KB_HUB_RST` | Resetting USB can also affect input |
| Keyboard LEDs | M5IOE1 `PYG8/9/11` | Shift/Fn/Sym indicators tied to driver state |
| External SPI | G11 CLK, G10 MOSI, G9 MISO, G7 CS1, G22 extra CS | LCD already owns CS0 |
| External I²C/UART | G2/G3 SDA/SCL, G14/G15 UART | Shared with board/Grove configuration |
| HAT USB | GL852G through P0/P1 | Requires Host mode and matching mux state |

Slave mode changes physical USB routing to USB-A, USB-C1, Ethernet and HAT. A future UI must describe unavailable ports rather than assuming a software toggle can overcome the switch. Do not enable every optional camera/mic/speaker GPIO overlay simultaneously: an active DTS may already own those resources.

## External module selection and ABI

[The kmods recipe](../../packaging/apk/alcard-kmods-rpi/APKBUILD) and [its builder](../../scripts/build-kmods-rpi.sh) evaluate the **actual** matching `linux-rpi-dev` configuration and `Module.symvers`:

| Function | Selection rule |
| --- | --- |
| M5IOE1, GPIO-forwarder, TCA8418, PWM backlight, ES8389, audio machine | Compile the pinned M5Stack implementations |
| Simple amplifier | Prefer stock `CONFIG_SND_SOC_SIMPLE_AMPLIFIER=y/m`; compile vendor fallback only when absent |
| BQ27220 | Compile vendor BQ27xxx core/I²C only when the stock family is disabled; stop if stock is enabled, pending a coordinated backport/replacement |
| BMI270 | Use stock core and I²C when both are present; compile the vendor pair when both are absent; reject an inconsistent partial configuration |
| BMM150 AUX | Compile the M5Stack bridge. If `CONFIG_BMC150_MAGN=y/m`, use the stock core and stop if any required symbol is not exported. Compile the vendor core only when that stock option is disabled |

The common Kbuild/MODPOST pass resolves cross-module symbols. Do not suppress undefined symbols or install duplicate stock/vendor modules. The BMM150 bridge patch reads `bosch,aux-i2c-addr`, matching the Alcard platform-node DTS; the original vendor `reg` lookup was unsuitable for that root-level platform device. See [the tracked patch](../../patches/m5stack/0001-bmi270-aux-address-property.patch).

Before changing BQ27220 capacity, NTC or initialization parameters in DT, review the exact vendor driver path for writes to the gauge's nonvolatile memory (NVM). The nominal battery values are not permission to reprogram the gauge. Persistent writes, initialization order and power-loss behaviour have no hardware acceptance yet. The gauge interface also does not establish that a separate Linux-controlled charger chip exists.

The current kernel/package contract selects 12 external `.ko`. The image verifier expects that set; a provider/Kconfig change requires updating and revalidating the package/image contract, not just changing a table. Lite still installs the common module bundle; it omits absent-device **active DT nodes**, rather than having a separate stripped module APK.

Do not add unrelated vendor `aw882xx`, `tpa6130`, `lsm6ds3tr` or CM4 modules merely because they exist upstream. The standard RTC, IR, USB/Ethernet, Wi-Fi/Bluetooth and DRM drivers remain Alpine-owned. `powerfail_suo`, alternate ST7789 drivers and the vendor `panel-mipi-dbi-m` implementation are not additional default providers.

The separate BMI270 initialization blob is 8,192 bytes, SHA-256 `2d75e68e343a13ff99be98261dfbd99d9e8c6f267da9e3c5aeb883276ad178db`. Its pinned M5Stack source is `modules/imu-1.0/bmi270-init-data.fw`; provenance review matched it to Bosch's `bmi270_config_file[]`. The firmware APK installs it at `/usr/lib/firmware/bmi270-init-data.fw` with the Bosch BSD-3-Clause notice. This exact-byte provenance does not establish permission or compatibility for arbitrary other vendor blobs; see [licensing](licensing.md).

## Internal LCD: the selected DRM/KMS path

Linux VT reaches the panel through fbcon → DRM fbdev emulation → the upstream `panel-mipi-dbi` DRM driver. labwc uses the DRM/KMS device directly. There is no custom framebuffer terminal daemon and no new Alcard DRM driver.

Required kernel capabilities include `CONFIG_DRM`, `CONFIG_DRM_KMS_HELPER`, `CONFIG_DRM_PANEL_MIPI_DBI`, `CONFIG_DRM_MIPI_DBI`, `CONFIG_SPI`, `CONFIG_SPI_BCM2835`, backlight/PWM support, `CONFIG_VT`, `CONFIG_VT_CONSOLE`, `CONFIG_DRM_FBDEV_EMULATION`, `CONFIG_FRAMEBUFFER_CONSOLE` and firmware loading. Verify whether panel MIPI DBI is built in or a module, and that DRM fbdev/fbcon are enabled; include any required modules and firmware in the initramfs. Check the exact new `.config` again for each kernel release; upstream defconfig does not establish Alpine binary settings.

The integrated [v5 DTS](../../board/cardputer-zero/device-tree/alcard-zero-v5.dts) replaces the legacy display node in source. It defines one upstream `panel-mipi-dbi-spi` device on SPI0 CS0, 40 MHz SPI, G25 D/C, M5IOE1-backed backlight and a 35-pixel back porch. The first compatible is `cardputerzero,st7789v`; adding that compatible to the schema is still needed for strict `dtbs_check`.

Panel reset is intentionally not added to the DTS before its expander mapping and polarity are verified. TE is also not enabled speculatively. DT references and deferred probe must establish expander/backlight dependencies; arbitrary service sleeps do not solve the dependency graph. Verify the merged DT contains neither a second active CS0 display nor a conflicting spidev node.

The panel initialization is **adapted from the pinned M5Stack implementation**, recorded in [st7789v3.toml](../../board/cardputer-zero/firmware/st7789v3.toml), and converted by [mipi-dbi-firmware.py](../../tools/mipi-dbi-firmware.py) to the Linux MIPI DBI v1 format. The installed name is `/usr/lib/firmware/cardputerzero,st7789v.bin`.

This file is a command stream, not executable device code: 15 magic bytes, one version byte, then command records. A delay uses the NOP record `(00,01,ms)`. The source reference is M5Stack's `st7789v_m5stack.c`, `HSD20_IPS=1`, `init_display()`, `set_var(90°)` and `HSD20_IPS_GAMMA`. Firmware byte changes require a package revision/version change, regenerated hashes, and validation against the Linux parser.

The current specification produces 93 bytes and 16 steps, SHA-256 `cb8a403f7f7648622b386f312a5dd3e42d706c3419e1fd5216a36289d399fbe2`. It retains the source sleep-out delay (120 ms), RGB565 pixel format (`0x55`), landscape `MADCTL=0xA0`, porch/voltage settings, both gamma curves, inversion and display-on sequence. This verifies structure and reproducibility, not the real glass: orientation, RGB/BGR, 35-pixel offset, row wrap, timings and blank/unblank must still be tested.

Firmware must exist when the driver probes. For an early initramfs console, include the matching firmware/modules in initramfs; the first hardware target is the normal OpenRC/tty1 stage, so do not claim early output merely because firmware is present on ext4. Current `config.txt` is generated by the image builder, not modified by the boot APK.

Do not hardcode `/dev/dri/card0` or `/dev/fb0`: SPI LCD and VC4/HDMI can be separate DRM devices. Confirm fbcon selection and HDMI hotplug on hardware. Keep `console=tty1`; disabling DRM fbdev emulation would remove the intended console path. The single tty1 dispatcher selects Hello and then getty or greetd according to the installed mode.

## Current board configurations

[Standard v5](../../board/cardputer-zero/boot/alcard.cfg) enables the integrated board, battery and BMI270/BMM150 overlays, IR RX/TX and DWC2. IMX219 and optional VC4/HDMI overlay lines are commented; they are conditional capabilities, not automatic camera/HDMI acceptance. [Lite](../../board/cardputer-zero/boot/alcard-lite.cfg) omits the absent sensor/camera configuration. A separate BMI270-only DTBO exists for an explicitly verified sensor variant, but does not implement automatic SKU detection.

Only the chosen board-profile owns `/boot/alcard.cfg`. It contains device settings; root PARTUUID and `console=tty1` belong in `cmdline.txt`. Packages and boot staging are described in [packaging](packaging.md) and [boot and updates](boot-updates.md).

## Physical acceptance checklist

For each actual board/profile, record the source/package versions and verify:

1. Cold boot, selected CM0 base DTB, kernel ABI, persistent ext4 and normal reboot.
2. Internal DRM/fbcon output, readable Hello/login, panel geometry/colour, backlight cycle and HDMI coexistence.
3. Built-in keyboard, Fn/Sym/Shift/Meta, LEDs and shared hub-reset effects.
4. Battery readings/charging, RTC, power-off and the physical switch without filesystem corruption.
5. Audio playback/capture, headphone routing, amplifier and headset microphone mode.
6. Equipped sensors and camera with correct IIO/media topology; absent Lite devices do not repeatedly probe.
7. Wi-Fi/Bluetooth/Ethernet, USB Host/Slave and hub recovery.
8. IR and external ports with correct mux, power limits and conflict-free ownership.

For each Alpine-owned driver, inspect the exact target Kconfig and, when built as a module, its `.ko`, `modinfo` and `modules.alias`. Match DT `compatible` values to `MODULE_DEVICE_TABLE`/module aliases; a source driver name alone does not prove automatic binding. For SR9900A, check `USB_DEVICE(0x0fe6,0x9900)` and `CONFIG_USB_RTL8152` in the exact Alpine sources/configuration before accepting the Ethernet route.

Review `dmesg` for firmware requests, unresolved dependencies, I²C/SPI errors, repeated deferred probes and driver conflicts. This checklist is not marked passed by a source audit, QEMU session or isolated package installation. Verification boundaries are in [current status](status.md); remaining work is in [roadmap](roadmap.md).
