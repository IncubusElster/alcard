#!/usr/bin/env python3
"""One persistent Alpine guest: real Hello keys, reboot, login and idle sample.

Production applications come from the already verified signed APK artifact.
Only this private acceptance harness changes. No observer operation provisions
the user: the ordinary tty1 dispatcher starts Hello, whose own Apply action
invokes its privileged backend. Credentials remain in memory/the private disk.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import time
import tomllib


REPOSITORY = Path(__file__).resolve().parents[3]
SPEC = importlib.util.spec_from_file_location(
    'alcard_login_acceptance', REPOSITORY / 'tools/test-login-vm.py')
LOGIN = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(LOGIN)
SETTINGS_SPEC = importlib.util.spec_from_file_location(
    'installed_settings_acceptance', REPOSITORY / 'scripts/ci/settings/driver.py')
SETTINGS = importlib.util.module_from_spec(SETTINGS_SPEC)
SETTINGS_SPEC.loader.exec_module(SETTINGS)


class HelloFrames(LOGIN.ShellFrames):
    """Observe the same production glyphs in Hello's selected language.

    The ordinary login guest is English. This persistent guest starts in RU
    and exercises live EN/ES/RU changes, so an English-only footer hash cannot
    establish readiness here. Fixed positions and source glyphs remain exact.
    """
    LABELS = {
        'en': ('Quick', 'Menu', 'Applications', 'Open', 'Quick settings', 'Select',
               'Lock screen', 'Log out'),
        'ru': ('Быстро', 'Меню', 'Приложения', 'Откр.', 'Быстрые настройки', 'Выбор',
               'Заблокировать', 'Выйти из сеанса'),
        'es': ('Rápido', 'Menú', 'Aplicaciones', 'Abrir', 'Ajustes rápidos', 'Elegir',
               'Bloquear', 'Cerrar sesión'),
    }

    def __init__(self, root):
        super().__init__(root)
        self.root = root
        self.last_locale = None

    def palette(self, name):
        assert name in ('classic', 'emerald')
        palette = tomllib.loads((self.root / 'rust/crates/alcard-settings-core/themes' /
                                (name + '.toml')).read_text())['palette']
        self.colors = {key: tuple(bytes.fromhex(palette[key][1:])) for key in ('text', 'accent')}

    def centered(self, image, center, label):
        return self.text_matches(image, center - len(label) * 4, 158, label, 'text')

    def classify(self, image):
        if image.size != (320, 170):
            return 'unknown'
        for locale, labels in self.LABELS.items():
            quick, menu, applications, open_label, quick_title, select_label, lock, logout = labels
            result = None
            if (self.text_matches(image, 8, 5, 'ALCARD', 'accent') and
                    self.centered(image, 96, quick) and self.centered(image, 160, menu)):
                result = 'home'
            elif (self.text_matches(image, 8, 5, applications, 'accent') and
                    self.text_matches(image, 8, 20, '000 CI Policy Probe', 'text') and
                    self.centered(image, 32, open_label)):
                result = 'probe-menu'
            elif self.text_matches(image, 16, 22, quick_title, 'accent'):
                result = 'quick-menu'
                if self.centered(image, 32, select_label):
                    if self.text_matches(image, 16, 120, lock, 'text'):
                        result = 'quick-lock'
                    elif self.text_matches(image, 16, 120, logout, 'text'):
                        result = 'quick-logout'
            if result:
                self.last_locale = locale
                return result
        return 'unknown'


class HelloMachine(LOGIN.Machine):
    def __init__(self, directory):
        super().__init__(directory, 'graphics')
        self.evidence = directory / 'evidence' / 'hello'
        self.evidence.mkdir(parents=True, exist_ok=True)
        self.boot_number = 0
        self.boot_ids = []
        self.frames = HelloFrames(REPOSITORY)
        self.dialog_locale = 'ru'
        self.settings = SETTINGS.SettingsAcceptance(self)

    def wait(self, description, predicate, timeout=40, *, operation='state'):
        # A state reply may be delayed by a guest scheduling stall. Bound the
        # read by the caller's existing deadline instead of the transport's
        # unrelated 30-second default. Never replay input or a mutation.
        deadline = time.monotonic() + timeout
        last = None
        while (remaining := deadline - time.monotonic()) > 0:
            if self.process.poll() is not None:
                raise RuntimeError('QEMU exited before ' + description)
            try:
                last = self.call(operation, timeout=remaining)
            except TimeoutError:
                break
            result = predicate(last)
            if result:
                return result
            time.sleep(min(.25, max(0, deadline - time.monotonic())))
        raise AssertionError(f'Timeout: {description}; public state={last}')

    def start(self):
        """Start the same writable disk; never take a QEMU snapshot."""
        self.boot_number += 1
        private = self.directory / 'private'
        disk = private / 'rootfs.ext4'
        assert disk.is_file() and not disk.is_symlink()
        os.chmod(disk, 0o600)
        qmp_socket, observer_socket = self.work / 'qmp', self.work / 'observer'
        for path in (qmp_socket, observer_socket):
            path.unlink(missing_ok=True)
        self.serial = (self.work / 'serial.log').open('wb')
        self.stderr = (self.work / 'qemu.log').open('wb')
        command = [
            'qemu-system-aarch64', '-machine', 'virt', '-accel', 'tcg,thread=multi',
            '-cpu', 'max', '-smp', '2', '-m', '512',
            '-kernel', str(private / 'vmlinuz-virt'),
            '-initrd', str(private / 'initramfs-virt'),
            '-append', 'root=/dev/vda rw rootfstype=ext4 modules=virtio_pci,virtio_blk,ext4 '
                       'console=ttyAMA0 console=tty0 alcard_ci_vm=1',
            '-drive', f'file={disk},format=raw,if=none,id=rootdisk',
            '-device', 'virtio-blk-pci,drive=rootdisk',
            '-device', 'virtio-gpu-pci,xres=320,yres=170,max_outputs=1,edid=off',
            '-device', 'qemu-xhci,id=usb', '-device', 'usb-kbd,bus=usb.0',
            '-display', 'none', '-monitor', 'none', '-no-reboot',
            '-netdev', 'user,id=settingsnet,restrict=on,net=192.0.2.0/24,dhcpstart=192.0.2.100',
            '-device', 'virtio-net-pci,netdev=settingsnet,mac=52:54:00:12:34:56',
            '-qmp', f'unix:{qmp_socket},server=on,wait=off',
            '-serial', 'stdio',
            '-chardev', f'socket,id=control,path={observer_socket},server=on,wait=off',
            '-device', 'virtio-serial-pci',
            '-device', 'virtserialport,chardev=control,name=org.alcard.ci',
        ]
        self.process = subprocess.Popen(command, stdin=subprocess.DEVNULL,
                                        stdout=self.serial, stderr=self.stderr,
                                        start_new_session=True)
        self.qmp = LOGIN.Lines(qmp_socket, self.process)
        assert 'QMP' in self.qmp.receive()
        self.qmp.request({'execute': 'qmp_capabilities'})
        self.observer = LOGIN.Lines(observer_socket, self.process)
        assert self.observer.receive(timeout=240) == {'ready': True, 'protocol': 1}
        metadata = self.call('setup-metadata')
        account_exists = metadata['account']['exists']
        assert account_exists == (self.boot_number > 1), 'Unexpected pre-Hello account state'
        release = self.call('start-interactive')
        assert release['released'] is True
        assert release['completed_before_release'] == (self.boot_number > 1)
        assert release['boot_id'] not in self.boot_ids, 'Kernel did not reboot'
        self.boot_ids.append(release['boot_id'])
        self.note('Released ordinary tty1 dispatcher without backend setup RPC',
                  boot=self.boot_number, release=release,
                  guest={'architecture': 'aarch64', 'ram_mib': 512, 'vcpus': 2,
                         'acceleration': 'tcg', 'persistent_disk': True,
                         'display': [320, 170], 'network': 'isolated_usernet_ethernet'})

    def capture(self, name, *, home=False):
        super().capture(name, home=home)
        if name.endswith('-auth-empty.png'):
            # This is called before credentials are typed. Verify a new native
            # policy dialog observes the current system-language preference.
            title = {'en': 'Authorization', 'ru': 'Авторизация', 'es': 'Autorizacion'}[self.dialog_locale]
            with LOGIN.Image.open(self.evidence / name) as source:
                assert self.frames.text_matches(source.convert('RGB'), 8, 6, title, 'accent'), (
                    'New policy dialog did not use the current system language')

    def hello(self, *, step=None, pane=None, editing=None, focus=None, timeout=35):
        deadline = time.monotonic() + timeout
        last = None
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                raise RuntimeError('QEMU stopped during interactive Hello')
            last = self.call('hello-state')
            if last['running']:
                assert last['columns'] >= 40 and last['rows'] >= 21, last
                assert not last['show_password'], 'Show-password option must remain disabled'
                if all(expected is None or last[key] == expected for key, expected in (
                        ('step', step), ('pane', pane), ('editing', editing), ('focus', focus))):
                    return last
            time.sleep(.15)
        raise AssertionError(f'Hello screen did not reach expected safe metadata: {last}')

    def focus_hello(self, target, *, step, pane='main', count):
        current = self.hello(step=step, pane=pane, editing=False)
        assert isinstance(current['focus'], int) and 0 <= current['focus'] < count, current
        for _ in range((target - current['focus']) % count):
            self.key('down')
        return self.hello(step=step, pane=pane, editing=False, focus=target)

    def hello_text(self, value):
        if value in (self.password, self.bad_password):
            self.credentials_entered = True
        for character in value:
            if 'a' <= character <= 'z' or character in '0123456789':
                self.key(character)
            elif 'A' <= character <= 'Z':
                self.key('shift', character.lower())
            elif character in {' ', '/'}:
                # The installed Cardputer map puts '/' on KEY_HENKAN (92),
                # exposed by QEMU as 'henkan'. PC 'slash' is keycode 53 and
                # deliberately produces '(' in keyboard-zero/console/us.map.
                self.key({' ': 'spc', '/': 'henkan'}[character])
            else:
                raise ValueError('Unsupported fixture character')

    def edit_hello(self, text, *, step, focus, clear=0):
        self.hello(step=step, pane='main', editing=False, focus=focus)
        self.key('5')
        self.hello(step=step, pane='main', editing=True, focus=focus)
        for _ in range(clear):
            self.key('backspace')
        self.hello_text(text)
        self.key('ret')
        self.hello(step=step, pane='main', editing=False, focus=focus)

    def wizard(self):
        self.hello(step=1, pane='main', editing=False, focus=0)
        self.capture('01-hello-welcome.png')
        self.key('5')
        self.hello(step=1, pane='languages', editing=False)
        self.focus_hello(1, step=1, pane='languages', count=3)
        self.key('4')
        self.hello(step=1, pane='main', editing=False)
        self.focus_hello(1, step=1, count=2)
        self.key('5')
        self.hello(step=1, pane='keyboards', editing=False)
        self.focus_hello(1, step=1, pane='keyboards', count=4)
        self.key('5')  # Add RU; mandatory US remains primary.
        self.key('4')
        self.hello(step=1, pane='main', editing=False)
        self.capture('02-hello-language-keyboard.png')
        self.note('Hello language and keyboard pickers operated through real USB keys',
                  language='ru', keyboards=['us', 'ru'])

        self.key('4')
        self.hello(step=2, pane='main', editing=False, focus=0)
        self.capture('03-hello-account-empty.png')
        self.edit_hello('alcardci', step=2, focus=0)
        self.focus_hello(1, step=2, count=9)
        self.edit_hello('Alcard CI', step=2, focus=1)
        self.focus_hello(2, step=2, count=9)
        self.edit_hello(self.password, step=2, focus=2)
        self.focus_hello(4, step=2, count=9)
        self.edit_hello(self.password, step=2, focus=4)
        # Doas stays enabled; root credentials and both reveal toggles stay off.
        self.key('4')
        self.hello(step=3, pane='main', editing=False)
        self.capture('04-hello-network.png')
        self.note('Hello accepted matching user credentials with doas and locked-root plan')
        self.key('6')  # Skip here; ordinary Settings later creates the Ethernet profile.
        self.hello(step=4, pane='main', editing=False, focus=0)
        self.edit_hello('America/Montevideo', step=4, focus=0, clear=len('Etc/UTC'))
        self.capture('05-hello-time.png')
        self.key('4')
        self.hello(step=5, pane='main', editing=False)
        self.focus_hello(0, step=5, count=3)
        self.key('5')  # Explicit Graphics selection.
        self.capture('06-hello-session.png')
        self.key('4')
        self.hello(step=6, pane='main', editing=False)
        self.key('8')
        self.hello(step=5, pane='main', editing=False, focus=0)
        self.key('4')
        self.hello(step=6, pane='main', editing=False)
        self.capture('07-hello-review.png')
        self.note('All six Hello pages and review back/forward navigation observed',
                  pages=6, mode='graphics', timezone='America/Montevideo', ntp=False)
        self.key('4')  # This is the sole provisioning path: Hello's own Apply.
        self.wait('ordinary greetd after interactive Hello Apply', lambda state:
                  state['login_prompt'] and not state['hello']['running'], 120)
        metadata = self.call('setup-metadata')
        assert metadata['account']['exists']
        self.uid = metadata['account']['uid']
        assert 1000 <= self.uid < 65534
        self.note('Interactive Hello Apply completed and returned to ordinary greetd',
                  uid=self.uid, setup_metadata=metadata)

    @staticmethod
    def validate_persistence(value):
        for key, expected in {
                'complete': True, 'pending': False, 'root_locked': True,
                'locale': 'ru_RU.UTF-8', 'keyboards': 'us,ru',
                'timezone': 'America/Montevideo', 'default_session': 'alcard-shell',
                'ntp_enabled': False}.items():
            assert value[key] == expected, (key, value[key])
        assert value['account']['home'] == '/home/alcardci'
        assert not {'wheel', 'video', 'input'} & set(value['groups'])
        assert value['doas']['syntax_valid'] and value['doas']['password_required']
        sentinel = value['sentinel']
        assert sentinel['uid'] == value['account']['uid']
        assert sentinel['gid'] == value['account']['gid']
        assert sentinel['mode'] == '0600'
        assert len(sentinel['sha256']) == 64
        return {key: item for key, item in value.items() if key != 'boot_id'}

    def reboot(self):
        result = self.call('reboot')
        assert result['requested'] and result['boot_id'] == self.boot_ids[-1]
        self.note('Requested ordinary guest reboot of the persistent disk', **result)
        try:
            status = self.process.wait(timeout=60)
        except subprocess.TimeoutExpired as error:
            raise RuntimeError('Guest did not finish its ordinary reboot') from error
        assert status == 0, f'QEMU ended unsuccessfully during reboot: {status}'
        for channel in (self.qmp, self.observer):
            channel.close()
        self.qmp = self.observer = None
        for stream in (self.serial, self.stderr):
            stream.close()
        self.serial = self.stderr = None
        for name in ('serial.log', 'qemu.log'):
            (self.evidence / f'boot-{self.boot_number}-{name}').write_bytes(
                (self.work / name).read_bytes())
        self.process = None
        self.start()

    def validate_resources(self, value):
        assert value['schema'] == 1 and value['architecture'] == 'aarch64'
        assert value['session_uid'] == self.uid and value['sampler_uid'] == 0
        assert value['requested_duration_seconds'] == 30
        assert value['requested_settle_seconds'] == 10
        assert value['elapsed_seconds'] >= 30, 'Idle window was shorter than requested'
        assert value['cpu']['online_cpus_start'] == value['cpu']['online_cpus_end'] == 2
        quality = value['quality']
        assert quality['session_processes_stable'], 'GUI processes changed during idle sampling'
        assert quality['cpu_count_stable'], 'Guest CPU count changed during idle sampling'
        for role in ('shell', 'compositor', 'agent'):
            lifetime = quality['required_roles'][role]
            assert lifetime['same_single_lifetime'], (role, lifetime)
            assert lifetime['initial_count'] == lifetime['final_count'] == 1
        for endpoint in ('start', 'end'):
            memory = value['memory'][endpoint]
            for role in ('shell', 'compositor', 'agent'):
                reading = memory['roles_disjoint'][role]
                assert reading['process_count'] == 1, (endpoint, role, reading)
                assert reading['unreadable_memory_count'] == 0, (endpoint, role, reading)
                assert reading['pss_kib'] > 0, (endpoint, role, reading)
            for group in ('gui_core', 'user_session'):
                reading = memory['views_overlapping'][group]
                assert reading['process_count'] >= 3, (endpoint, group, reading)
                assert reading['unreadable_memory_count'] == 0, (endpoint, group, reading)
                assert reading['pss_kib'] > 0, (endpoint, group, reading)

    def run(self):
        self.start()
        self.wizard()
        assert self.call('prepare-login')['prepared']
        first_sid, _ = self.login()
        self.capture('08-first-login-home.png', home=True)
        before = self.call('persistence-seed')
        self.settings.before_reboot(first_sid)
        before = self.call('persistence-check')
        before_checked = self.validate_persistence(before)
        assert before['boot_id'] == self.boot_ids[0]
        assert before['account']['uid'] == self.uid
        self.note('User-owned home sentinel and nondefault settings recorded before reboot',
                  snapshot=before, session=first_sid)
        # Save the requested measurements before the second boot. The single
        # idle window costs the same time and survives a later lifecycle failure.
        self.no_client('probe')
        self.no_client('foot')
        self.observed_home(time.monotonic() + 40)
        self.capture('09-idle-home.png', home=True)
        self.note('Sampling idle Home after the first proven PAM login',
                  boot_id=self.boot_ids[0], window_seconds=30, settle_seconds=10)
        resources = self.call('idle-resources', timeout=60)
        resources['boot_id'] = self.boot_ids[0]
        resources['measurement_phase'] = 'first_login_after_settings_acceptance_before_reboot'
        (self.evidence / 'idle-resources.json').write_text(json.dumps(resources, indent=2))
        self.validate_resources(resources)
        self.note('Completed guest idle resource sample',
                  report='idle-resources.json', guest_ram_mib=512, guest_vcpus=2,
                  acceleration='tcg', hardware_measurement=False)
        self.reboot()
        self.wait('fresh greetd after reboot without repeated Hello', lambda state:
                  state['login_prompt'] and not state['hello']['running'], 60)
        after = self.call('persistence-check')
        assert self.validate_persistence(after) == before_checked, (
            'Persistent account, home sentinel, settings or admin metadata changed')
        assert after['boot_id'] == self.boot_ids[1] != before['boot_id']
        self.uid = after['account']['uid']
        self.note('Reboot preserved home, settings and administrative route without Hello',
                  snapshot=after)
        second_sid, _ = self.login()
        self.settings.after_reboot()
        # Elogind session labels may repeat across kernel boots; boot_id proves
        # the new lifetime. The inherited login() verifies each actual PAM SID.
        self.capture('10-reboot-login-home.png', home=True)
        self.policy_probe(second_sid, correct=True, label='11-persisted-admin')
        self.no_client('probe')
        self.no_client('foot')
        self.observed_home(time.monotonic() + 40)
        self.logout(second_sid)
        self.settings.console_after_removal()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--directory', type=Path, required=True)
    arguments = parser.parse_args()
    if os.getuid() == 0 or os.environ.get('ALCARD_LOGIN_VM_TEST') != '1':
        raise SystemExit('Only the isolated nonroot CI host may run this gate')
    machine = HelloMachine(arguments.directory.resolve())
    succeeded = False
    try:
        machine.run()
        succeeded = True
    finally:
        if not succeeded:
            machine.settings.failure()
        machine.finish(succeeded)
    print('PASS: one persistent guest, Hello, installed Settings/NetworkManager, reboot, PAM and idle sample', flush=True)


if __name__ == '__main__':
    main()
