"""The native program cover: the checked-in frozen file, or derived on demand.

Deriving observes every corpus program through 14 native builds, which costs
more than the slim campaigns it serves, so campaigns read
native-tests/regressions/native-selection.json and `tools/bless.sh
native-selection` regenerates it (`-c hirsel.native_selection=derive`).
"""

FROZEN = "native-tests/regressions/native-selection.json"

# Each observation now owns a process, releasing compiler state between cases.
# Keep the existing 256 stripes and 4 GiB request for the cost of each program's
# 14 native builds; isolation does not bound an individual program's cost.
OBSERVATION_SHARDS = 256
OBSERVATION_REQUEST = (1, 4194304)  # sync.py adds it to POOL_BUDGETS

load(":platforms.bzl", "budget_constraint", "pool_budget_name")

def _impl(ctx):
    tree = ctx.actions.copied_dir("corpus", {f.short_path: f for f in ctx.attrs.data})
    output = ctx.actions.declare_output("selection.json")
    llvm = ctx.attrs._llvm[DefaultInfo].default_outputs[0]
    inventory = ctx.actions.declare_output("inventory.json")
    ctx.actions.run(
        cmd_args(ctx.attrs.collector[RunInfo], "--root", tree, "--inventory-output", inventory.as_output(), hidden = [llvm]),
        env = {"LD_LIBRARY_PATH": cmd_args(llvm.project("lib"))}, category = "native_inventory",
    )
    observations = []
    parts = {"inventory": [DefaultInfo(default_output = inventory)]}
    precheck = ctx.attrs.precheck[DefaultInfo].default_outputs[0]
    for index in range(1, OBSERVATION_SHARDS + 1):
        part = ctx.actions.declare_output("observations-{}.json".format(index))
        ctx.actions.run(
            cmd_args(ctx.attrs.collector[RunInfo], "--root", tree, "--derive", part.as_output(), "--shard", "{}/{}".format(index, OBSERVATION_SHARDS), "--inventory-input", inventory, hidden = [llvm, precheck]),
            env = {"LD_LIBRARY_PATH": cmd_args(llvm.project("lib")), "HIRSEL_PRECHECK": cmd_args(precheck)},
            category = "native_observation", identifier = str(index),
        )
        observations.append(part)
        parts["observations-{}".format(index)] = [DefaultInfo(default_output = part)]
    merge = cmd_args(ctx.attrs.collector[RunInfo], "--root", tree, "--derive", output.as_output(), hidden = [llvm])
    for observation in observations:
        merge.add("--merge", observation)
    ctx.actions.run(merge, env = {"LD_LIBRARY_PATH": cmd_args(llvm.project("lib"))}, category = "native_selection")
    return [DefaultInfo(default_output = output, sub_targets = parts)]

_selection = rule(
    impl = _impl,
    attrs = {
        "collector": attrs.exec_dep(providers = [RunInfo]),
        "data": attrs.list(attrs.source()),
        "labels": attrs.list(attrs.string(), default = []),
        "precheck": attrs.dep(),
        "_llvm": attrs.exec_dep(default = "toolchains//:llvm"),
    },
)


def hirsel_native_selection(name, collector, data, precheck):
    # Fault replay passes its own frozen cover (hirsel.native_selection_file).
    frozen = read_root_config("hirsel", "native_selection_file", FROZEN)
    if read_root_config("hirsel", "native_selection", "frozen") != "derive":
        native.export_file(name = name, src = frozen, labels = ["gate"], visibility = ["PUBLIC"])
        return
    # Gate-labelled targets are excluded from the ordinary workspace build.
    native.configured_alias(name = name + "-collector", actual = collector, platform = "root//tools/buck2:checked", labels = ["gate"])
    _selection(name = name, collector = ":" + name + "-collector", data = data, precheck = precheck,
               exec_compatible_with = [budget_constraint(pool_budget_name(*OBSERVATION_REQUEST))],
               labels = ["gate"], visibility = ["PUBLIC"])
