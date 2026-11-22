#!/usr/bin/env python3
"""Check the single graphical architecture, including resolved Cargo packages."""
from pathlib import Path
import json
import os
import re
import subprocess
import sys

root = Path(__file__).resolve().parents[1]
app = root / 'rust/apps/alcard-shell'
errors = []
source = '\n'.join(p.read_text() for p in (app / 'src').glob('*.rs'))
for pattern in (r'\bwinit\b', r'\bsoftbuffer\b', r'\bstruct\s+Canvas\b',
                r'\bdraw_scaled\b', r'\bEventLoopProxy\b', r'\bmemfd_create\b', r'\bnix::poll\b'):
    if re.search(pattern, source):
        errors.append(f'legacy source pattern: {pattern}')
if source.count('Connection::connect_to_env(') != 1:
    errors.append('Shell must have exactly one Wayland connection')
for name in ('wayland.rs', 'render.rs', 'input.rs'):
    if not (app / 'src' / name).is_file():
        errors.append(f'missing module: {name}')
# Resolve the actual lockfile rather than guessing from direct dependencies.
cargo = os.environ.get('ALCARD_CARGO', 'cargo')
def resolve(project):
    return json.loads(subprocess.check_output([
        cargo, 'metadata', '--locked', '--format-version=1',
        '--manifest-path', str(project / 'Cargo.toml')], text=True))

metadata = resolve(app)
names = {p['name'] for p in metadata['packages']}
for name in ('winit', 'softbuffer', 'raw-window-handle', 'wgpu', 'iced', 'gtk4', 'slint'):
    if name in names:
        errors.append(f'legacy/unapproved graphical dependency: {name}')
for name in ('smithay-client-toolkit', 'tiny-skia', 'alcard-ui'):
    if name not in names:
        errors.append(f'missing renderer/runtime dependency: {name}')
for name in ('glib', 'polkit-rs', 'polkit-agent-rs'):
    if name in names:
        errors.append(f'authentication dependency leaked into Shell: {name}')
for name in ('font.rs','shm.rs'):
    if not (root/'rust/crates/alcard-ui/src'/name).is_file():
        errors.append(f'missing shared presentation module: {name}')
for name in ('theme.rs', 'preferences.rs', 'network.rs', 'settings.rs'):
    if not (root/'rust/crates/alcard-settings-core/src'/name).is_file():
        errors.append(f'missing shared settings module: {name}')
locker = root / 'rust/apps/alcard-lock'
for name in ('main.rs', 'gui.rs', 'render.rs', 'ui.rs', 'authentication.rs', 'protocol.rs', 'secret.rs'):
    if not (locker / 'src' / name).is_file():
        errors.append(f'missing locker module: {name}')
lock_source = '\n'.join(p.read_text() for p in (locker / 'src').glob('*.rs') if p.name != 'integration.rs')
if lock_source.count('Connection::connect_to_env(') != 1:
    errors.append('Locker must have exactly one Wayland connection')
lock_metadata = resolve(locker)
lock_names = {p['name'] for p in lock_metadata['packages']}
for name in ('winit', 'softbuffer', 'raw-window-handle', 'wgpu', 'iced', 'gtk4', 'slint', 'polkit-rs', 'polkit-agent-rs'):
    if name in lock_names:
        errors.append(f'unapproved locker dependency: {name}')
for name in ('smithay-client-toolkit', 'tiny-skia', 'alcard-ui', 'zeroize'):
    if name not in lock_names:
        errors.append(f'missing locker runtime/security dependency: {name}')
if 'pam_authenticate' in source or 'pam_acct_mgmt' in source:
    errors.append('PAM authentication must remain outside Shell')
console_names = {p['name'] for p in resolve(root / 'rust')['packages']}
for name in ('wayland-client', 'smithay-client-toolkit', 'tiny-skia', 'glib', 'gio',
             'polkit-rs', 'polkit-agent-rs', 'gtk4'):
    if name in console_names:
        errors.append(f'graphical dependency leaked into console workspace: {name}')
agent_names = {p['name'] for p in resolve(root / 'rust/apps/alcard-polkit-agent')['packages']}
for name in ('polkit-rs', 'polkit-agent-rs', 'polkit-sys', 'polkit-agent-sys',
             'winit', 'softbuffer', 'gtk4'):
    if name in agent_names:
        errors.append(f'retired authentication/GUI dependency returned: {name}')
for name in ('glib', 'gio', 'zeroize', 'smithay-client-toolkit', 'tiny-skia'):
    if name not in agent_names:
        errors.append(f'missing native agent dependency: {name}')
if errors:
    sys.exit('\n'.join(errors))
print('PASS: console remains GUI-free; independent Shell/locker and native GLib agent; no retired GUI/polkit wrappers')

