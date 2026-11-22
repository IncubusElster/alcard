#!/usr/bin/env bash
# Capture the real Shell at 320x170 in a separate headless labwc session.
# Invoke with: dbus-run-session -- bash scripts/capture-shell.sh
# PNGs are review artifacts, not generated production assets or hardware proof.
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
BIN="${ALCARD_SHELL_BIN:-$ROOT/rust/apps/alcard-shell/target/debug/alcard-shell}"
POINTER_BIN="${ALCARD_POINTER_BIN:-$(dirname -- "$BIN")/examples/park-pointer}"
OUT="${ALCARD_SCREENSHOT_DIR:-$ROOT/.alcard-local/screenshots}"
CAPTURE_LOCALE="${ALCARD_SCREENSHOT_LOCALE:-ru}"
CAPTURE_THEME="${ALCARD_SCREENSHOT_THEME:-classic}"
case "$CAPTURE_LOCALE" in
  ru|en|es) ;;
  *) echo 'ALCARD_SCREENSHOT_LOCALE must be ru, en or es' >&2; exit 1 ;;
esac
case "$CAPTURE_THEME" in
  classic|emerald) ;;
  *) echo 'ALCARD_SCREENSHOT_THEME must be classic or emerald' >&2; exit 1 ;;
esac
[[ -x "$BIN" ]] || { echo 'Build alcard-shell before capture' >&2; exit 1; }
[[ -n "${DBUS_SESSION_BUS_ADDRESS:-}" ]] ||
  { echo 'Run capture-shell.sh through a fresh dbus-run-session' >&2; exit 1; }
for tool in labwc foot grim wtype wlr-randr notify-send gdbus python3 timeout stdbuf; do
  command -v "$tool" >/dev/null || { echo "Missing capture tool: $tool" >&2; exit 1; }
done
python3 -c 'from PIL import Image; import tomllib'

bus_call() {
  gdbus call --session --dest org.freedesktop.DBus \
    --object-path /org/freedesktop/DBus \
    --method "org.freedesktop.DBus.$1" "${@:2}"
}
# Do not replace the notification server of an existing desktop session.
if [[ "$(bus_call NameHasOwner org.freedesktop.Notifications)" == *true* ]]; then
  echo 'Refusing a bus with an existing notification server; use dbus-run-session' >&2
  exit 1
fi

