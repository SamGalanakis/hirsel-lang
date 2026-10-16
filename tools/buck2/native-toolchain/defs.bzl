"""Pinned downloads become relocatable tool directories; only unpacking uses the host."""
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
