"""An on-demand native stress action whose scratch survives remote execution."""
load(":platforms.bzl", "budget_constraint")

def _impl(ctx):
    corpus = ctx.actions.copied_dir("corpus", {f.short_path: f for f in ctx.attrs.data})
    output = ctx.actions.declare_output("diagnostics", dir = True)
    command = cmd_args(
        "python3", ctx.attrs.script, "--native", ctx.attrs.binary[DefaultInfo].default_outputs[0],
        "--root", corpus, "--output", output.as_output(), "--repetitions", ctx.attrs.repetitions,
        "--shard", ctx.attrs.shard,
    )
    for name, tool in ctx.attrs.tools.items():
        command.add("--tool", name, tool[DefaultInfo].default_outputs[0])
    # This diagnostic action records the campaign's exit code in result.json;
    # it must publish that evidence even when the campaign fails.
    ctx.actions.run(command, category = "native_timeouts")
    return [DefaultInfo(default_output = output)]

_diagnostics = rule(impl = _impl, attrs = {
    "binary": attrs.exec_dep(),
    "data": attrs.list(attrs.source()),
    "script": attrs.source(),
    "tools": attrs.dict(attrs.string(), attrs.exec_dep()),
    "repetitions": attrs.string(),
    "shard": attrs.string(),
    "labels": attrs.list(attrs.string(), default = []),
})

def hirsel_native_timeouts(name, data):
    native.configured_alias(
        name = name + "-binary", actual = "//:hirsel-conformance-bin-native",
        platform = "root//tools/buck2:checked", labels = ["gate"],
    )
    _diagnostics(
        name = name, binary = ":" + name + "-binary", data = data,
        script = "//tools/buck2:native_timeouts.py",
        tools = {
            "llvm": "//tools/buck2/native-toolchain:llvm",
            "kit": "//tools/buck2/native-toolchain:kit",
            "glibc_floor": "//tools/buck2/native-toolchain:glibc_floor",
            "archives": "//:hirsel-rt-archives",
            "precheck": "//:hirsel-std-precheck",
        },
        repetitions = read_root_config("hirsel", "native_timeout_repetitions", "8"),
        shard = read_root_config("hirsel", "native_timeout_shard", "1"),
        exec_compatible_with = [budget_constraint("pool_test_2_2883584")],
        labels = ["gate"], visibility = ["PUBLIC"],
    )