mkdir -p -- "$OUT"
OUT="$(cd -- "$OUT" && pwd -P)"
TMP="$(mktemp -d)"
shell_pid='' compositor_pid='' fixture_pid=''
client_pids=()
cleanup() {
  local result=$?
  trap - EXIT INT TERM
  if [[ $result -ne 0 ]]; then
    echo 'Shell capture failed; compositor and Shell logs follow.' >&2
    tail -80 "$OUT/labwc.log" >&2 2>/dev/null || true
    tail -120 "$OUT/shell.log" >&2 2>/dev/null || true
    tail -80 "$OUT/fixture-terminal.log" >&2 2>/dev/null || true
  fi
  for pid in "${client_pids[@]}" "$fixture_pid" "$shell_pid" "$compositor_pid"; do
    [[ -z "$pid" ]] || kill "$pid" 2>/dev/null || true
  done
  wait 2>/dev/null || true
  rm -rf -- "$TMP"
  exit "$result"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

export XDG_RUNTIME_DIR="$TMP/runtime"
export XDG_CONFIG_HOME="$TMP/config-root"
export XDG_CONFIG_DIRS="$TMP/config-dirs"
export XDG_DATA_HOME="$TMP/data"
export XDG_DATA_DIRS="$TMP/data-dirs"
mkdir -m 700 "$XDG_RUNTIME_DIR"
mkdir -p "$TMP/labwc" "$XDG_CONFIG_HOME/alcard" "$XDG_CONFIG_DIRS" \
  "$XDG_DATA_HOME/applications" "$XDG_DATA_DIRS"
# Ordinary .desktop fixtures provide a reproducible, fully populated Menu.
# They are explicitly labelled in the manifest and never modify host apps.
write_desktop() {
  local id=$1 name=$2 ru=$3 es=$4 icon=$5
  cat >"$XDG_DATA_HOME/applications/alcard-capture-$id.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=$name
Name[ru]=$ru
Name[es]=$es
Exec=/bin/cat
Terminal=true
Icon=$icon
DESKTOP
}
write_desktop terminal Terminal Терминал Terminal utilities-terminal
write_desktop files Files Файлы Archivos system-file-manager
write_desktop settings Settings Настройки Ajustes preferences-system
write_desktop web Web Браузер Web web-browser
write_desktop music Music Музыка Música multimedia-player
write_desktop games Games Игры Juegos applications-games
write_desktop calendar Calendar Календарь Calendario x-office-calendar
write_desktop notes Notes Заметки Notas accessories-text-editor
# A ninth entry deliberately exercises pagination and mixed-script search.
# Its visible name cannot accidentally match the desktop-ID-only query.
write_desktop search 'Z Поиск año 4 5 6 7 8' 'Я Поиск año 4 5 6 7 8' \
  'Z Поиск año 4 5 6 7 8' accessories-text-editor
export WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_HEADLESS_OUTPUTS=1
export WLR_LIBINPUT_NO_DEVICES=1 WLR_XWAYLAND=
unset WAYLAND_DISPLAY DISPLAY

# labwc-config(5): identifier matches Wayland app_id; serverDecoration=no
# overrides SSD on first map. Ubuntu 24.04 labwc 0.7.1 provides
# ToggleFullscreen, not the newer SetFullscreen. Shell starts windowed, so
# the one-time onFirstMap action enters fullscreen without later toggles.
# Do not set skipTaskbar: it removes the foreign-toplevel handle used by Home.
# These rules belong only to this isolated screenshot session.
# https://github.com/labwc/labwc/blob/0.7.1/docs/labwc-config.5.scd
# https://github.com/labwc/labwc/blob/0.7.1/docs/labwc-actions.5.scd
cat >"$TMP/labwc/rc.xml" <<'XML'
<?xml version="1.0" encoding="UTF-8"?>
<labwc_config>
  <windowRules>
    <windowRule identifier="alcard-shell" serverDecoration="no" event="onFirstMap">
      <action name="ToggleFullscreen" />
    </windowRule>
  </windowRules>
</labwc_config>
XML
# Explicit no-op hooks prevent inherited desktop autostart/shutdown scripts.
printf '#!/bin/sh\nexit 0\n' >"$TMP/labwc/autostart"
printf '#!/bin/sh\nexit 0\n' >"$TMP/labwc/shutdown"
chmod 700 "$TMP/labwc/autostart" "$TMP/labwc/shutdown"
labwc -C "$TMP/labwc" >"$OUT/labwc.log" 2>&1 & compositor_pid=$!
for _ in $(seq 1 100); do
  for socket in "$XDG_RUNTIME_DIR"/wayland-*; do
    if [[ -S "$socket" ]]; then
      export WAYLAND_DISPLAY="$(basename -- "$socket")"
      break 2
    fi
  done
  kill -0 "$compositor_pid"
  sleep 0.1
done
[[ -n "${WAYLAND_DISPLAY:-}" ]] || { echo 'No headless Wayland socket' >&2; exit 1; }
output="$(wlr-randr | python3 -c 'import sys; print(next(line.split()[0] for line in sys.stdin if line and not line[0].isspace()))')"
[[ "$output" == HEADLESS-* ]] || { echo "Unexpected output: $output" >&2; exit 1; }
wlr-randr --output "$output" --custom-mode 320x170@60Hz --scale 1 --pos 0,0
wlr-randr >"$OUT/output.txt"

WAYLAND_DEBUG=1 "$BIN" --locale "$CAPTURE_LOCALE" --theme "$CAPTURE_THEME" \
  >"$OUT/shell.log" 2>&1 & shell_pid=$!
ready=false
for _ in $(seq 1 100); do
  kill -0 "$shell_pid"
  if [[ "$(bus_call NameHasOwner org.freedesktop.Notifications)" == *true* ]]; then
    ready=true
    break
  fi
  sleep 0.1
done
[[ "$ready" == true ]] || { echo 'Shell notification server did not become ready' >&2; exit 1; }
[[ -x "$POINTER_BIN" ]] || { echo 'Build the park-pointer example before capture' >&2; exit 1; }
ALCARD_POINTER_FIXTURE=1 timeout 5 "$POINTER_BIN"
gdbus call --session --dest org.freedesktop.Notifications \
  --object-path /org/freedesktop/Notifications \
  --method org.freedesktop.Notifications.GetCapabilities >"$OUT/capabilities.txt"
sleep 0.5

cat >"$OUT/manifest.txt" <<'TEXT'
Alcard Shell: actual headless Wayland screenshots
Logical output: 320x170, scale 1, Pixman.
The process is the production alcard-shell binary, not a renderer demo.
The private labwc config requests fullscreen and removes window decorations.
Menu entries are isolated local .desktop fixtures; the one Recent window is real Foot.
Theme and UI locale are selected explicitly by CLI, without any keyboard theme shortcut.
The reload check writes only the temporary XDG_CONFIG_HOME/alcard/shell.toml.
Hardware indicators read this execution host's real sysfs/proc/getifaddrs data.
No battery, Wi-Fi, Ethernet or Internet status has been injected or fabricated.
Absent/unknown hardware in these images is not Cardputer hardware acceptance.
Notifications below are explicit test messages from the sender "Alcard check".
Keyboard events use wtype's explicit key commands with verified evdev codes.
The native virtual-pointer fixture parks the cursor at the upper-right status edge; screenshots are not retouched.
No production key handling or system session configuration is modified.
Search uses a ninth explicit UTF-8 .desktop fixture, not a real installed app.
Plain wtype text is sent only after the real search editor is visible.
TEXT
printf 'Shell locale: %s; theme: %s.\n' "$CAPTURE_LOCALE" "$CAPTURE_THEME" >>"$OUT/manifest.txt"
python3 - "$BIN" "$OUT/manifest.txt" <<'PY'
import datetime, hashlib, pathlib, sys
binary, manifest = map(pathlib.Path, sys.argv[1:])
with manifest.open('a', encoding='utf-8') as output:
    output.write('Captured UTC: ' + datetime.datetime.now(datetime.timezone.utc).isoformat() + '\n')
    output.write('Binary SHA256: ' + hashlib.sha256(binary.read_bytes()).hexdigest() + '\n\n')
PY

capture() {
  local name=$1
  kill -0 "$shell_pid"
  sleep 0.2
  grim -o "$output" "$OUT/$name.png"
  python3 - "$OUT/$name.png" "$OUT/manifest.txt" <<'PY'
import hashlib, pathlib, struct, sys
path, manifest = map(pathlib.Path, sys.argv[1:])
data = path.read_bytes()
assert data[:8] == b'\x89PNG\r\n\x1a\n', f'Not a PNG: {path}'
size = struct.unpack('>II', data[16:24])
assert size == (320, 170), f'Wrong screenshot dimensions: {path.name}: {size}'
with manifest.open('a', encoding='utf-8') as output:
    output.write(f'{path.name}: 320x170; SHA256 {hashlib.sha256(data).hexdigest()}\n')
PY
}

physical_key() {
  local raw=$1 symbol=$2 placeholder slot
  local -a args=(-s 100)
  # wtype assigns explicit key commands consecutive raw codes starting at 1.
  # Reserve the preceding codes using release-only private-use keysyms; no
  # placeholder press is sent. Plain `wtype 8` would NOT mean physical KEY_8.
  # Source: https://github.com/atx/wtype/blob/master/main.c
  for ((slot=1; slot<raw; slot++)); do
    printf -v placeholder 'U%04X' "$((0xE000 + slot))"
    args+=(-p "$placeholder")
  done
  args+=(-s 100 -P "$symbol" -s 30 -p "$symbol" -s 100)
  WAYLAND_DEBUG=1 wtype "${args[@]}" >"$TMP/wtype.out" 2>"$TMP/wtype.log"
  python3 - "$TMP/wtype.log" "$raw" <<'PY'
import pathlib, re, sys
log = pathlib.Path(sys.argv[1]).read_text()
raw = int(sys.argv[2])
pattern = r'zwp_virtual_keyboard_v1[^\n]*\.key\([^,]*,\s*' + str(raw) + r',\s*1\)'
assert re.search(pattern, log), f'wtype did not send expected physical keycode {raw}'
PY
  cat "$TMP/wtype.log" >>"$OUT/keyboard.log"
  sleep 0.15
}

different() {
  if cmp -s "$OUT/$1.png" "$OUT/$2.png"; then
    echo "UI did not change between $1 and $2" >&2
    exit 1
  fi
}

# Assert text from the actual 8x8 raster. No renderer-only test mode, OCR,
# or rewritten screenshots: grim output is retained as the review evidence.
cat >"$TMP/observe-text.py" <<'PY'
import pathlib, sys, tomllib
from PIL import Image

root, theme, mode = pathlib.Path(sys.argv[1]), sys.argv[2], sys.argv[3]
palette = tomllib.loads((root / f'rust/crates/alcard-settings-core/themes/{theme}.toml').read_text())['palette']
colors = {tuple(bytes.fromhex(palette[key][1:])) for key in ('text', 'muted', 'accent', 'disabled')}
glyphs = {}
for line in (root / 'fonts/alcard-pixel/source/AlcardPixel-Regular.hex').read_text().splitlines():
    if ':' in line:
        code, bits = line.split(':', 1)
        if len(bits) == 16:
            glyphs[chr(int(code, 16))] = bytes.fromhex(bits)

def positions(path, text):
    image = Image.open(path).convert('RGB')
    assert image.size == (320, 170), image.size
    pixels = bytes(pixel in colors for pixel in image.getdata())
    rows = [pixels[y * 320:(y + 1) * 320] for y in range(170)]
    template = [bytes(bool(glyphs[ch][y] & (0x80 >> x)) for ch in text for x in range(8)) for y in range(8)]
    anchor = max(range(8), key=lambda row: sum(template[row]))
    assert sum(template[anchor]), 'Text assertion must contain a visible glyph'
    result = []
    for y in range(163):
        start = 0
        while (x := rows[y + anchor].find(template[anchor], start)) != -1:
            if all(rows[y + line][x:x + len(template[line])] == template[line] for line in range(8)):
                result.append((x, y))
            start = x + 1
    return result

if mode == 'text':
    raise SystemExit(0 if all(positions(sys.argv[4], text) for text in sys.argv[5:]) else 1)
if mode == 'footer':
    for text in sys.argv[5:]:
        found = positions(sys.argv[4], text)
        assert any(y >= 154 for _, y in found), f'Missing bottom action: {text}'
        assert not any(90 <= y < 148 for _, y in found), f'Duplicated internal action: {text}'
if mode == 'same-content':
    before, after = (Image.open(path).convert('RGB') for path in sys.argv[4:6])
    bounds = (0, 18, 320, 170)  # Exclude only live time/status, retain query, selection and footer.
    assert before.crop(bounds).tobytes() == after.crop(bounds).tobytes(), 'Cancel changed the committed Menu/query/selection'
PY
observe_text() { python3 "$TMP/observe-text.py" "$ROOT" "$CAPTURE_THEME" "$@"; }
screen_has() {
  timeout 5 grim -o "$output" "$TMP/observed.png" || return 1
  observe_text text "$TMP/observed.png" "$@"
}
wait_text() {
  local deadline=$((SECONDS + 5))
  while ((SECONDS < deadline)); do
    kill -0 "$shell_pid"
    kill -0 "$compositor_pid"
    if screen_has "$@"; then return 0; fi
    sleep 0.1
  done
  [[ ! -f "$TMP/observed.png" ]] || cp -- "$TMP/observed.png" "$OUT/failure-last-observed.png"
  printf 'Timed out waiting for visible text: %s\n' "$*" >&2
  return 1
}
case "$CAPTURE_LOCALE" in
  ru)
    search_title='Поиск приложений'; search_prefix='Поиск'; search_fixture='Я Поиск'
    no_matches='Приложения не найдены'; files_label='Файлы'; web_label='Браузер'
    clear_title='Очистить уведомления?'; clear_label='Очист.'; cancel_label='Отмена'; empty_label='Пока пусто'
    quick_title='Быстрые настройки'; brightness_label='Яркость'; volume_label='Громкость'
    ;;
  es)
    search_title='Buscar aplicaciones'; search_prefix='Buscar'; search_fixture='Z Поиск'
    no_matches='No hay coincidencias'; files_label='Archivos'; web_label='Web'
    clear_title='¿Borrar los avisos?'; clear_label='Borrar'; cancel_label='Anular'; empty_label='Sin elementos'
    quick_title='Ajustes rápidos'; brightness_label='Brillo'; volume_label='Volumen'
    ;;
  en)
    search_title='Search apps'; search_prefix='Search'; search_fixture='Z Поиск'
    no_matches='No matching apps'; files_label='Files'; web_label='Web'
    clear_title='Clear notifications?'; clear_label='Clear'; cancel_label='Cancel'; empty_label='Nothing here yet'
    quick_title='Quick settings'; brightness_label='Brightness'; volume_label='Volume'
    ;;
