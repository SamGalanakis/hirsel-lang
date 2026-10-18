#!/usr/bin/env python3
"""Install the pinned Buck2 prelude at `.buck2/prelude`, with Hirsel's overlay.

The transforms make wrapper execution self-contained and take helper work off the pool:

- command wrappers use the image's `/bin/bash` without looking it up in `PATH`;
- `failure_filter` is decided in the daemon from the compile's build status;
- each compile's `-Ldependency` tree (category `deps`) is laid out by the
  daemon instead of a remote action;
- compiler and rustdoc actions stop carrying extracted third-party sources
  that rustc can never read.

`pins.json` pins every patched file twice: its stock bytes (`inputs`) and the
overlay's output (`outputs`). A prelude pin change fails on an input checksum,
a stock text the transform no longer finds fails on its anchor, and a
transform edited without re-pinning fails on an output checksum. Nothing is
written from a file the pins do not name. After a deliberate prelude or
transform change, `--print-pins` prints the new pins from a stock prelude.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import stat
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
PINS = pathlib.Path(__file__).with_name("pins.json")


class OverlayError(Exception):
    pass


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def replace_once(text: str, old: str, new: str, what: str) -> str:
    if text.count(old) != 1:
        raise OverlayError(f"stock {what} not found exactly once; the pinned prelude changed")
    return text.replace(old, new, 1)


# Reindeer's generated package (third-party/reindeer.toml). Its crates compile
# from extracted `<name>-<version>.crate` archive directories.
THIRD_PARTY_PACKAGE = "third-party"

STOCK_SOURCES_TSET = '''
RustSourcesTSet = transitive_set(
    args_projections = {
        "artifacts": _get_artifacts,
    },
)
'''

# rust-lang/rust#153898 lets a dependency's source text change the crate hash
# of a dependent, so the stock prelude ships every transitive source tree to
# every compile. Each compile remaps its `__srcs` root to its owning package
# path, and dependency metadata keeps only that remapped name, so rustc can
# find dependency text only at `<package>/<file>` relative to the action root.
# First-party sources are there, locally and remotely, and stay inputs.
# Extracted third-party archives never are, so shipping them cannot change a
# hash and only multiplied each archive's files into every input root.
CHECKOUT_SOURCES_TSET = '''
# Hirsel overlay (tools/buck2/prelude_overlay.py): third-party archives cannot
# be read through a remapped path, so compiles do not carry them.
def _get_checkout_artifacts(sources: Artifact) -> list[Artifact]:
    owner = sources.owner
    if owner != None and owner.package == "{package}":
        return []
    return [sources]

RustSourcesTSet = transitive_set(
    args_projections = {{
        "artifacts": _get_artifacts,
        "checkout_artifacts": _get_checkout_artifacts,
    }},
)
'''.format(package=THIRD_PARTY_PACKAGE)


def add_checkout_source_projection(text: str) -> str:
    return replace_once(text, STOCK_SOURCES_TSET, CHECKOUT_SOURCES_TSET, "Rust sources transitive set")


def narrow_transitive_source_inputs(text: str) -> str:
    # Compiler and rustdoc actions take the checkout projection. The provider
    # and its full `artifacts` projection stay stock for every other user.
    for old, new, what in (
        (
            'hidden = compile_ctx.transitive_srcs.project_as_args("artifacts") if compile_ctx else [],',
            'hidden = compile_ctx.transitive_srcs.project_as_args("checkout_artifacts") if compile_ctx else [],',
            "compiler transitive source inputs",
        ),
        (
            'hidden = [toolchain_info.compiler, compile_ctx.transitive_srcs.project_as_args("artifacts")],',
            'hidden = [toolchain_info.compiler, compile_ctx.transitive_srcs.project_as_args("checkout_artifacts")],',
            "rustdoc transitive source inputs",
        ),
    ):
        text = replace_once(text, old, new, what)
    return text


# `failure_filter` re-exposes one rustc output once the build status lists it,
# and otherwise replays rustc's diagnostics and fails with rustc's status. The
# stock rule spends a remote action on that decision after every compile whose
# diagnostics must not fail dependents: a second serial round trip, and a
# queue slot of the target's whole compile request, for a hard link. The
# daemon takes the decision from the build status instead: the passing case is
# a declared copy that executes nothing and fetches only the build status, and
# the failing case runs the stock action unchanged, so a compile error reads
# as before.
STOCK_FAILURE_FILTER = '''    cmd = cmd_args(
        failure_filter_action,
        "--stderr",
        stderr,
        "--required-file",
        required.short_path,
        required,
        output.as_output(),
        "--build-status",
        build_status,
    )

    toolchain_info = compile_ctx.toolchain_info
    ctx.actions.run(
        cmd,
        category = "failure_filter",
        identifier = identifier,
        error_handler = toolchain_info.rust_error_handler,
    )

    return output
'''

DAEMON_FAILURE_FILTER = '''    toolchain_info = compile_ctx.toolchain_info

    # Hirsel overlay (tools/buck2/prelude_overlay.py): decided in the daemon;
    # only a failed compile runs the stock action, which replays rustc's
    # diagnostics and status.
    def filter_from_build_status(ctx, artifacts, outputs):
        if required.short_path in artifacts[build_status].read_json()["files"]:
            ctx.actions.copy_file(outputs[output].as_output(), required)
            return
        ctx.actions.run(
            cmd_args(
                failure_filter_action,
                "--stderr",
                stderr,
                "--required-file",
                required.short_path,
                required,
                outputs[output].as_output(),
                "--build-status",
                build_status,
            ),
            category = "failure_filter",
            identifier = identifier,
            error_handler = toolchain_info.rust_error_handler,
        )

    ctx.actions.dynamic_output(
        dynamic = [build_status],
        inputs = [],
        outputs = [output.as_output()],
        f = filter_from_build_status,
    )

    return output
'''


def filter_failures_in_daemon(text: str) -> str:
    return replace_once(text, STOCK_FAILURE_FILTER, DAEMON_FAILURE_FILTER, "failure filter action")


# The prelude lays out each compile's `-Ldependency` directory with a remote
# action (category `deps`): a Python tool that symlinks every transitive
# dependency under its file name. The tool is needed only for `crate_dynamic`,
# whose crate name is known at build time, and no Hirsel crate uses it. As a
# remote action it ran once per compile variant, each reserving the whole
# compile request of its target, because Buck2 gives a target one execution
# platform. The daemon builds the same tree with `ctx.actions.assembled_dir`:
# the same `<n>/<file>` relative symlinks in the same name-collision order,
# and the same `dirs` flag file as a regular file. A consumer's input tree is
# therefore unchanged, and so are its action digest and cache entry.
STOCK_DEPENDENCY_DIRS = """    artifacts = transitive_deps.project_as_json("artifacts")

    # Pass the list of rlibs to transitive_dependency_symlinks.py through a file
    # because there can be a lot of them. This avoids running out of command
    # line length, particularly on Windows.
    artifacts_json = ctx.actions.write_json(
        "{}-symlinked_dirs.json".format(prefix),
        artifacts,
        pretty = True,
        has_content_based_path = getattr(ctx.attrs, "use_content_based_paths", False),
    )

    arguments = [
        internal_tools_info.transitive_dependency_symlinks_tool,
        cmd_args(ctx.label.name, format = "--name={}"),
        cmd_args(transitive_dependency_dir.as_output(), format = "--out-dir={}"),
        cmd_args(
            artifacts_json,
            format = "--artifacts={}",
            # Don't take a dependency on all the artifacts in here, just the dynamic names; the
            # rmetas/rlibs we only want to create symlinks to, so there's no need for them to
            # actually be available
            hidden = transitive_deps.project_as_args("dynamic_name_args"),
        ),
    ]

    if cwd:
        arguments.append(
            cmd_args(
                transitive_dependency_dir.as_output(),
                format = "--out-dir-relative-to-cwd={}",
                relative_to = cwd,
            )
        )

    ctx.actions.run(
        arguments,
        category = "deps",
        identifier = str(len(transitive_dependency_dirs)),
    )
