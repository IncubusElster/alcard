"""Shared Hello VM platform and a bounded, diskless QMP startup check."""
import json
import os
import selectors
import subprocess
import time


MACHINE_ARGUMENTS = (
    '-machine', 'virt', '-accel', 'tcg,thread=multi',
    '-cpu', 'max', '-smp', '2', '-m', '512',
)
DEVICE_ARGUMENTS = (
    '-device', 'virtio-gpu-pci,xres=320,yres=170,max_outputs=1,edid=off',
    '-device', 'qemu-xhci,id=usb', '-device', 'usb-kbd,bus=usb.0',
    '-display', 'none', '-monitor', 'none', '-no-reboot',
    '-netdev', 'user,id=settingsnet,restrict=on,net=192.0.2.0/24,dhcpstart=192.0.2.100',
    # Direct kernel/disk boot needs no optional PXE ROM from ipxe-qemu.
    '-device', 'virtio-net-pci,netdev=settingsnet,mac=52:54:00:12:34:56,romfile=',
    '-device', 'virtio-serial-pci',
)


def _probe(command, timeout=10, output_limit=65536):
    """Negotiate QMP and quit an initialized, stopped platform over stdio."""
    deadline = time.monotonic() + timeout
    output, buffer = bytearray(), bytearray()
    failure, status, greeting = None, None, None
    process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                               stderr=subprocess.STDOUT, bufsize=0)
    with selectors.DefaultSelector() as selector:
        selector.register(process.stdout, selectors.EVENT_READ)

        def remaining():
            value = deadline - time.monotonic()
            if value <= 0:
                raise TimeoutError('QEMU platform startup deadline exceeded')
            return value

        def collect(chunk):
            available = output_limit - len(output)
            output.extend(chunk[:available])
            if len(chunk) > available:
                raise RuntimeError('QEMU platform output exceeded its limit')

        def receive():
            while True:
                while b'\n' in buffer:
                    line, _, rest = buffer.partition(b'\n')
                    buffer[:] = rest
                    try:
                        value = json.loads(line)
                    except ValueError:
                        # QEMU startup diagnostics share this bounded stream.
                        continue
                    if not isinstance(value, dict):
                        raise RuntimeError('Invalid QMP response')
                    return value
                if not selector.select(remaining()):
                    raise TimeoutError('QEMU platform startup deadline exceeded')
                chunk = os.read(process.stdout.fileno(), 4096)
                if not chunk:
                    raise EOFError('QEMU closed its startup channel')
                collect(chunk)
                buffer.extend(chunk)

        def request(name, identifier):
            remaining()
            process.stdin.write(json.dumps({'execute': name, 'id': identifier}).encode() + b'\n')
            process.stdin.flush()
            while True:
                reply = receive()
                if 'event' in reply:
                    continue
                if reply.get('id') != identifier or 'error' in reply or 'return' not in reply:
                    raise RuntimeError(f'Unexpected QMP reply to {name}: {reply!r}')
                return reply['return']

        try:
            greeting = receive()
            if not isinstance(greeting.get('QMP'), dict):
                raise RuntimeError('Missing QMP greeting')
            request('qmp_capabilities', 1)
            status = request('query-status', 2)
            if (not isinstance(status, dict) or status.get('running') is not False or
                    status.get('status') not in ('prelaunch', 'paused')):
                raise RuntimeError(f'Platform did not stay stopped: {status!r}')
            try:
                request('quit', 3)
            except EOFError:
                # QMP documents EOF before the quit reply as valid. The prior
                # status reply and a clean process exit are still required.
                pass
            if process.wait(timeout=remaining()) != 0:
                raise RuntimeError(f'QEMU exited with status {process.returncode}')
        except (OSError, EOFError, RuntimeError, TimeoutError, subprocess.SubprocessError) as error:
            failure = error
        finally:
            if process.poll() is None:
                process.kill()
            process.wait(timeout=2)
            # The reaped child has closed its pipe; this bounded drain also
            # preserves diagnostics from a startup exit before the first reply.
            while True:
                chunk = os.read(process.stdout.fileno(), min(4096, output_limit - len(output) + 1))
                if not chunk:
                    break
                try:
                    collect(chunk)
                except RuntimeError as error:
                    failure = failure or error
                    break
            process.stdin.close()
            process.stdout.close()
    if failure is not None:
        detail = output.decode(errors='replace')[-4096:].strip()
        raise RuntimeError(f'QEMU platform check failed: {failure}\n{detail}') from failure
    return {'qemu': greeting['QMP'], 'state': status['status'], 'running': False}


def check_platform():
    result = _probe(['qemu-system-aarch64', *MACHINE_ARGUMENTS, *DEVICE_ARGUMENTS,
                     '-S', '-qmp', 'stdio', '-serial', 'none'])
    print('PASS: initialized Hello QEMU platform without guest code, disk or credentials', flush=True)
    print(json.dumps(result, sort_keys=True), flush=True)