esac
editor_text() {
  wait_text "$search_title"
  WAYLAND_DEBUG=1 timeout 5 wtype -d 10 -- "$1" >"$TMP/wtype.out" 2>"$TMP/wtype.log"
  cat "$TMP/wtype.log" >>"$OUT/keyboard.log"
}
editor_clear() {
  wait_text "$search_title"
  WAYLAND_DEBUG=1 timeout 5 wtype -M ctrl -P u -p u -m ctrl >"$TMP/wtype.out" 2>"$TMP/wtype.log"
  cat "$TMP/wtype.log" >>"$OUT/keyboard.log"
}

capture 01-home
spanish_title='Lectura completa: acentos y texto largo'
spanish_body=$'Mensaje de prueba de Alcard check, no es un aviso del hardware.\n\nEl texto contiene español: conexión, batería, próximo, pingüino, ¿qué ocurrió?\nUna notificación larga debe poder leerse completa, con origen, fecha y desplazamiento.\nEl enlace de red no demuestra acceso a Internet.\nEsta última línea también debe permanecer accesible.'
notify-send --app-name='Alcard check' --urgency=low --expire-time=0 \
  "$spanish_title" "$spanish_body"
ru_title='Полное уведомление: источник, чтение и действия'
ru_body=$'Это проверочное сообщение от Alcard check. Оно не описывает состояние реального устройства.\n\nДлинный текст должен читаться целиком, а не исчезать после первых двух строк. Стрелки прокручивают сообщение, вправо и влево — переходят на страницу.\n\nЗдесь проверяются русский текст, переносы и отдельные абзацы. Открытие подробностей отмечает уведомление прочитанным, но не запускает действие приложения.\n\nДействия выбираются отдельно: «Открыть запись» или «Напомнить позже». Сначала можно прочитать весь текст и вернуться к списку.\n\nEspañol: conexión, batería, pingüino, ¿cuándo?\nПоследняя строка проверочного уведомления.'
# Older libnotify versions do not flush --print-id while --wait is running.
# Line buffering makes the acknowledgement observable before the UI action.
stdbuf -oL notify-send --app-name='Alcard check' --urgency=normal --expire-time=0 \
  --print-id --wait --action='open=Открыть запись' --action='later=Напомнить позже' \
  "$ru_title" "$ru_body" >"$OUT/notification-action.txt" 2>"$OUT/notification-client.log" &