"""

DAEMON_DEPENDENCY_DIRS = """    # Hirsel overlay (tools/buck2/prelude_overlay.py): laid out by the daemon,
    # not a remote `deps` action. This mirrors transitive_dependency_symlinks.py:
    # the json projection's order, a new numbered directory whenever a file
    # name repeats, relative symlinks, and a `dirs` file of one
    # `-Ldependency=` line per directory.
    deps_dirs = [{}]
    for dep in transitive_deps.traverse():
        if dep.crate.dynamic:
            fail("Hirsel lays out dependency directories in the daemon and supports no crate_dynamic: {}".format(ctx.label))
        filename = dep.artifact.basename
        if filename in deps_dirs[-1]:
            deps_dirs.append({})
        deps_dirs[-1][filename] = dep.artifact

    contents = {}
    flags = []
    for index, srcs in enumerate(deps_dirs):
        if not srcs:
            continue
        for filename, artifact in srcs.items():
            contents["{}/{}".format(index, filename)] = assembled_dir.symlink(artifact)
        flags.append(cmd_args(
            transitive_dependency_dir,
            format = "-Ldependency={}/" + str(index),
            relative_to = cwd,
            ignore_artifacts = True,
        ) if cwd else cmd_args(
            transitive_dependency_dir,
            format = "-Ldependency={}/" + str(index),
            ignore_artifacts = True,
        ))

    # The tool ends every line with a newline; `write` only separates them.
    contents["dirs"] = assembled_dir.copy(ctx.actions.write(
        "{}-symlinked_dirs.flags".format(prefix),
        flags + [""] if flags else "",
    ))
    ctx.actions.assembled_dir(transitive_dependency_dir, contents = contents)
