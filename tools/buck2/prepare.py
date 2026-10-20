#!/usr/bin/env python3
"""Make this checkout ready for Buck2: every pinned tool, the generated build
files, and the execution configuration.

    prepare.py           the remote-execution pool, or fail saying why
    prepare.py --local   every action on this host (`hirsel.execution = local`)

Runs bootstrap.py, which also regenerates stale build files, then the
remote-execution hook: the executable named by `$HIRSEL_BUCK2_REMOTE_CONFIG`, or
`~/.config/hirsel/buck2-remote-config` when that exists, called with the
checkout's root. The hook writes the gitignored `.buckconfig.local` for a
private pool: its endpoint, credentials and the platform properties its workers
match (`hirsel.remote_properties`), rewriting the file only when its content
changes. When the hook is missing or fails, an existing remote
`.buckconfig.local` is kept; with no remote configuration at all, prepare.py
fails. Running everything on this host is only ever chosen with `--local`: an
unattended caller (tools/land, tools/main-runs) that silently fell back to it
would redo every compile, test and toolchain unpack here.
"""
from pathlib import Path
import os
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
HERE = ROOT / 'tools/buck2'
LOCAL_CONFIG = ('# Every action runs on this host (tools/buck2/prepare.py --local).\n'
                '[hirsel]\nexecution = local\n')


def remote_hook():
    configured = os.environ.get('HIRSEL_BUCK2_REMOTE_CONFIG')
    if configured:
        return Path(configured)
    default = Path.home() / '.config/hirsel/buck2-remote-config'
    return default if default.is_file() else None


def is_remote(config):
    """Whether a `.buckconfig.local` configures a remote-execution client."""
    if not config.is_file():
        return False
    text = config.read_text()
    return '[buck2_re_client]' in text and 'execution = local' not in text


def main():
    arguments = sys.argv[1:]
    if arguments not in ([], ['--local']):
        print(__doc__, file=sys.stderr)
        return 2
    subprocess.run([sys.executable, str(HERE / 'bootstrap.py')], check=True, cwd=ROOT,
                   stdout=subprocess.DEVNULL)
    local = ROOT / '.buckconfig.local'
    if arguments == ['--local']:
        if not local.is_file() or local.read_text() != LOCAL_CONFIG:
            local.write_text(LOCAL_CONFIG)
        print('prepare: every action runs on this host (--local)', file=sys.stderr)
        return 0
    hook = remote_hook()
    reason = ('no remote-execution hook ($HIRSEL_BUCK2_REMOTE_CONFIG or '
              '~/.config/hirsel/buck2-remote-config)')
    if hook:
        remote = subprocess.run([str(hook), str(ROOT)], cwd=ROOT, capture_output=True, text=True)
        if remote.returncode == 0:
            return 0
        reason = f'{hook} exited {remote.returncode}' + (f': {remote.stderr.strip()}' if remote.stderr.strip() else '')
    if is_remote(local):
        print(f'prepare: kept {local.name}; {reason}', file=sys.stderr)
        return 0
    print(f'prepare: no remote-execution configuration: {reason}.\n'
          f'Install the hook, or run `{Path(sys.argv[0]).name} --local` to run every action on this host.',
          file=sys.stderr)
    return 1


if __name__ == '__main__':
    sys.exit(main())