action_pid=$!
client_pids+=("$action_pid")
for _ in $(seq 1 100); do
  kill -0 "$action_pid"
  [[ -s "$OUT/notification-action.txt" ]] && break
  sleep 0.1
done
[[ -s "$OUT/notification-action.txt" ]] || { echo 'Action notification was not acknowledged' >&2; exit 1; }
{
  printf '\nSpanish test title: %s\n%s\n' "$spanish_title" "$spanish_body"
  printf '\nRussian test title: %s\n%s\n\n' "$ru_title" "$ru_body"
} >>"$OUT/manifest.txt"
# Allow the ordinary banner to expire so it cannot obscure list/detail images.
sleep 4.5
capture 02-home-unread

physical_key 9 8                 # Physical KEY_8: Notifications.
capture 03-notifications-list
different 02-home-unread 03-notifications-list
physical_key 5 4                 # Physical KEY_4: read; no application action.
capture 04-notification-detail-top
different 03-notifications-list 04-notification-detail-top
physical_key 106 Right           # One page of detail, using real navigation.
capture 05-notification-detail-scroll
different 04-notification-detail-top 05-notification-detail-scroll
physical_key 6 5                 # Detail -> context options on physical 5.
physical_key 108 Down            # Select the second, non-default action.
capture 06-notification-actions
different 05-notification-detail-scroll 06-notification-actions
physical_key 57 space            # Space = physical 5: select "later" via D-Bus.
for _ in $(seq 1 100); do
  if ! kill -0 "$action_pid" 2>/dev/null; then break; fi
  sleep 0.1
