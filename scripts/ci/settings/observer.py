#!/usr/bin/python3
"""Private installed-Settings acceptance helper; never shipped in an APK.

The root observer prepares disposable guest conditions and reads system state.
Every Settings change under test is made by the packaged, unmodified TUI running
as the provisioned ordinary user. Terminal contents and passwords stay in memory
and are never part of RPC replies, logs, screenshots or exported evidence.
"""
import codecs
import errno
import fcntl
import os
from pathlib import Path
import pty
import pwd
import re
import select
import signal
import struct
import subprocess
import termios
import time


PAGES = {
    'home': ('ALCARD / SETTINGS', 'ALCARD / НАСТРОЙКИ', 'ALCARD / AJUSTES'),
    'system': ('System', 'Система', 'Sistema'),
    'language': ('Language & keyboard', 'Язык и клавиатура', 'Idioma y teclado'),
    'clock': ('Date & time', 'Дата и время', 'Fecha y hora'),
    'network': ('Network', 'Сеть', 'Red'),
    'profiles': ('Saved connections', 'Сохранённые подключения', 'Conexiones guardadas'),
    'profile': ('Connection profile', 'Профиль подключения', 'Perfil de conexión'),
    'device': ('Interface', 'Интерфейс', 'Interfaz'),
    'bluetooth': ('Bluetooth',),
    'power': ('Display & power', 'Экран и питание', 'Pantalla y energía'),
    'sound': ('Sound', 'Звук', 'Sonido'),
    'appearance': ('Appearance', 'Оформление', 'Apariencia'),
}
STATUS = {'saved': ('Done', 'Готово', 'Listo'),
          'unavailable': ('Unavailable', 'Недоступно', 'No disponible'),
          'working': ('Working…', 'Подождите…', 'En curso…')}
CONFIRM = ('Apply this change?', 'Применить изменение?', '¿Aplicar el cambio?')
PICKER = ('↑↓ Select', '↑↓ Выбор', '↑↓ Elegir')
KEYS = {'up': b'\x1b[A', 'down': b'\x1b[B', 'enter': b'\r',
        'back': b'8', 'escape': b'\x1b', 'refresh': b'6',
        'interrupt': b'\x03', 'erase': b'\x7f'}


