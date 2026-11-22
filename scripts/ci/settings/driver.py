#!/usr/bin/env python3
"""Bounded real installed Settings scenarios within the persistent Hello VM.

There is no new VM or package build. The observer supplies only a private PTY,
controlled guest conditions and independent readback. Actual UI input performs
every accepted setting change; proof records contain no terminal transcript.
"""
import json
import time


class SettingsAcceptance:
    def __init__(self, machine):
        self.machine = machine
        self.path = machine.evidence / 'settings.json'
        self.report = {
            'schema': 1, 'status': 'RUNNING',
            'scope': 'Installed release Settings TUI, real native authentication and NetworkManager',
            'network': 'Isolated QEMU virtual Ethernet with real DHCP and NetworkManager',
            'input': 'Private controlling PTY, 40 columns by 21 rows, provisioned ordinary UID',
            'scenarios': [],
            'not_exercised': [
                'Physical Wi-Fi discovery, RF, PSK/enterprise association or roaming',
                'Real Bluetooth controller, pairing peer, radio transport or audio',
                'Physical backlight, battery, audio output or suspend/resume',
            ],
        }
        self.started = time.monotonic()
        self.last_input = self.started
        self.gui = True
        self.interface = self.uuid = self.before = None
        self.input_revision = -1
        self.write()

    def write(self):
        self.report['elapsed_seconds'] = round(time.monotonic() - self.started, 3)
        self.path.write_text(json.dumps(self.report, indent=2) + '\n')

    def record(self, scenario, **proof):
        self.report['scenarios'].append({'scenario': scenario, 'status': 'PASS', **proof})
        self.write()
        self.machine.note('Installed Settings scenario passed', scenario=scenario, **proof)

    def call(self, operation, timeout=30, **arguments):
        return self.machine.call('settings-' + operation, timeout=timeout, **arguments)

    def state(self):
        return self.call('state')

    def wait(self, description, predicate, timeout=25):
        deadline = time.monotonic() + timeout
        last = None
        while (remaining := deadline - time.monotonic()) > 0:
            if self.machine.process.poll() is not None:
                raise RuntimeError('QEMU exited during installed Settings acceptance')
            if self.gui and time.monotonic() - self.last_input > 30:
                # A real modifier key keeps this interactive test from becoming
                # an idle-lock test while Settings uses a separate terminal.
                self.machine.key('shift', deadline=deadline)
                self.last_input = time.monotonic()
            last = self.call('state', timeout=remaining)
            if predicate(last):
                return last
            time.sleep(min(.15, max(0, deadline - time.monotonic())))
        raise AssertionError('Settings timeout: ' + description + '; safe state=' + repr(last))

    def ui(self, page=None, *, dialog='list', selected=None, status=None, timeout=25):
        return self.wait('responsive ' + str(page) + '/' + dialog, lambda value:
            value.get('alive') and value['alternate'] and not value['canonical'] and
            value['revision'] > self.input_revision and
            not value['echo'] and value['dialog'] == dialog and
            value['status'] != 'working' and
            (page is None or value['page'] == page) and
            (selected is None or value['selected'] == selected) and
            (status is None or value['status'] == status), timeout)

    def key(self, key, count=1):
        self.input_revision = self.call('input', key=key, count=count)['before_revision']
        # Mutation and follow-up observation remain sequential. This tiny gap
        # prevents a parser from mistaking the old frame for the new one.
        time.sleep(.08)

    def text(self, value, *, secret=False):
        self.input_revision = self.call('input', text=value, secret=secret)['before_revision']

    def choose_row(self, row, page, *, dialog='list'):
        state = self.ui(page, dialog=dialog)
        assert isinstance(state['selected'], int), 'No selected Settings row'
        difference = row - state['selected']
        if difference:
            self.key('down' if difference > 0 else 'up', abs(difference))
        self.ui(page, dialog=dialog, selected=row)

    def open_row(self, row, page, destination):
        self.choose_row(row, page)
        self.key('enter')
        return self.ui(destination)

    def back(self, page):
        self.key('back')
        return self.ui(page)

    def home(self):
        for _ in range(5):
            state = self.ui()
            if state['page'] == 'home':
                return state
            self.key('back')
        raise AssertionError('Settings did not return to Home')

    def native_prompt(self, route):
        value = self.wait('private native ' + route + ' prompt', lambda state:
            state.get('alive') and state['auth_prompt'] and state['canonical'] and
            not state['echo'] and route in state['auth_children'])
        assert not value['alternate'] and value['signals']
        return value

    def authorize(self, *, route=None):
        state = self.wait('completion or native authentication', lambda value:
            value.get('alive') and (
                value['auth_prompt'] or (value['alternate'] and
                value['revision'] > self.input_revision and value['dialog'] == 'list' and
                value['status'] in ('saved', 'unavailable'))), 40)
        if state['auth_prompt']:
            if route:
                self.native_prompt(route)
            assert not state['echo']
            self.text(self.machine.password, secret=True)
            self.key('enter')
        state = self.ui(status='saved', timeout=40)
        assert not state['signals'], 'Settings did not restore raw terminal mode'
        return state

    def begin_edit(self, row, page, value):
        self.choose_row(row, page)
        self.key('enter')
        self.ui(page, dialog='edit')
        self.key('erase', 128)
        if value:
            self.text(value)

    def edit(self, row, page, value, *, route=None):
        self.begin_edit(row, page, value)
        self.key('enter')
        return self.authorize(route=route)

    def wrong_password(self, route, value, *, scenario=None):
        before = self.call('readback')['hostname']
        self.begin_edit(0, 'system', value)
        self.key('enter')
        prompt = self.native_prompt(route)
        self.text(self.machine.bad_password, secret=True)
        self.key('enter')
        rejected = self.wait('wrong native credential rejected', lambda state:
            state.get('alive') and (state['auth_failed'] or (
                state['alternate'] and state['status'] == 'unavailable')), 15)
        if not rejected['alternate']:
            self.key('interrupt')
        restored = self.ui('system', status='unavailable')
        assert restored['alternate_entries'] > prompt['alternate_entries']
        assert self.call('readback')['hostname'] == before
        self.record(scenario or route + '_incorrect_password', terminal=restored,
                    unchanged_hostname=True)

    def interrupt_password(self, route, value):
        before = self.call('readback')['hostname']
        self.begin_edit(0, 'system', value)
        self.key('enter')
        prompt = self.native_prompt(route)
        self.key('interrupt')
        restored = self.ui('system', status='unavailable')
        assert restored['alternate_entries'] > prompt['alternate_entries']
        assert self.call('readback')['hostname'] == before
        self.choose_row(1, 'system')
        self.choose_row(0, 'system')
        self.record(route + '_ctrl_c_restores_terminal', terminal=restored,
                    unchanged_hostname=True, subsequent_navigation=True)

    def language(self, code):
        self.home()
        self.open_row(1, 'home', 'language')
        self.key('enter')
        self.ui('language', dialog='choose')
        self.choose_row({'en': 0, 'ru': 1, 'es': 2}[code], 'language', dialog='choose')
        self.key('enter')
        state = self.authorize()
        assert state['locale'] == code
        self.home()
        self.machine.wait_shell_frame({'home'}, time.monotonic() + 20,
                                      'Shell live language reload to ' + code)
        assert self.machine.frames.last_locale == code, 'Shell did not display the new locale'
        self.machine.dialog_locale = code
        self.record('language_' + code + '_tui_and_live_shell',
                    locale=code, readback=self.call('readback')['locale'])

    def profile(self):
        self.home()
        self.open_row(3, 'home', 'network')
        self.open_row(0, 'network', 'profiles')
        self.open_row(0, 'profiles', 'profile')

    def network(self):
        self.home()
        self.open_row(3, 'home', 'network')
        self.open_row(1, 'network', 'device')
        self.key('enter')  # NetworkManager Device.Connect creates the first profile.
        self.authorize()
        created = self.call('readback')
        assert created['profile_count'] == 1
        self.uuid = created['profile']['connection.uuid']
        assert created['profile']['ipv4.method'] == 'auto'
        assert '192.0.2.100/24' in created['device'].get('IP4.ADDRESS[1]', '')
        assert created['device']['GENERAL.CON-UUID'] == self.uuid
        self.record('ethernet_profile_created_and_dhcp_activated_through_tui', readback=created)

        self.profile()
        self.choose_row(8, 'profile')
        self.key('enter')
        self.ui('profile', dialog='confirm')
        self.key('escape')
        self.ui('profile')
        assert self.call('readback')['profile']['connection.uuid'] == self.uuid
        self.record('forget_confirmation_cancelled_without_deleting_profile', uuid=self.uuid)

        self.edit(5, 'profile', '192.0.2.110/24')
        self.edit(6, 'profile', '192.0.2.2')
        self.edit(7, 'profile', '192.0.2.3')
        configured = self.call('readback')['profile']
        assert configured['ipv4.method'] == 'manual'
        assert configured['ipv4.addresses'] == '192.0.2.110/24'
        assert configured['ipv4.gateway'] == '192.0.2.2'
        assert configured['ipv4.dns'] == '192.0.2.3'
        assert configured['ipv4.ignore-auto-dns'] == 'yes'
        self.choose_row(1, 'profile')
        self.key('enter')
        self.authorize()
        static = self.call('readback')
        assert static['device']['GENERAL.CON-UUID'] == self.uuid
        assert static['device'].get('IP4.ADDRESS[1]') == '192.0.2.110/24'
        assert static['device'].get('IP4.GATEWAY') == '192.0.2.2'
        assert static['device'].get('IP4.DNS[1]') == '192.0.2.3'
        self.record('static_ipv4_gateway_dns_applied_and_active_readback', readback=static)

        self.choose_row(2, 'profile')
        self.key('enter')
        self.authorize()
        assert self.call('readback')['profile']['connection.autoconnect'] == 'no'
        self.key('enter')
        self.authorize()
        assert self.call('readback')['profile']['connection.autoconnect'] == 'yes'
        self.record('autoconnect_toggle_roundtrip', uuid=self.uuid)

        self.choose_row(4, 'profile')
        self.key('enter')
        self.ui('profile', dialog='confirm')
        self.key('escape')
        self.ui('profile')
        assert self.call('readback')['profile']['ipv4.method'] == 'manual'
        self.key('enter')
        self.ui('profile', dialog='confirm')
        self.key('enter')
        self.authorize()
        automatic = self.call('readback')['profile']
        assert automatic['ipv4.method'] == 'auto'
        assert all(automatic[key] in ('', '--') for key in (
            'ipv4.addresses', 'ipv4.gateway', 'ipv4.dns'))
        assert automatic['ipv4.ignore-auto-dns'] == 'no'
        self.choose_row(1, 'profile')
        self.key('enter')
        self.authorize()
        dhcp = self.call('readback')
        assert dhcp['device'].get('IP4.ADDRESS[1]') == '192.0.2.100/24'
        assert dhcp['device']['GENERAL.CON-UUID'] == self.uuid
        self.record('dhcp_cancel_then_apply_clears_static_fields_and_reactivates', readback=dhcp)

        self.home()
        self.open_row(3, 'home', 'network')
        self.call('service', action='stop', timeout=35)
        try:
            self.key('refresh')
            unavailable = self.ui('network', status='unavailable', timeout=20)
            self.back('home')
            self.record('networkmanager_unavailable_ui_remains_navigable', terminal=unavailable)
        finally:
            self.call('service', action='start', timeout=35)
        self.open_row(3, 'home', 'network')
        self.open_row(1, 'network', 'device')
        self.record('networkmanager_service_recovery', readback=self.call('readback'))
        self.home()

    def appearance(self):
        self.open_row(7, 'home', 'appearance')
        self.key('enter')
        self.ui('appearance', dialog='choose')
        self.choose_row(1, 'appearance', dialog='choose')
        self.key('enter')
        self.authorize()
        assert self.call('readback')['preferences']['theme'] == 'emerald'
        self.machine.frames.palette('emerald')
        self.machine.wait_shell_frame({'home'}, time.monotonic() + 20, 'live Emerald palette')
        self.machine.capture('settings-emerald-home.png')
        self.key('enter')
        self.ui('appearance', dialog='choose')
        self.choose_row(0, 'appearance', dialog='choose')
        self.key('enter')
        self.authorize()
        self.machine.frames.palette('classic')
        self.machine.wait_shell_frame({'home'}, time.monotonic() + 20, 'live Classic palette restored')
        self.choose_row(2, 'appearance')
        self.key('enter')
        self.ui('appearance', dialog='choose')
        self.choose_row(1, 'appearance', dialog='choose')
        self.key('enter')
        self.authorize()
        preferences = self.call('readback')['preferences']
        assert preferences['theme'] == 'classic' and preferences['icon_style'] == 'outline'
        self.record('appearance_preferences_and_live_shell_palette_roundtrip', preferences=preferences)
        self.home()

    def before_reboot(self, sid):
        try:
            prepared = self.call('prepare', timeout=40)
            assert prepared['profile_count'] == 0, 'Profile creation test requires no Ethernet profile'
            assert prepared['uid'] == self.machine.uid and prepared['root_locked']
            self.interface = prepared['interface']
            initial = self.call('readback')
            shell_lifetime = initial['shell']
            assert len(shell_lifetime) == 1
            self.record('controlled_guest_preconditions', **prepared)
            self.call('start')
            self.ui('home')
            self.open_row(0, 'home', 'system')
            self.begin_edit(0, 'system', 'alcard-cancelled')
            self.key('escape')
            self.ui('system')
            assert self.call('readback')['hostname'] == initial['hostname']
            self.record('text_editor_cancel_preserves_hostname')
            self.wrong_password('doas', 'alcard-wrong-doas')
            self.interrupt_password('doas', 'alcard-interrupt-doas')
            restored = self.edit(0, 'system', 'alcard-settings-doas', route='doas')
            assert self.call('readback')['hostname'] == 'alcard-settings-doas'
            self.record('doas_correct_password_applies_hostname', terminal=restored)

            denied = self.call('auth-mode', mode='denied')
            assert denied['root_locked']
            self.wrong_password('su', 'alcard-denied', scenario='su_locked_root_denies_password')
            assert self.call('readback')['hostname'] == 'alcard-settings-doas'
            self.record('no_doas_locked_root_refuses_privileged_change')
            mode = self.call('auth-mode', mode='su', password=self.machine.password)
            assert not mode['root_locked']
            self.wrong_password('su', 'alcard-wrong-su')
            self.interrupt_password('su', 'alcard-interrupt-su')
            restored = self.edit(0, 'system', 'alcard-settings-su', route='su')
            assert self.call('readback')['hostname'] == 'alcard-settings-su'
            self.record('su_root_password_applies_hostname', terminal=restored)
            self.edit(0, 'system', initial['hostname'], route='su')
            assert self.call('auth-mode', mode='doas')['root_locked']

            self.language('en')
            self.network()
            self.appearance()
            self.language('es')
            self.machine.policy_probe(sid, correct=False, label='settings-es-new-dialog')
            self.record('new_polkit_dialog_uses_current_spanish_locale',
                        screenshot='settings-es-new-dialog-auth-empty.png', wrong_password_rejected=True)
            self.language('ru')
            final = self.call('readback')
            assert final['hostname'] == initial['hostname'] and final['root_locked']
            assert final['shell'] == shell_lifetime, 'Live preference reload restarted Shell'
            self.record('settings_language_and_appearance_reloaded_without_shell_restart', shell=final['shell'])
            self.before = final
            self.key('back')
            exited = self.wait('ordinary Settings exit', lambda state: not state.get('alive'))
            assert exited['exit_status'] == 0 and exited['canonical'] and exited['echo']
            assert not exited['alternate']
            self.record('settings_normal_exit_restores_terminal', terminal=exited)
            self.call('cleanup')
        except BaseException:
            self.report['status'] = 'FAIL'
            self.write()
            try:
                self.call('cleanup', timeout=40)
            except Exception:
                pass
            raise

    def after_reboot(self):
        value = self.call('resume', interface=self.interface)
        assert value['root_locked'] and value['locale'] == self.before['locale']
        assert value['preferences'] == self.before['preferences']
        assert value['hostname'] == self.before['hostname']
        assert value['profile'] == self.before['profile']
        assert value['device']['GENERAL.CON-UUID'] == self.uuid
        assert value['device'].get('IP4.ADDRESS[1]') == '192.0.2.100/24'
        self.call('start')
        self.input_revision = -1
        assert self.ui('home')['locale'] == 'ru'
        self.profile()
        state = self.ui('profile')
        self.record('reboot_preserves_profile_preferences_locale_and_tui_readback', readback=value,
                    terminal=state)
        self.home()
        self.key('back')
        self.wait('Settings exit after reboot', lambda state: not state.get('alive'))
        self.call('stop')

    def console_after_removal(self):
        self.gui = False
        removed = self.call('console-remove', timeout=60)
        assert removed['uid'] == self.machine.uid
        assert removed['readback']['profile']['connection.uuid'] == self.uuid
        self.call('start')
        self.input_revision = -1
        self.ui('home')
        self.open_row(0, 'home', 'system')
        self.ui('system')
        self.profile()
        self.choose_row(1, 'profile')
        self.key('enter')
        self.authorize()
        value = self.call('readback')
        assert value['profile']['connection.uuid'] == self.uuid
        assert value['device']['GENERAL.CON-UUID'] == self.uuid
        assert value['device'].get('IP4.ADDRESS[1]') == '192.0.2.100/24'
        self.home()
        self.key('back')
        exited = self.wait('Console Settings exit', lambda state: not state.get('alive'))
        assert exited['exit_status'] == 0 and exited['canonical'] and exited['echo']
        self.call('stop')
        self.record('console_settings_and_saved_network_survive_gui_package_removal',
                    packages=removed, readback=value,
                    boundary='Same-account PTY smoke; real getty login is covered by the separate Console VM')
        self.report['status'] = 'PASS'
        self.write()

    def failure(self):
        self.report['status'] = 'FAIL'
        self.write()