done
if kill -0 "$action_pid" 2>/dev/null; then
  echo 'The notification client did not receive its selected action' >&2
  exit 1
fi
wait "$action_pid"
client_pids=()
python3 - "$OUT/notification-action.txt" "$OUT/manifest.txt" <<'PY'
import pathlib, sys
action, manifest = map(pathlib.Path, sys.argv[1:])
lines = action.read_text().splitlines()
assert 'later' in lines, f'Expected named action later, received {lines!r}'
with manifest.open('a', encoding='utf-8') as output:
    output.write('Verified: selecting the second UI action returned the D-Bus action key later.\n')
PY

timeout 5 "$BIN" --home
capture 07-home-after-reading
physical_key 9 8
physical_key 108 Down            # Retained Russian history, then Spanish notice.
physical_key 5 4
capture 08-notification-detail-spanish
timeout 5 "$BIN" --home

# Keep the existing notification/action evidence, and also capture the full
# application surfaces for each explicit locale/theme invocation.
physical_key 7 6                 # Physical KEY_6: Menu.
capture 10-menu
different 08-notification-detail-spanish 10-menu
timeout 5 "$BIN" --home
WAYLAND_DEBUG= foot --app-id=alcard-capture-terminal --title 'Alcard capture terminal' \
  -- /bin/cat >"$OUT/fixture-terminal.log" 2>&1 & fixture_pid=$!
