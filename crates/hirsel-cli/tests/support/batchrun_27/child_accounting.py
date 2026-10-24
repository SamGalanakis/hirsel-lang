"""Linux kernel accounting, independent of fixture readiness and language tapes.

Follows strace's event loop (src/strace.c, ptrace event/stop dispatch) and
src/syscall.c's PTRACE_GET_SYSCALL_INFO entry/exit pairing and
filter_seccomp.c:65-88's selective SECCOMP_RET_TRACE filter. Unlike strace we
only decode x86-64 wait4/waitid: this harness runs on the x86-64 pool. All
created processes count, including failed exec and death before user code.
"""
import ctypes
import errno
import json
import os
from pathlib import Path
import signal
import select
import time
import struct
import sys

LIBC = ctypes.CDLL(None, use_errno=True)
LIBC.ptrace.restype = ctypes.c_long


def ptrace(request, pid, address=0, data=0):
    ctypes.set_errno(0)
    result = LIBC.ptrace(request, pid, ctypes.c_void_p(address), ctypes.c_void_p(data))
    if result == -1 and ctypes.get_errno():
        raise OSError(ctypes.get_errno(), 'ptrace')
    return result


def word(pid, address):
    return ptrace(2, pid, address) & ((1 << 64) - 1)  # PEEKDATA


def message(pid):
    value = ctypes.c_ulong()
    ptrace(0x4201, pid, data=ctypes.addressof(value))  # GETEVENTMSG
    return value.value


def assert_reaped(row):
    pid = row['pid']
    assert not Path('/proc', str(pid)).exists(), ('unreaped', row)
    assert row.get('parent_wait_status') == row.get('kernel_exit_status') is not None, ('missing/exact wait proof', row)
    assert row.get('tracer_wait_status') == row['kernel_exit_status'], row


def account(report, markers):
    children = report['children']
    by_pid = {row['pid']: row for row in children}
    assert len(by_pid) == len(children), report
    assert set(markers.values()) <= by_pid.keys(), ('unaccounted marker', markers, report)
    for row in children:
        assert_reaped(row)
        row['readiness'] = 'ready' if row['pid'] in markers.values() else 'no-marker'
        if not row.get('exec'):
            row['state'] = ('exec failed' if row.get('exec_error_errno') else
                            'killed before the marker' if os.WIFSIGNALED(row['kernel_exit_status']) else 'never execed')
        elif row['readiness'] == 'ready':
            row['state'] = 'ready then signaled' if os.WIFSIGNALED(row['kernel_exit_status']) else 'ready then exited'
        else:
            assert os.WIFSIGNALED(row['kernel_exit_status']), ('exited before marker', row)
            row['state'] = 'killed before the marker'
    return children


def install_wait_filter():
    # Like strace's --seccomp-bpf: avoid stopping the interpreter's unrelated
    # filesystem/compiler syscalls. Fork/exec/exit events still account for all
    # processes. The filter is inherited by every tracee and changes no result.
    class Instruction(ctypes.Structure):
        _fields_ = [('code', ctypes.c_ushort), ('jt', ctypes.c_ubyte),
                    ('jf', ctypes.c_ubyte), ('k', ctypes.c_uint)]

    class Program(ctypes.Structure):
        _fields_ = [('length', ctypes.c_ushort), ('instructions', ctypes.POINTER(Instruction))]

    # Unknown syscall ABIs stop explicitly rather than silently bypass accounting.
    instructions = [(0x20, 0, 0, 4), (0x15, 1, 0, 0xc000003e),
                    (0x06, 0, 0, 0x7ff00000), (0x20, 0, 0, 0)]
    for number in (61, 247, 59, 322):
        instructions.extend([(0x15, 0, 1, number), (0x06, 0, 0, 0x7ff00000)])
    instructions.append((0x06, 0, 0, 0x7fff0000))  # ALLOW
    array = (Instruction * len(instructions))(*(Instruction(*row) for row in instructions))
    program = Program(len(array), array)
    if LIBC.prctl(38, 1, 0, 0, 0) != 0 or LIBC.prctl(22, 2, ctypes.byref(program), 0, 0) != 0:
        raise OSError(ctypes.get_errno(), 'install seccomp observer filter')


