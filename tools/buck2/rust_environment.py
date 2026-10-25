"""Find host Cargo and Rust without requiring a shell's rustup PATH setup.

Buck2's prepared compiler tree contains rustc but no Cargo. Generation also
needs Cargo, from the pinned rustup toolchain or the user's Cargo home.
"""
import os
from pathlib import Path
import shutil
import subprocess
import tomllib


def environment(root, validate=False):
    env = dict(os.environ)
    home = Path.home()
    cargo_home = Path(env.get('CARGO_HOME', home / '.cargo')).resolve()
    rustup_home = Path(env.get('RUSTUP_HOME', home / '.rustup')).resolve()
    directories = [root / 'tools/buck2/toolchains/rust-files/bin']
    pin = root / 'rust-toolchain.toml'
    if pin.is_file():
        channel = tomllib.loads(pin.read_text())['toolchain']['channel']
        directories += [rustup_home / 'toolchains' / channel / 'bin']
        directories += sorted((rustup_home / 'toolchains').glob(f'{channel}-*/bin'))
    directories.append(cargo_home / 'bin')
    # Keep other host tools/hooks available, but search known Rust homes first.
    path = os.pathsep.join(map(str, directories)) + os.pathsep + env.get('PATH', os.defpath)
    cargo = shutil.which(env.get('CARGO', 'cargo'), path=path)
    rustc = shutil.which('rustc', path=path)
    if not cargo or not rustc:
        raise ValueError('no Rust toolchain found; install Cargo and rustc in CARGO_HOME or the pinned rustup toolchain')
    env.update(PATH=path, CARGO=os.path.abspath(cargo))
    if validate:
        for executable in (cargo, rustc):
            try:
                probe = subprocess.run([executable, '--version'], cwd=root, env=env,
                                       capture_output=True, timeout=30)
            except (OSError, subprocess.TimeoutExpired) as error:
                raise ValueError(f'Rust toolchain unavailable: {executable} --version failed') from error
            if probe.returncode:
                raise ValueError(f'Rust toolchain unavailable: {executable} --version failed')
    return env