ready=false
for _ in $(seq 1 100); do
  kill -0 "$fixture_pid"
  kill -0 "$shell_pid"
  if grep -Eq 'zwlr_foreign_toplevel_handle_v1.*app_id.*"alcard-capture-terminal"' "$OUT/shell.log"; then
    ready=true
    break
  fi
  sleep 0.1
done
[[ "$ready" == true ]] || { echo 'Capture Foot window was not discovered' >&2; exit 1; }
timeout 5 "$BIN" --home
physical_key 5 4                 # Physical KEY_4: Recent.
capture 11-recent
different 10-menu 11-recent
physical_key 6 5                 # Physical KEY_5: window options.
capture 12-options
different 11-recent 12-options
physical_key 57 space            # Physical 5 alias: select Close.
capture 13-close-yes-no
different 12-options 13-close-yes-no

# Exercise the real signal source while a confirmation remains open. CLI
# theme/locale overrides stay in force; only the file's accent is changed.
cat >"$XDG_CONFIG_HOME/alcard/shell.toml.new" <<'TOML'
schema_version = 1
accent = "#C7A3FF"
TOML
mv -- "$XDG_CONFIG_HOME/alcard/shell.toml.new" "$XDG_CONFIG_HOME/alcard/shell.toml"
kill -HUP "$shell_pid"
capture 14-close-reloaded
python3 - "$OUT/13-close-yes-no.png" "$OUT/14-close-reloaded.png" <<'PY'
from PIL import Image
import sys
before, after = (Image.open(path).convert('RGB') for path in sys.argv[1:])
accent = (0xC7, 0xA3, 0xFF)
assert accent not in set(before.getdata()), 'Reload accent already present before SIGHUP'
assert accent in set(after.getdata()), 'SIGHUP did not render the requested accent'
PY
cat >"$XDG_CONFIG_HOME/alcard/shell.toml.new" <<'TOML'
schema_version = 1
accent = "invalid-color"
TOML
mv -- "$XDG_CONFIG_HOME/alcard/shell.toml.new" "$XDG_CONFIG_HOME/alcard/shell.toml"
kill -HUP "$shell_pid"
ready=false
for _ in $(seq 1 20); do
  kill -0 "$shell_pid"
  if grep -Fq 'Shell preferences were not applied: accent must be #RRGGBB' "$OUT/shell.log"; then
    ready=true
    break
  fi
  sleep 0.1