def trace(argv, output, hold_exec_until=None):
    root = os.fork()
    if root == 0:
        try:
            ptrace(0, 0)  # TRACEME
            install_wait_filter()
            os.kill(os.getpid(), signal.SIGSTOP)
            os.execvpe(argv[0], argv, os.environ)
        finally:
            os._exit(127)
    live = {root}
    processes = {}
    pending_stops = set()
    waits = {}
    result = None
    try:
        _, status = os.waitpid(root, 0)
        assert os.WIFSTOPPED(status), status
        # TRACESYSGOOD, TRACEFORK/VFORK/CLONE/EXEC/EXIT/SECCOMP, EXITKILL.
        ptrace(0x4200, root, data=1 | 2 | 4 | 8 | 16 | 64 | 128 | 0x100000)
        ptrace(7, root)  # CONT; only selected syscalls produce seccomp stops
        while live:
            tid, status = os.waitpid(-1, 0x40000000)  # __WALL, our tracees only
            if os.WIFEXITED(status) or os.WIFSIGNALED(status):
                live.remove(tid)
                if tid == root:
                    result = os.waitstatus_to_exitcode(status)
                if tid in processes:
                    row = processes[tid]
                    row['kernel_exit_status'] = status
                    row['tracer_wait_status'] = status
                continue
            # A newborn's initial stop can be reported before its creator's
            # fork event. Keep it stopped until creation has been recorded.
            if tid not in live:
                assert os.WSTOPSIG(status) == signal.SIGSTOP, (tid, status)
                pending_stops.add(tid)
                continue
            event = status >> 16
            sig = os.WSTOPSIG(status)
            deliver = 0
            restart = 7
            if event in (1, 2, 3):
                pid = message(tid)
                live.add(pid)
                # CLONE includes runtime threads; only new thread groups are
                # processes. The new tracee is stopped, so /proc is stable here.
                tgid = next(line.split()[1] for line in Path('/proc', str(pid), 'status').read_text().splitlines() if line.startswith('Tgid:'))
                if int(tgid) == pid:
                    processes[pid] = dict(pid=pid, creator_tid=tid)
                if pid in pending_stops:
                    pending_stops.remove(pid)
                    ptrace(7, pid)
            elif event == 4 and tid in processes:
                waits.pop(tid, None)
                processes[tid]['exec'] = Path('/proc', str(tid), 'cmdline').read_bytes().split(b'\0')[:-1]
                processes[tid]['exec'] = [arg.decode(errors='replace') for arg in processes[tid]['exec']]
                if hold_exec_until is not None:
                    remaining = hold_exec_until - time.monotonic()
                    assert remaining > 0, 'exec-stop control missed its deadline'
                    select.select([], [], [], remaining)
                    os.kill(tid, signal.SIGKILL)
            elif event == 6 and tid in processes:
                # EXIT is a pre-exit stop; a pending SIGKILL can still
                # change it. The terminal kernel wait is authoritative.
                processes[tid]['exit_stop_status'] = message(tid)
            elif event == 7 or sig == signal.SIGTRAP | 0x80:
                info = ctypes.create_string_buffer(128)
                ptrace(0x420e, tid, 128, ctypes.addressof(info))  # GET_SYSCALL_INFO
                op, arch = info.raw[0], struct.unpack_from('I', info.raw, 4)[0]
                assert arch == 0xc000003e, ('expected x86-64 syscall ABI', arch)
                if op in (1, 3):  # syscall entry or seccomp entry
                    restart = 24  # one syscall-exit stop for its exact result
                    nr, *args = struct.unpack_from('7Q', info.raw, 24)
                    waits.pop(tid, None)
                    if nr in (61, 247, 59, 322):  # wait4, waitid, execve, execveat
                        waits[tid] = (nr, args)
                    if nr in (59, 322) and tid in processes:
                        processes[tid]['exec_attempt'] = True
                elif op == 2 and tid in waits:
                    nr, args = waits.pop(tid)
                    returned = struct.unpack_from('q', info.raw, 24)[0]
                    if nr in (59, 322):
                        if returned < 0 and tid in processes:
                            processes[tid]['exec_error_errno'] = -returned
                        pid = 0
                    elif nr == 61 and returned > 0:
                        pid = returned
                        assert args[1], ('wait4 without status', tid, pid)
                        raw = word(tid, args[1]) & 0xffffffff
                    elif nr == 247 and returned == 0 and not args[3] & 0x1000000:  # !WNOWAIT
                        address = args[2]
                        pid = word(tid, address + 16) & 0xffffffff
                        code = word(tid, address + 8) & 0xffffffff
                        value = word(tid, address + 24) & 0xffffffff
                        raw = value << 8 if code == 1 else value | (0x80 if code == 3 else 0)
                    else:
                        pid = 0
                    if pid in processes:
                        processes[pid]['parent_wait_status'] = raw
                        processes[pid]['waiter_tid'] = tid
            elif sig not in (signal.SIGSTOP, signal.SIGTRAP):
                deliver = sig
            try:
                ptrace(restart, tid, data=deliver)
            except OSError as error:
                if error.errno != errno.ESRCH:
                    raise
        assert result is not None
        output.write_text(json.dumps(dict(root_pid=root, exit=result, children=list(processes.values())), indent=2))
        return result
    finally:
        live.update(pending_stops)
        for pid in live:
            try:
                os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        while live:
            try:
                pid, status = os.waitpid(-1, 0x40000000)
            except ChildProcessError:
                break
            if os.WIFSTOPPED(status):
                try:
                    ptrace(7, pid, data=signal.SIGKILL)
                except OSError:
                    pass
            else:
                live.discard(pid)


if __name__ == '__main__':
    sys.exit(trace(sys.argv[2:], Path(sys.argv[1])))