"""


def lay_out_dependency_dirs_in_daemon(text: str) -> str:
    return replace_once(text, STOCK_DEPENDENCY_DIRS, DAEMON_DEPENDENCY_DIRS, "dependency directory action")


def use_absolute_wrapper_interpreter(text: str) -> str:
    # Rustdoc runs linker wrappers without the compiler action's PATH.
    return replace_once(text, '"#!/usr/bin/env bash"', '"#!/bin/bash"', "command wrapper interpreter")


def cache_doctest_verdicts(text: str) -> str:
    # Like the libtest provider in defs.bzl, a rustdoc test is hermetic and
    # uses repository-relative inputs. Failures remain uncached.
    return replace_once(
        text,
        '            type = "rustdoc",\n',
        '            type = "rustdoc",\n            supports_test_execution_caching = True,\n            env = {"TEST_TARGET": "run:-:" + str(sources.owner.raw_target())},\n',
        "rustdoc test caching",
    )


def explicit_test_executor_policy(text: str) -> str:
    text = replace_once(
        text,
        '    local_enabled = re_props_copy.pop("local_enabled", False)\n',
        '    local_enabled = re_props_copy.pop("local_enabled", False)\n'
        '    remote_enabled = re_props_copy.pop("remote_enabled", True)\n'
        '    priority = re_props_copy.pop("priority", 0)\n',
        "explicit test executor policy",
    )
    for spaces in (8, 12):
        indent = ' ' * spaces
        text = replace_once(text, '\n' + indent + 'remote_enabled = True,\n',
                            '\n' + indent + 'remote_enabled = remote_enabled,\n' + indent + 'priority = priority,\n',
                            f"test executor at indentation {spaces}")
    return text


TRANSFORMS = {
    "rust/build.bzl": (narrow_transitive_source_inputs, lay_out_dependency_dirs_in_daemon),
    "rust/failure_filter.bzl": (filter_failures_in_daemon,),
    "rust/sources.bzl": (add_checkout_source_projection,),
    "rust/rust_library.bzl": (cache_doctest_verdicts,),
    "tests/re_utils.bzl": (explicit_test_executor_policy,),
    "utils/cmd_script.bzl": (use_absolute_wrapper_interpreter,),
}


def transform(relative: str, text: str) -> str:
    for step in TRANSFORMS[relative]:
        text = step(text)
    return text


def load_pins(path: pathlib.Path = PINS) -> dict:
    pins = json.loads(path.read_text())["prelude_overlay"]
    for kind in ("inputs", "outputs"):
        if set(pins[kind]) != set(TRANSFORMS):
            raise OverlayError(f"pins.json prelude_overlay.{kind} must name exactly {sorted(TRANSFORMS)}")
    return pins


def overlaid(prelude: pathlib.Path, pins: dict) -> dict[str, bytes]:
    """The overlay's bytes for every patched file, from stock or already-overlaid files."""
    outputs = {}
    for relative in TRANSFORMS:
        data = (prelude / relative).read_bytes()
        actual = digest(data)
        if actual != pins["outputs"][relative]:
            if actual != pins["inputs"][relative]:
                raise OverlayError(f"pinned prelude input changed: {relative}: {actual}")
            data = transform(relative, data.decode()).encode()
            if digest(data) != pins["outputs"][relative]:
                raise OverlayError(f"overlay output does not match pins.json: {relative}: {digest(data)}")
        outputs[relative] = data
    return outputs


