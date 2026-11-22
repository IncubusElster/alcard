#!/usr/bin/python3
"""Read-only idle measurements for a disposable Alcard Linux guest.

Run after login and stop sending input/polling the guest until this returns.
PSS is sampled at both ends; CPU is the average of cumulative counter deltas.
The guest includes this sampler and the private CI observer. Their process
costs are identified, but no subtraction from MemAvailable-based use is made.
Results describe this VM, not physical Cardputer power or CPU performance.
"""

import argparse
import json
import os
from pathlib import Path
import time


PROC = Path('/proc')
PROGRAMS = {
    'alcard-shell': 'shell',
    'alcard-polkit-agent': 'agent',
    'alcard-lock': 'locker',
    'labwc': 'compositor',
    'foot': 'terminal',
    'elogind': 'elogind',
    'polkitd': 'polkitd',
    'greetd': 'greetd',
    'agreety': 'agreety',
    'dbus-run-session': 'session-launcher',
}
SCRIPTS = {
    b'/usr/libexec/alcard-ci/observer.py': 'ci-observer',
    b'/usr/libexec/alcard-ci/boot': 'ci-boot',
    b'/usr/libexec/alcard-ci/measure-idle-resources.py': 'ci-sampler',
    b'/usr/libexec/alcard/session-client': 'session-supervisor',
    b'/usr/libexec/alcard/shell-session': 'session-launcher',
    b'/usr/libexec/alcard/tty1-dispatch': 'tty-dispatch',
}
MEMORY_FIELDS = ('MemTotal', 'MemFree', 'MemAvailable', 'Buffers', 'Cached',
                 'SReclaimable', 'SUnreclaim', 'Shmem', 'Slab', 'KernelStack',
                 'PageTables', 'SwapTotal', 'SwapFree')
REQUIRED_ROLES = ('shell', 'compositor', 'agent')
INSTRUMENTATION = {'ci-sampler', 'ci-observer', 'ci-boot'}
GUI_CORE = {'shell', 'compositor', 'agent', 'locker'}
SYSTEM_SERVICES = {'system-bus', 'elogind', 'polkitd', 'greetd', 'agreety'}


def colon_numbers(path):
    result = {}
    for line in path.read_text().splitlines():
        name, separator, value = line.partition(':')
        fields = value.split()
        if separator and fields and fields[0].isdigit():
            result[name] = int(fields[0])
    return result


def cpu_counters():
    lines = (PROC / 'stat').read_text().splitlines()
    values = [int(value) for value in lines[0].split()[1:]]
    if len(values) < 8:
        raise RuntimeError('Incomplete Linux CPU counters')
    # guest and guest_nice are already included in user/nice. Do not add twice.
    fields = ('user', 'nice', 'system', 'idle', 'iowait', 'irq', 'softirq', 'steal')
    return {'ticks': dict(zip(fields, values[:8])),
            'online_cpus': sum(line.split()[0][3:].isdigit() for line in lines
                               if line.startswith('cpu'))}


def process_stat(path):
    # comm can contain spaces and parentheses. Field 3 starts after the last ).
    fields = (path / 'stat').read_text().rsplit(')', 1)[1].split()
    return {'ppid': int(fields[1]), 'state': fields[0],
            'kernel_thread': bool(int(fields[6]) & 0x00200000),
            'cpu_ticks': int(fields[11]) + int(fields[12]),
            'start_ticks': int(fields[19])}


def role_for(path, uid, session_uid, sampler_pid):
    if int(path.name) == sampler_pid:
        return 'ci-sampler'
    # Fixed names identify fixture/supervisor scripts. No arguments or
    # environment contents are copied into the result.
    try:
        arguments = (path / 'cmdline').read_bytes().split(b'\0')
        for script, role in SCRIPTS.items():
            if script in arguments:
                return role
        executable = Path(os.readlink(path / 'exe').removesuffix(' (deleted)')).name
        if executable == 'dbus-daemon':
            return 'session-bus' if uid == session_uid else 'system-bus'
        if executable in PROGRAMS:
            return PROGRAMS[executable]
    except OSError:
        pass
    return 'session-other' if uid == session_uid else 'system-other'


def process_memory(path, kernel_thread):
    if kernel_thread:
        return {'pss_kib': 0, 'rss_kib': 0, 'private_kib': 0, 'swap_pss_kib': 0}
    try:
        values = colon_numbers(path / 'smaps_rollup')
    except OSError:
        return None
    if not {'Pss', 'Rss', 'Private_Clean', 'Private_Dirty'} <= values.keys():
        return None
    return {'pss_kib': values['Pss'], 'rss_kib': values['Rss'],
            'private_kib': values['Private_Clean'] + values['Private_Dirty']
                           + values.get('Private_Hugetlb', 0),
            'swap_pss_kib': values.get('SwapPss')}


