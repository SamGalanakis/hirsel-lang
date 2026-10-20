"""Pinned downloads become relocatable tool directories.

Unpacking and the link kit run on the pool like any compile (the default
platform, platforms.bzl), so the remote action cache shares their outputs
between checkouts instead of every worktree redoing them on this host. They
need only the worker image's python3, dpkg-deb and readelf (the kiln root
filesystem the runtime fingerprint pins) and the pinned LLVM tree. The
downloads themselves are deferred: with a SHA-256 the daemon fetches an archive
only when the pool's CAS lacks it, and not at all on an action-cache hit
(buck2 app/buck2_action_impl/src/actions/impls/download_file.rs `declare_http`,
app/buck2_execute/src/re/uploader.rs `find_missing`)."""
load("//tools/buck2:platforms.bzl", "budget_constraint", "test_budget_name", "test_executor", "test_request")

def _unpack(ctx):
    output = ctx.actions.declare_output(ctx.label.name, dir = True)
    ctx.actions.run(
        cmd_args("python3", ctx.attrs._unpack, ctx.attrs.kind, output.as_output(), [dep[DefaultInfo].default_outputs[0] for dep in ctx.attrs.archives]),
        category = "native_toolchain_unpack",
    )
    return [DefaultInfo(default_output = output)]

unpack_toolchain = rule(impl = _unpack, attrs = {
    "archives": attrs.list(attrs.dep()),
    "kind": attrs.string(),
    "_unpack": attrs.source(default = "//tools/buck2/native-toolchain:unpack.py"),
})

def _link_kit(ctx):
    output = ctx.actions.declare_output(ctx.label.name, dir = True)
    pins = ctx.actions.write_json("pins.json", ctx.attrs.pins)
    source = lambda name: ctx.attrs.sources[name][DefaultInfo].default_outputs[0]
    command = cmd_args(
        "python3", ctx.attrs._script,
        "--llvm", ctx.attrs.llvm[DefaultInfo].default_outputs[0],
        "--abilists", source("abilists"),
        "--libunwind", source("libunwind"),
        "--compiler-rt", source("compiler-rt"),
        "--pins", pins,
        "--output", output.as_output(),
    )
    for name in sorted(ctx.attrs.sources):
        if ":" in name:
            command.add("--deb", cmd_args(name, "=", source(name), delimiter = ""))
    ctx.actions.run(command, category = "link_kit")
    return [DefaultInfo(default_output = output)]

# The per-target link kit (link_kit.py): built once from pinned sources and
# hashed into every executable's build identity.
link_kit = rule(impl = _link_kit, attrs = {
    "llvm": attrs.dep(),
    "sources": attrs.dict(attrs.string(), attrs.dep()),
    "pins": attrs.dict(attrs.string(), attrs.dict(attrs.string(), attrs.any())),
    "_script": attrs.source(default = "//tools/buck2/native-toolchain:link_kit.py"),
})

def _proof(ctx):
    trees = [dep[DefaultInfo].default_outputs[0] for dep in ctx.attrs.tools]
    command = cmd_args("bash", ctx.attrs.script, trees)
    return [DefaultInfo(), ExternalRunnerTestInfo(
        type = "custom", command = [command], env = {}, labels = ["gate"],
        run_from_project_root = True, use_project_relative_paths = True,
        supports_test_execution_caching = True,
        default_executor = test_executor(int(ctx.attrs.request[0]), int(ctx.attrs.request[1]), ctx.attrs.request[2]),
    )]

_toolchain_proof = rule(impl = _proof, attrs = {
    "tools": attrs.list(attrs.dep()), "script": attrs.source(),
    "request": attrs.list(attrs.string()),
})

def toolchain_proof(name, tools, script):
    _toolchain_proof(name = name, tools = tools, script = script,
        request = [str(value) for value in test_request("native-toolchain-proof")],
        exec_compatible_with = [budget_constraint(test_budget_name("native-toolchain-proof"))],
        visibility = ["PUBLIC"])