def replace(path: pathlib.Path, data: bytes) -> None:
    """Write a new file over `path`; a prelude file may be a link shared with other checkouts."""
    temporary = path.with_name(path.name + ".tmp")
    temporary.write_bytes(data)
    # Buck2 hashes the executable bit of a prelude file with its bytes.
    temporary.chmod(stat.S_IMODE(path.stat().st_mode) | 0o200)
    temporary.replace(path)


def expand(buck2: pathlib.Path, prelude: pathlib.Path) -> None:
    """Expand the prelude bundled in the pinned Buck2 executable at its cell path."""
    config = ROOT / ".buckconfig"
    original = config.read_text()
    bundled = original.replace("prelude = disabled", "prelude = bundled", 1)
    if bundled == original:
        raise OverlayError(".buckconfig does not set [external_cells] prelude = disabled")
    bootstrap = [str(buck2.resolve()), "--isolation-dir", "buck2-bootstrap"]
    try:
        config.write_text(bundled)
        subprocess.run(bootstrap + ["expand-external-cell", "prelude"], cwd=ROOT, check=True,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    finally:
        config.write_text(original)
        # Expansion starts its own daemon, which has no use afterwards.
        subprocess.run(bootstrap + ["kill"], cwd=ROOT, check=False,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    if not prelude.is_dir():
        raise OverlayError(f"Buck2 did not expand the pinned prelude at {prelude}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--buck2", type=pathlib.Path, default=ROOT / ".buck2/bin/buck2")
    parser.add_argument("--prelude-dir", type=pathlib.Path, default=ROOT / ".buck2/prelude")
    parser.add_argument("--check", action="store_true", help="Verify the installed overlay; change nothing.")
    parser.add_argument("--print-pins", action="store_true",
                        help="Print pins.json's prelude_overlay from a stock prelude directory, without checking it.")
    args = parser.parse_args()
    try:
        if args.print_pins:
            inputs = {relative: (args.prelude_dir / relative).read_bytes() for relative in TRANSFORMS}
            pins = {
                "inputs": {relative: digest(data) for relative, data in inputs.items()},
                "outputs": {relative: digest(transform(relative, data.decode()).encode()) for relative, data in inputs.items()},
            }
            print(json.dumps(pins, indent=2, sort_keys=True))
            return 0
        pins = load_pins()
        if not args.prelude_dir.is_dir():
            if args.check:
                print(f"prelude overlay: {args.prelude_dir} is absent", file=sys.stderr)
                return 1
            expand(args.buck2, args.prelude_dir)
        outputs = overlaid(args.prelude_dir, pins)
        stale = [relative for relative, data in outputs.items() if (args.prelude_dir / relative).read_bytes() != data]
        if args.check and stale:
            print("prelude overlay is not applied: " + ", ".join(stale), file=sys.stderr)
            return 1
        if not args.check:
            for relative in stale:
                replace(args.prelude_dir / relative, outputs[relative])
    except OverlayError as error:
        print(f"prelude overlay: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