def snapshot(session_uid, sampler_pid):
    started = time.monotonic()
    cpu = cpu_counters()
    uptime = float((PROC / 'uptime').read_text().split()[0])
    meminfo = colon_numbers(PROC / 'meminfo')
    processes = {}
    omitted = 0
    for path in PROC.iterdir():
        if not path.name.isdigit():
            continue
        try:
            before = process_stat(path)
            if before['state'] in {'Z', 'X'}:
                continue
            status = colon_numbers(path / 'status')
            uid = status['Uid']
            role = ('kernel-thread' if before['kernel_thread'] else
                    role_for(path, uid, session_uid, sampler_pid))
            memory = process_memory(path, before['kernel_thread'])
            after = process_stat(path)
            if before['start_ticks'] != after['start_ticks'] or after['state'] in {'Z', 'X'}:
                omitted += 1
                continue
            item = {'pid': int(path.name), 'uid': uid, 'role': role,
                    **before, 'memory': memory}
            processes[(item['pid'], item['start_ticks'])] = item
        except (OSError, ValueError, IndexError, KeyError):
            omitted += 1
    memory = {key + '_kib': meminfo.get(key) for key in MEMORY_FIELDS}
    if meminfo.get('MemAvailable') is None:
        raise RuntimeError('MemAvailable is required for comparable guest memory use')
    memory['used_from_available_kib'] = meminfo['MemTotal'] - meminfo['MemAvailable']
    return {'monotonic': started, 'uptime_seconds': uptime, 'cpu': cpu,
            'memory': memory, 'processes': processes, 'omitted_during_scan': omitted,
            'scan_seconds': time.monotonic() - started}


def memory_total(items):
    items = list(items)
    readable = [item['memory'] for item in items if item['memory'] is not None]
    return {'process_count': len(items), 'unreadable_memory_count': len(items) - len(readable),
            'pss_kib': sum(item['pss_kib'] for item in readable),
            'private_kib': sum(item['private_kib'] for item in readable),
            'swap_pss_kib': sum(item['swap_pss_kib'] or 0 for item in readable),
            'unavailable_swap_pss_count': sum(item['swap_pss_kib'] is None for item in readable)}


def memory_views(state, session_uid):
    processes = list(state['processes'].values())
    userspace = [item for item in processes if not item['kernel_thread']]
    return {
        'roles_disjoint': {role: memory_total(item for item in userspace if item['role'] == role)
                           for role in sorted({item['role'] for item in userspace})},
        # These named views overlap; they must not be added together.
        'views_overlapping': {
            'all_userspace': memory_total(userspace),
            'all_userspace_excluding_instrumentation': memory_total(
                item for item in userspace if item['role'] not in INSTRUMENTATION),
            'user_session': memory_total(item for item in userspace if item['uid'] == session_uid),
            'gui_core': memory_total(item for item in userspace if item['role'] in GUI_CORE),
            'system_services': memory_total(item for item in userspace if item['role'] in SYSTEM_SERVICES),
            'instrumentation': memory_total(item for item in userspace if item['role'] in INSTRUMENTATION),
        },
    }