class Screen:
    """Bounded VT state, sufficient for crossterm's screen/cursor operations.

    This is observation only: unknown escape sequences cannot synthesize a
    successful UI state. Both text storage and sequence buffers are bounded.
    """
    def __init__(self, columns=40, rows=21):
        self.columns, self.rows = columns, rows
        self.decoder = codecs.getincrementaldecoder('utf-8')('replace')
        self.normal = self.blank()
        self.alternative = self.blank()
        self.alternate = False
        self.x = self.y = 0
        self.saved = (0, 0)
        self.escape = ''
        self.osc = False
        self.entered = self.left = 0
        self.revision = 0
        self.native_text = ''

    def blank(self):
        return [[' '] * self.columns for _ in range(self.rows)]

    @property
    def grid(self):
        return self.alternative if self.alternate else self.normal

    def lines(self):
        return [''.join(row).rstrip() for row in self.grid]

    def feed(self, data):
        self.revision += 1
        for char in self.decoder.decode(data):
            if self.osc:
                if char == '\x07' or (self.escape == '\x1b' and char == '\\'):
                    self.osc = False
                    self.escape = ''
                else:
                    self.escape = '\x1b' if char == '\x1b' else ''
                continue
            if self.escape:
                self.escape += char
                if len(self.escape) == 2 and char == ']':
                    self.escape, self.osc = '', True
                elif self.escape.startswith('\x1b['):
                    if len(self.escape) > 2 and '@' <= char <= '~':
                        self.csi(self.escape[2:-1], char)
                        self.escape = ''
                    elif len(self.escape) > 128:
                        self.escape = ''
                elif len(self.escape) >= 2:
                    if char == '7':
                        self.saved = (self.x, self.y)
                    elif char == '8':
                        self.x, self.y = self.saved
                    self.escape = ''
                continue
            if char == '\x1b':
                self.escape = char
            elif char == '\r':
                self.x = 0
                if not self.alternate:
                    self.native_text = (self.native_text + '\n')[-4096:]
            elif char == '\n':
                if not self.alternate:
                    self.native_text = (self.native_text + '\n')[-4096:]
                self.y += 1
                if self.y >= self.rows:
                    self.grid.pop(0)
                    self.grid.append([' '] * self.columns)
                    self.y = self.rows - 1
            elif char == '\b':
                self.x = max(0, self.x - 1)
            elif char == '\t':
                self.x = min(self.columns - 1, (self.x // 8 + 1) * 8)
            elif ord(char) >= 32 and char != '\x7f':
                if not self.alternate:
                    self.native_text = (self.native_text + char)[-4096:]
                if self.x >= self.columns:
                    self.x, self.y = 0, min(self.rows - 1, self.y + 1)
                self.grid[self.y][self.x] = char
                self.x += 1

    def csi(self, arguments, command):
        private = arguments.startswith('?')
        try:
            values = [int(v) if v else 0 for v in arguments.lstrip('?').split(';')]
        except ValueError:
            return
        first = values[0] if values else 0
        count = first or 1
        if private and command in ('h', 'l'):
            if 1049 in values or 1047 in values or 47 in values:
                self.alternate = command == 'h'
                if self.alternate:
                    self.alternative = self.blank()
                    self.entered += 1
                else:
                    self.left += 1
                    # A previous PAM error still visible in terminal history
                    # must not count as rejection of the next credential.
                    self.native_text = ''
                self.x = self.y = 0
        elif command in ('H', 'f'):
            self.y = min(self.rows - 1, max(0, (values[0] or 1) - 1))
            self.x = min(self.columns - 1, max(0, ((values[1] if len(values) > 1 else 1) or 1) - 1))
        elif command == 'G':
            self.x = min(self.columns - 1, count - 1)
        elif command == 'd':
            self.y = min(self.rows - 1, count - 1)
        elif command == 'A':
            self.y = max(0, self.y - count)
        elif command == 'B':
            self.y = min(self.rows - 1, self.y + count)
        elif command == 'C':
            self.x = min(self.columns - 1, self.x + count)
        elif command == 'D':
            self.x = max(0, self.x - count)
        elif command == 'J':
            if first in (2, 3):
                self.grid[:] = self.blank()
            elif first == 0:
                self.grid[self.y][self.x:] = [' '] * (self.columns - self.x)
                for row in range(self.y + 1, self.rows):
                    self.grid[row] = [' '] * self.columns
        elif command == 'K':
            start, end = ((0, self.columns) if first == 2 else
                          (0, self.x + 1) if first == 1 else (self.x, self.columns))
            self.grid[self.y][start:end] = [' '] * (end - start)
        elif command == 'X':
            end = min(self.columns, self.x + count)
            self.grid[self.y][self.x:end] = [' '] * (end - self.x)
        elif command == 's':
            self.saved = (self.x, self.y)
        elif command == 'u':
            self.x, self.y = self.saved


class Acceptance:
    def __init__(self, base):
        self.base = base
        self.pid = self.master = None
        self.screen = None
        self.exit_status = None
        self.original_policy = self.original_root = None
        self.interface = None
        self.uuid = None
        self.service_stopped = False
        self.activation_file = None

    def command(self, args, *, data=None, timeout=20, check=True):
        result = subprocess.run(args, input=data, capture_output=True, timeout=timeout,
                                env={**os.environ, 'LC_ALL': 'C'})
        if check and result.returncode != 0:
            raise RuntimeError('Settings fixture command failed: ' + Path(args[0]).name)
        return result

    def nm(self, *arguments):
        return self.command(['/usr/bin/nmcli', '--wait', '10', *arguments]).stdout.decode().strip()

    def fields(self, *arguments):
        return dict(line.partition(':')[::2] for line in self.nm(
            '-t', '--escape', 'no', *arguments).splitlines() if ':' in line)

    def prepare(self):
        assert Path('/etc/alcard-ci-vm').is_file()
        assert 'alcard_ci_vm=1' in Path('/proc/cmdline').read_text().split()
        account = pwd.getpwnam(self.base.USER)
        assert 1000 <= account.pw_uid < 65534
        assert self.base.session_environment().get('DBUS_SESSION_BUS_ADDRESS')
        self.command(['/sbin/modprobe', 'virtio_net'])
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            devices = [line.split(':', 2) for line in self.nm(
                '-t', '-f', 'DEVICE,TYPE', 'device', 'status').splitlines()]
            ethernet = [entry[0] for entry in devices if len(entry) == 2 and entry[1] == 'ethernet']
            if len(ethernet) == 1:
                self.interface = ethernet[0]
                break
            time.sleep(.2)
        assert self.interface and re.fullmatch(r'[A-Za-z0-9_-]{1,32}', self.interface)
        self.command(['/usr/bin/nmcli', '--wait', '5', 'device', 'disconnect', self.interface], check=False)
        for line in self.nm('-t', '-f', 'UUID,TYPE', 'connection', 'show').splitlines():
            uuid, _, kind = line.partition(':')
            if kind in ('802-3-ethernet', 'ethernet'):
                self.nm('connection', 'delete', 'uuid', uuid)
        self.original_policy = Path('/etc/doas.d/alcard.conf').read_bytes()
        self.original_root = next(line for line in Path('/etc/shadow').read_bytes().splitlines(True)
                                  if line.startswith(b'root:'))
        assert self.original_root.split(b':')[1].startswith((b'!', b'*'))
        return {'interface': self.interface, 'uid': account.pw_uid,
                'profile_count': len(self.profiles()), 'root_locked': True,
                'transport': 'isolated_qemu_usernet_ethernet'}

    def profiles(self):
        return [line.split(':', 1)[0] for line in self.nm(
            '-t', '-f', 'UUID,TYPE', 'connection', 'show').splitlines()
                if line.endswith((':802-3-ethernet', ':ethernet'))]

    def start(self):
        assert self.master is None, 'Settings PTY is already open'
        account = pwd.getpwnam(self.base.USER)
        environment = {**self.base.session_environment(), 'HOME': account.pw_dir,
                       'USER': account.pw_name, 'LOGNAME': account.pw_name,
                       'PATH': '/usr/sbin:/usr/bin:/sbin:/bin', 'TERM': 'xterm-256color',
                       'LANG': 'en_US.UTF-8', 'LC_ALL': 'C.UTF-8'}
        self.screen = Screen()
        self.exit_status = None
        pid, master = pty.fork()
        if pid == 0:
            try:
                os.initgroups(account.pw_name, account.pw_gid)
                os.setgid(account.pw_gid)
                os.setuid(account.pw_uid)
                os.chdir(account.pw_dir)
                fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack('HHHH', 21, 40, 0, 0))
                os.execve('/usr/bin/alcard-settings', ['/usr/bin/alcard-settings'], environment)
            except BaseException:
                os._exit(127)
        self.pid, self.master = pid, master
        os.set_blocking(master, False)
        return self.state()

    def drain(self):
        if self.master is None:
            return
        for _ in range(128):
            if not select.select([self.master], [], [], 0)[0]:
                break
            try:
                chunk = os.read(self.master, 65536)
            except OSError as error:
                if error.errno in (errno.EAGAIN, errno.EIO):
                    break
                raise
            if not chunk:
                break
            self.screen.feed(chunk)
        if self.pid is not None:
            waited, status = os.waitpid(self.pid, os.WNOHANG)
            if waited:
                self.exit_status = os.waitstatus_to_exitcode(status)
                self.pid = None

    def state(self):
        self.drain()
        if self.master is None:
            return {'alive': False, 'open': False}
        flags = termios.tcgetattr(self.master)[3]
        lines = self.screen.lines()
        title = lines[0].strip()
        page = next((key for key, choices in PAGES.items() if title in choices), None)
        footer = lines[-1]
        body = '\n'.join(lines[1:-4])
        status = '\n'.join(lines[-4:-1])
        selected = [i for i, row in enumerate(lines[1:-4]) if row.startswith('› ')]
        dialog = ('choose' if any(value in footer for value in PICKER) else
                  'edit' if '▏' in body else
                  'confirm' if any(value in body for value in CONFIRM) else 'list')
        # A single bounded boolean recognizes the external native prompt. No
        # raw terminal line, entered text or PAM output leaves this process.
        native_text = self.screen.native_text.replace('\n', '').lower()
        prompt = not self.screen.alternate and 'password:' in native_text
        auth_failed = not self.screen.alternate and any(value in native_text for value in (
            'authentication failed', 'authentication failure', 'incorrect password',
            'permission denied', 'sorry'))
        auth_children = []
        if self.pid:
            for entry in Path('/proc').iterdir():
                if not entry.name.isdigit():
                    continue
                try:
                    fields = (entry / 'stat').read_text().rsplit(')', 1)[1].split()
                    exe = str((entry / 'exe').resolve())
                    if int(fields[1]) == self.pid and Path(exe).name in ('doas', 'su', 'busybox'):
                        auth_children.append('doas' if Path(exe).name == 'doas' else 'su')
                except (FileNotFoundError, ProcessLookupError, PermissionError):
                    continue
        return {'alive': self.pid is not None, 'open': True, 'exit_status': self.exit_status,
                'uid': pwd.getpwnam(self.base.USER).pw_uid, 'page': page,
                'locale': ('ru' if title in [v[1] for v in PAGES.values() if len(v) > 1] else
                           'es' if title in [v[2] for v in PAGES.values() if len(v) > 2] else 'en'),
                'dialog': dialog, 'selected': selected[0] - (2 if dialog == 'choose' else 0)
                if len(selected) == 1 else None,
                'status': next((key for key, choices in STATUS.items()
                                if any(v in status for v in choices)), None),
                'alternate': self.screen.alternate, 'auth_prompt': prompt,
                'auth_failed': auth_failed,
                'auth_children': auth_children, 'alternate_entries': self.screen.entered,
                'alternate_leaves': self.screen.left,
                'canonical': bool(flags & termios.ICANON), 'echo': bool(flags & termios.ECHO),
                'signals': bool(flags & termios.ISIG), 'columns': 40, 'rows': 21,
                'revision': self.screen.revision}

    def input(self, request):
        current = self.state()
        assert current['alive'], 'Settings TUI exited before input'
        key = request.get('key')
        if key is not None:
            assert key in KEYS, 'Unknown Settings acceptance key'
            count = request.get('count', 1)
            assert isinstance(count, int) and 1 <= count <= 128
            value = KEYS[key] * count
        else:
            value = request.get('text', '')
            assert isinstance(value, str) and 0 < len(value) <= 128
            assert all(32 <= ord(char) < 127 for char in value)
            if request.get('secret'):
                assert current['auth_prompt'] and not current['echo'], 'Native password prompt is not private'
            else:
                assert current['alternate'] and current['dialog'] == 'edit', 'Text requires a TUI editor'
            value = value.encode()
        revision = self.screen.revision
        assert os.write(self.master, value) == len(value), 'Short private PTY write'
        return {'sent': True, 'before_revision': revision}

    def stop(self):
        self.drain()
        if self.pid:
            os.killpg(self.pid, signal.SIGTERM)
            deadline = time.monotonic() + 3
            while self.pid and time.monotonic() < deadline:
                self.drain()
                time.sleep(.05)
            if self.pid:
                os.killpg(self.pid, signal.SIGKILL)
                os.waitpid(self.pid, 0)
                self.pid = None
        if self.master is not None:
            os.close(self.master)
            self.master = None
        self.screen = None
        return {'stopped': True}

    def clear_auth(self):
        account = pwd.getpwnam(self.base.USER)
        result = self.command(['/bin/setpriv', '--reuid', str(account.pw_uid),
                               '--regid', str(account.pw_gid), '--init-groups',
                               '/usr/bin/doas', '-L'], check=False)
        return {'requested': True, 'cache_clear_status': result.returncode}

    def auth_mode(self, request):
        assert self.original_root is not None and self.original_policy is not None
        mode = request.get('mode')
        assert mode in ('doas', 'su', 'denied')
        self.clear_auth()
        policy = Path('/etc/doas.d/alcard.conf')
        if mode in ('doas', 'denied'):
            shadow = Path('/etc/shadow')
            data = b''.join(self.original_root if line.startswith(b'root:') else line
                            for line in shadow.read_bytes().splitlines(True))
            shadow.write_bytes(data)
        if mode == 'doas':
            policy.write_bytes(self.original_policy)
        else:
            policy.write_text('deny ' + self.base.USER + '\n')
            if mode == 'su':
                password = request.get('password')
                assert isinstance(password, str) and re.fullmatch(r'[a-f0-9]{24}', password)
                self.command(['/usr/sbin/chpasswd'], data=('root:' + password + '\n').encode())
        os.chmod(policy, 0o600)
        return {'mode': mode, 'root_locked': self.readback()['root_locked']}

    def readback(self):
        locale = Path('/etc/profile.d/alcard-locale.sh').read_text().strip()
        root = next(line for line in Path('/etc/shadow').read_bytes().splitlines()
                    if line.startswith(b'root:')).split(b':')[1]
        account = pwd.getpwnam(self.base.USER)
        config = Path(account.pw_dir) / '.config/alcard/shell.toml'
        # This file contains appearance/power preferences only; restrict output
        # to the fixture's known settings instead of returning arbitrary text.
        preferences = {}
        if config.is_file():
            for key in ('theme', 'accent', 'icon_style'):
                match = re.search(r'^' + key + r'\s*=\s*"([a-zA-Z0-9#_-]{1,32})"\s*$',
                                  config.read_text(), re.MULTILINE)
                if match:
                    preferences[key] = match[1]
        shell = [{key: item[key] for key in ('pid', 'start')}
                 for item in self.base.processes()
                 if item['uid'] == account.pw_uid and item['role'] == 'shell']
        result = {'hostname': Path('/etc/hostname').read_text().strip(),
                  'locale': locale, 'root_locked': root.startswith((b'!', b'*')),
                  'preferences': preferences, 'shell': shell}
        if self.interface and not self.service_stopped:
            profiles = self.profiles()
            result['profile_count'] = len(profiles)
            if len(profiles) == 1:
                self.uuid = profiles[0]
                result['profile'] = self.fields('-f',
                    'connection.uuid,connection.id,connection.autoconnect,ipv4.method,'
                    'ipv4.addresses,ipv4.gateway,ipv4.dns,ipv4.ignore-auto-dns',
                    'connection', 'show', 'uuid', self.uuid)
            result['device'] = self.fields('-f', 'GENERAL.STATE,GENERAL.CON-UUID,IP4',
                                           'device', 'show', self.interface)
        return result

    def service(self, request):
        action = request.get('action')
        assert action in ('stop', 'start')
        activation = Path('/usr/share/dbus-1/system-services/org.freedesktop.NetworkManager.service')
        if action == 'stop' and activation.is_file():
            self.activation_file = (activation.read_bytes(), activation.stat().st_mode & 0o777)
            activation.unlink()
        elif action == 'start' and self.activation_file is not None:
            content, mode = self.activation_file
            activation.write_bytes(content)
            activation.chmod(mode)
            self.activation_file = None
        self.service_stopped = action == 'stop'
        self.command(['/usr/bin/busctl', '--system', 'call', 'org.freedesktop.DBus',
                      '/org/freedesktop/DBus', 'org.freedesktop.DBus', 'ReloadConfig'])
        result = self.command(['/sbin/rc-service', 'networkmanager', action], timeout=30)
        return {'service': 'networkmanager', 'action': action, 'status': result.returncode}

    def console_remove(self):
        assert self.master is None, 'Close the Settings terminal before GUI removal'
        account = pwd.getpwnam(self.base.USER)
        assert not any(item['uid'] == account.pw_uid for item in self.base.processes()), (
            'GUI removal is allowed only after the actual session has logged out')
        # This is a disposable package-removal condition, not a second Hello
        # Apply or an assertion that Settings offers session migration.
        targets = ('alcard-desktop-shell', 'alcard-shell', 'alcard-polkit-agent',
                   'alcard-lock', 'alcard-desktop-minimal', 'alcard-fonts-gui',
                   'greetd-agreety', 'greetd')
        installed = [name for name in targets if self.command(
            ['/sbin/apk', 'info', '-e', name], check=False).returncode == 0]
        assert installed, 'No GUI packages were installed before removal'
        self.command(['/sbin/apk', 'del', *installed], timeout=45)
        assert all(self.command(['/sbin/apk', 'info', '-e', name], check=False).returncode != 0
                   for name in targets), 'GUI packages remain after removal'
        # The separate Console VM proves real getty login. Here the same UID,
        # home, saved profile and Settings executable survive GUI removal.
        assert Path('/usr/bin/alcard-settings').is_file()
        assert Path('/usr/sbin/alcard-setupd').is_file()
        assert Path('/usr/bin/nmcli').is_file()
        Path('/etc/alcard/default-session').write_text('console\n')
        return {'removed': installed, 'uid': account.pw_uid,
                'settings_present': True, 'networkmanager_present': True,
                'readback': self.readback()}

    def resume(self, request):
        interface = request.get('interface')
        assert isinstance(interface, str) and re.fullmatch(r'[A-Za-z0-9_-]{1,32}', interface)
        self.interface = interface
        return self.readback()

    def cleanup(self):
        self.stop()
        if self.service_stopped or self.activation_file is not None:
            self.service({'action': 'start'})
        if self.original_root is not None:
            self.auth_mode({'mode': 'doas'})
        return {'cleaned': True}

    def dispatch(self, operation, request):
        handlers = {'prepare': self.prepare, 'start': self.start, 'state': self.state,
                    'stop': self.stop, 'readback': self.readback, 'clear-auth': self.clear_auth,
                    'cleanup': self.cleanup, 'console-remove': self.console_remove}
        if operation in handlers:
            return handlers[operation]()
        argument_handlers = {'input': self.input, 'auth-mode': self.auth_mode,
                             'service': self.service, 'resume': self.resume}
        if operation in argument_handlers:
            return argument_handlers[operation](request)
        raise ValueError('Operation is outside installed Settings acceptance')
