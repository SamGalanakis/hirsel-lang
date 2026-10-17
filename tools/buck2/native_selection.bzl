"""Derive the native program cover from the compiler and its complete corpus."""

load(":platforms.bzl", "budget_constraint", "pool_budget_name")

def _impl(ctx):
    tree = ctx.actions.copied_dir("corpus", {f.short_path: f for f in ctx.attrs.data})
    output = ctx.actions.declare_output("selection.json")
    llvm = ctx.attrs._llvm[DefaultInfo].default_outputs[0]
    observations = []
    for index in range(1, 9):
        part = ctx.actions.declare_output("observations-{}.json".format(index))
        ctx.actions.run(
            cmd_args(ctx.attrs.collector[RunInfo], "--root", tree, "--derive", part.as_output(), "--shard", "{}/8".format(index), hidden = [llvm]),
            env = {"LD_LIBRARY_PATH": cmd_args(llvm.project("lib"))},
            category = "native_observation", identifier = str(index),
        )
        observations.append(part)
    merge = cmd_args(ctx.attrs.collector[RunInfo], "--root", tree, "--derive", output.as_output(), hidden = [llvm])
    for observation in observations:
        merge.add("--merge", observation)
    ctx.actions.run(merge, env = {"LD_LIBRARY_PATH": cmd_args(llvm.project("lib"))}, category = "native_selection")
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
    # Fault replay must freeze the healthy cover instead of deriving it from
    # a deliberately broken compiler. The override is still a declared input.
    frozen = read_root_config("hirsel", "native_selection_file", "")
    if frozen:
        native.export_file(name = name, src = frozen, labels = ["gate"], visibility = ["PUBLIC"])
        return
    # Gate-labelled targets are excluded from the ordinary workspace build.
    # Keep this build action available without enabling full test campaigns.
    native.configured_alias(name = name + "-collector", actual = collector, platform = "root//tools/buck2:release", labels = ["gate"])
    _selection(name = name, collector = ":" + name + "-collector", data = data,
               exec_compatible_with = [budget_constraint(pool_budget_name(1, 1048576))],
               labels = ["gate"], visibility = ["PUBLIC"])