def compare(start, end, session_uid, hz):
    elapsed = end['monotonic'] - start['monotonic']
    cpu_count = start['cpu']['online_cpus']
    deltas = {name: end['cpu']['ticks'][name] - value
              for name, value in start['cpu']['ticks'].items()}
    total = sum(deltas.values())
    if total <= 0 or any(value < 0 for value in deltas.values()):
        raise RuntimeError('CPU counters did not advance monotonically')
    process_results = []
    stable, entered = [], []
    for identity, item in end['processes'].items():
        previous = start['processes'].get(identity)
        ticks = None
        if previous is not None:
            status = 'stable'
            ticks = item['cpu_ticks'] - previous['cpu_ticks']
            stable.append(identity)
        elif item['start_ticks'] >= start['uptime_seconds'] * hz:
            status = 'born_during_window'
            ticks = item['cpu_ticks']
            entered.append(identity)
        else:
            status = 'absent_from_initial_scan'
            entered.append(identity)
        if ticks is not None and ticks < 0:
            raise RuntimeError('Process CPU counter decreased in the same lifetime')
        core_percent = 100 * ticks / hz / elapsed if ticks is not None else None
        process_results.append({
            'pid': item['pid'], 'start_ticks': item['start_ticks'], 'uid': item['uid'],
            'role': item['role'], 'kernel_thread': item['kernel_thread'], 'lifetime': status,
            'memory_start': previous['memory'] if previous else None,
            'memory_end': item['memory'], 'cpu_ticks_delta': ticks,
            'cpu_percent_one_core': core_percent,
            'cpu_percent_vm_capacity': core_percent / cpu_count if core_percent is not None else None,
        })
    departed = [item for identity, item in start['processes'].items()
                if identity not in end['processes']]
    required = {}
    for role in REQUIRED_ROLES:
        initial = [identity for identity, item in start['processes'].items()
                   if item['role'] == role and item['uid'] == session_uid]
        final = [identity for identity, item in end['processes'].items()
                 if item['role'] == role and item['uid'] == session_uid]
        required[role] = {'initial_count': len(initial), 'final_count': len(final),
                          'same_single_lifetime': len(initial) == 1 and initial == final}
    by_role = {}
    for item in process_results:
        role = by_role.setdefault(item['role'], {'process_count': 0, 'cpu_ticks_delta': 0,
                                                'unaccounted_process_count': 0})
        role['process_count'] += 1
        if item['cpu_ticks_delta'] is None:
            role['unaccounted_process_count'] += 1
        else:
            role['cpu_ticks_delta'] += item['cpu_ticks_delta']
    for item in departed:
        role = by_role.setdefault(item['role'], {'process_count': 0, 'cpu_ticks_delta': 0,
                                                'unaccounted_process_count': 0})
        role['unaccounted_process_count'] += 1
    for role in by_role.values():
        role['cpu_percent_one_core'] = 100 * role['cpu_ticks_delta'] / hz / elapsed
        role['cpu_percent_vm_capacity'] = role['cpu_percent_one_core'] / cpu_count
    executing = sum(deltas[key] for key in ('user', 'nice', 'system', 'irq', 'softirq'))
    return {
        'guest_uptime_seconds': {'start': start['uptime_seconds'], 'end': end['uptime_seconds']},
        'elapsed_seconds': elapsed, 'cpu': {
            'online_cpus_start': cpu_count, 'online_cpus_end': end['cpu']['online_cpus'],
            'ticks_delta': deltas, 'total_ticks_delta': total,
            'executing_percent_vm_capacity': 100 * executing / total,
            'idle_percent_vm_capacity': 100 * deltas['idle'] / total,
            'iowait_percent_vm_capacity': 100 * deltas['iowait'] / total,
            'steal_percent_vm_capacity': 100 * deltas['steal'] / total,
            'roles_disjoint': by_role,
        },
        'memory': {'start': {**start['memory'], **memory_views(start, session_uid)},
                   'end': {**end['memory'], **memory_views(end, session_uid)}},
        'quality': {
            'required_roles': required,
            'session_processes_stable': all(value['same_single_lifetime'] for value in required.values()),
            'cpu_count_stable': cpu_count == end['cpu']['online_cpus'],
            'endpoint_scan_seconds': [start['scan_seconds'], end['scan_seconds']],
            'omitted_during_endpoint_scans': [start['omitted_during_scan'], end['omitted_during_scan']],
            'stable_lifetime_count': len(stable), 'entered_lifetime_count': len(entered),
            'departed': [{'pid': item['pid'], 'start_ticks': item['start_ticks'],
                          'role': item['role']} for item in departed],
            'per_process_cpu_complete_for_observed_lifetimes': not departed and all(
                item['cpu_ticks_delta'] is not None for item in process_results),
        },
        'processes': sorted(process_results, key=lambda item: (item['role'], item['pid'])),
    }


def bounded_seconds(value):
    number = float(value)
    if not 0 <= number <= 60:
        raise argparse.ArgumentTypeError('seconds must be within 0..60')
    return number


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--uid', type=int, required=True, help='ordinary logged-in user UID')
    parser.add_argument('--settle', type=bounded_seconds, default=10)
    parser.add_argument('--duration', type=bounded_seconds, default=30)
    args = parser.parse_args()
    if args.uid < 1 or args.duration < 1:
        parser.error('UID must be positive and duration at least one second')
    hz = os.sysconf('SC_CLK_TCK')
    time.sleep(args.settle)
    start = snapshot(args.uid, os.getpid())
    # Count the first scan within the requested window, not as extra idle time.
    time.sleep(max(0, args.duration - (time.monotonic() - start['monotonic'])))
    end = snapshot(args.uid, os.getpid())
    result = {'schema': 1, 'architecture': os.uname().machine,
              'kernel_release': os.uname().release, 'session_uid': args.uid,
              'sampler_uid': os.getuid(), 'clock_ticks_per_second': hz,
              'requested_settle_seconds': args.settle,
              'requested_duration_seconds': args.duration,
              **compare(start, end, args.uid, hz),
              'interpretation': {
                  'memory_units': 'KiB (Linux procfs kB means 1024 bytes)',
                  'memory_sampling': 'Two endpoints, not peak or continuous resident memory',
                  'pss': 'Proportional shared-page accounting; aggregate PSS, never aggregate RSS',
                  'rss': 'Per-process only; shared pages can appear in multiple processes',
                  'guest_used': 'MemTotal - MemAvailable; includes kernel and measurement fixtures',
                  'cpu_one_core': '100% means one fully occupied vCPU; quantized to kernel clock ticks',
                  'cpu_vm_capacity': '100% means all guest vCPUs; guest CPU counters exclude iowait/steal from executing',
                  'process_cpu_churn': 'Exited processes may have unobserved CPU; aggregate guest counters include all activity',
                  'instrumentation': 'Sampler/observer/CI-boot roles are reported, not subtracted from guest memory',
                  'conditions': 'Caller must keep input and guest polling quiescent throughout this interval',
                  'scope': 'Virtual guest only; does not establish physical Cardputer CPU or energy use',
              }}
    print(json.dumps(result, sort_keys=True, allow_nan=False))


if __name__ == '__main__':
    main()