done
[[ "$ready" == true ]] || { echo 'Invalid reload was not diagnosed' >&2; exit 1; }
capture 15-close-reload-rejected
python3 - "$OUT/14-close-reloaded.png" "$OUT/15-close-reload-rejected.png" "$OUT/manifest.txt" <<'PY'
from PIL import Image
import pathlib, sys
before, after = (Image.open(path).convert('RGB') for path in sys.argv[1:3])
assert (0xC7, 0xA3, 0xFF) in set(after.getdata()), 'Invalid reload replaced the valid accent'
# The clock and unread diagnostic count can change. Compare the actual
# question/window region and complete bottom softkey row instead.
for name, bounds in [('close dialog', (0, 48, 320, 128)), ('Yes/No footer', (0, 148, 320, 170))]:
    assert before.crop(bounds).tobytes() == after.crop(bounds).tobytes(), f'Invalid reload changed {name}'
with pathlib.Path(sys.argv[3]).open('a', encoding='utf-8') as manifest:
    manifest.write('Verified: real SIGHUP applied #C7A3FF during Close; invalid reload was diagnosed and retained the accent, dialog pixels and Yes/No footer.\n')
PY
kill -0 "$fixture_pid"
# Restore a valid default file before leaving the modal and capture the final
# banner in the originally selected theme, without the temporary accent.
printf 'schema_version = 1\n' >"$XDG_CONFIG_HOME/alcard/shell.toml.new"
mv -- "$XDG_CONFIG_HOME/alcard/shell.toml.new" "$XDG_CONFIG_HOME/alcard/shell.toml"
kill -HUP "$shell_pid"
physical_key 9 8                 # No: leave the real window running.
kill -0 "$fixture_pid"
printf 'Verified: a real Foot window was discovered; Close was cancelled with physical 8.\n' >>"$OUT/manifest.txt"
timeout 5 "$BIN" --home
notify-send --app-name='Alcard check' --urgency=critical --expire-time=0 --wait \
  'Проверка важного уведомления' 'Тест категории critical; это не предупреждение об оборудовании.' \
  >"$OUT/critical-client.txt" 2>"$OUT/critical-client.log" &
client_pids+=("$!")
sleep 0.4
capture 09-critical-banner

# Ordinary UTF-8 input reaches the real focused editor. Physical
# 4..8/Space must insert text here instead of invoking navigation actions.
physical_key 7 6                 # Menu, not search.
physical_key 61 F3               # Focus-local alias of slot 7.
wait_text "$search_title"
editor_text 'поиск año '
physical_key 5 4
physical_key 57 space
physical_key 6 5
physical_key 57 space
physical_key 7 6
physical_key 57 space
physical_key 8 7
physical_key 57 space
physical_key 9 8
wait_text "$search_title" 'поиск año 4 5 6 7 8'
capture 16-search-utf8-input

# Edit the middle of a mixed-script string; Home must move the cursor.
physical_key 102 Home
for _ in 1 2 3 4 5 6; do physical_key 106 Right; done
physical_key 111 Delete
editor_text A
physical_key 106 Right
physical_key 14 BackSpace
editor_text Ñ
physical_key 106 Right
physical_key 14 BackSpace
editor_text O
physical_key 107 End
query='поиск AÑO 4 5 6 7 8'
wait_text "$search_title" "$query"
capture 17-search-middle-edit
physical_key 28 Return
wait_text "$search_prefix: $query (1)" "$search_fixture"
capture 18-search-applied

