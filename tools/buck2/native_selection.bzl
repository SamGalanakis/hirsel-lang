"""Derive the native program cover from the compiler and its complete corpus."""

load(":platforms.bzl", "budget_constraint", "pool_budget_name")

def _impl(ctx):
    tree = ctx.actions.copied_dir("corpus", {f.short_path: f for f in ctx.attrs.data})
    output = ctx.actions.declare_output("selection.json")
    llvm = ctx.attrs._llvm[DefaultInfo].default_outputs[0]
    ctx.actions.run(
        cmd_args(ctx.attrs.collector[RunInfo], "--root", tree, "--derive", output.as_output(), hidden = [llvm]),
        env = {"LD_LIBRARY_PATH": cmd_args(llvm.project("lib"))},
        category = "native_selection",
    )
    return [DefaultInfo(default_output = output)]

_selection = rule(
    impl = _impl,
    attrs = {
        "collector": attrs.exec_dep(providers = [RunInfo]),
        "data": attrs.list(attrs.source()),
        "labels": attrs.list(attrs.string(), default = []),
        "_llvm": attrs.exec_dep(default = "toolchains//:llvm"),
    },
)


def hirsel_native_selection(name, collector, data):
    # Gate-labelled targets are excluded from the ordinary workspace build.
    # Keep this build action available without enabling full test campaigns.
    native.configured_alias(name = name + "-collector", actual = collector, platform = "root//tools/buck2:release", labels = ["gate"])
    _selection(name = name, collector = ":" + name + "-collector", data = data,
               exec_compatible_with = [budget_constraint(pool_budget_name(1, 1048576))],
               labels = ["gate"], visibility = ["PUBLIC"])
