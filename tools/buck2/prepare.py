#!/usr/bin/env python3
"""Make this checkout ready for Buck2: every pinned tool, the generated build
files, and the execution configuration.

Runs bootstrap.py, which also regenerates stale build files, then the optional
remote-execution hook: the executable named by `$HIRSEL_BUCK2_REMOTE_CONFIG`, or
`~/.config/hirsel/buck2-remote-config` when that exists, called with the
checkout's root. The hook writes the gitignored
`.buckconfig.local` for a private pool: its endpoint, credentials and the platform
properties its workers match (`hirsel.remote_properties`), rewriting the file only
when its content changes. Without a hook an existing `.buckconfig.local` is kept,
and without one every action runs locally (`hirsel.execution = local`).
"""
from pathlib import Path
import os
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
HERE = ROOT / 'tools/buck2'


def remote_hook():
    configured = os.environ.get('HIRSEL_BUCK2_REMOTE_CONFIG')
    if configured:
        return Path(configured)
    default = Path.home() / '.config/hirsel/buck2-remote-config'
    return default if default.is_file() else None


def main():
    subprocess.run([sys.executable, str(HERE / 'bootstrap.py')], check=True, cwd=ROOT,
                   stdout=subprocess.DEVNULL)
    hook = remote_hook()
    reason = 'no remote-execution hook'
    if hook:
        remote = subprocess.run([str(hook), str(ROOT)], cwd=ROOT, capture_output=True, text=True)
        if remote.returncode == 0:
            return 0
        reason = f'{hook}: {remote.stderr.strip()}'
    local = ROOT / '.buckconfig.local'
    if local.is_file():
        if hook:
            print(f'prepare: kept {local.name}; {reason}', file=sys.stderr)
        return 0
    local.write_text('# No remote-execution pool (tools/buck2/prepare.py): every action runs here.\n'
                     '[hirsel]\nexecution = local\n')
    print(f'prepare: every action runs locally ({reason})', file=sys.stderr)
    return 0


if __name__ == '__main__':
    sys.exit(main())