# Physical 7 opens the editor; Esc must preserve the committed filter and
# selection. Compare Menu content, including the actual highlight and footer.
physical_key 8 7
wait_text "$search_title" "$query"
editor_clear
editor_text 'discard this draft'
wait_text "$search_title" 'discard this draft'
capture 19-search-cancel-draft
physical_key 1 Escape
wait_text "$search_prefix: $query (1)" "$search_fixture"
capture 20-search-cancel-preserved
observe_text same-content "$OUT/18-search-applied.png" "$OUT/20-search-cancel-preserved.png"

physical_key 8 7
editor_clear
physical_key 28 Return
# The previously selected ninth item may remain on page two. Up moves into
# the first page without assuming that the isolated menu has only eight apps.
physical_key 103 Up
wait_text "$files_label" "$web_label"
capture 21-search-empty-query
physical_key 61 F3
editor_clear
editor_text 'no_such_alcard_297'
physical_key 28 Return
wait_text "$no_matches"
capture 22-search-no-results
physical_key 5 4                 # Empty results cannot launch a neighboring app.
wait_text "$no_matches"

physical_key 8 7
editor_clear
editor_text 'ALCARD-CAPTURE-SEARCH'
physical_key 28 Return
wait_text "$search_prefix: ALCARD-CAPTURE-SEARCH (1)" "$search_fixture"
capture 23-search-desktop-id
physical_key 8 7
editor_clear
physical_key 28 Return
printf 'Verified: real editor accepted RU/ES UTF-8, physical digits 4-8 and Space; middle edits, case-insensitive name/desktop-ID filters, Enter, Esc preservation, empty query and empty results passed.\n' >>"$OUT/manifest.txt"

# The current critical test notice has no application actions: Options are
# Delete then Clear. Cancellation must preserve history; Enter clears it.
timeout 5 "$BIN" --home
physical_key 9 8
physical_key 6 5
physical_key 108 Down
physical_key 57 space
wait_text "$clear_title" "$clear_label" "$cancel_label"
capture 24-clear-bottom-actions
observe_text footer "$OUT/24-clear-bottom-actions.png" "$clear_label" "$cancel_label"
physical_key 1 Escape
physical_key 9 8
wait_text 'Проверка важного уведомления'
capture 25-clear-cancelled
physical_key 6 5
physical_key 108 Down
physical_key 57 space
wait_text "$clear_title"
physical_key 28 Return
wait_text "$empty_label"
capture 26-notifications-cleared
printf 'Verified: Clear uses only lower Clear/Cancel; Esc preserved the real critical notice and Enter cleared notification history.\n' >>"$OUT/manifest.txt"

# Current Quick matrix coverage opens the nested list and returns without selecting
# or confirming any system operation. Capabilities describe this CI host;
# mutation cases belong only to check-shell-system.sh's private fake login1.
timeout 5 "$BIN" --home
physical_key 6 5
wait_text "$quick_title" "$brightness_label" "$volume_label" Wi-Fi Bluetooth
capture 27-quick-actions
physical_key 9 8
wait_text ALCARD
printf 'Verified: Home 5 opened Quick settings and 8 returned; no system operation was selected or confirmed. Availability is the execution host observation.\n' >>"$OUT/manifest.txt"

kill -0 "$shell_pid"
if grep -Ei 'protocol error|panicked at|Wayland keyboard initialization|SHM buffer length mismatch' "$OUT/shell.log"; then
  exit 1
fi
printf '\nPASS: actual Shell, 320x170 surfaces, physical softkeys, scroll, named notification action, atomic SIGHUP reload/rejection, real UTF-8 search/editor and lower-only Clear.\n' >>"$OUT/manifest.txt"
printf 'PASS: screenshots and manifest are available in %s\n' "$OUT"
