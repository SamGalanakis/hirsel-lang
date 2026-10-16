"""Execution platforms: one per remote pool budget, plus the invoking host.

`-c hirsel.execution=local` (no `.buckconfig.local` needed) runs every action
on this host instead. Otherwise actions go to the remote-execution pool that
`.buckconfig.local` configures; its scheduler matches the platform properties
below, plus the pool's own `hirsel.remote_properties`, and reserves the
requested CPUs and memory for the action.

A budget is (CPU, memory, CPU class). Only the critical-path compiles named in
CPU_CLASSES (tools/buck2/action_sizes_from_log.py) and the conformance shards
(TEST_REQUESTS) ask for `cpu_class=fast`; every other compile, test run and gate
shard runs on any worker.

A pool that serves its queue highest priority first without preempting
(NativeLink) runs an edit's compiles, queued at 100, before test runs and
campaign shards, which queue at 0.
The priority is not a platform property, so it is not part of an action key.
"""

load(":exec_sizes.bzl", "COMPILE_CLASSES", "COMPILE_REQUESTS", "POOL_BUDGETS")
load(":test_sizes.bzl", "TEST_REQUESTS")

COMPILE_PRIORITY = 100
TEST_PRIORITY = 0

# A pool test binary's request, `[cpu, memory_kb, cpu_class]`, is measured
# from the workers' usage logs (TEST_REQUESTS, test_sizes_from_usage.py): one
# CPU for a binary that uses about one core, more only where it measured more.
# The test launcher runs as many test threads as the request's whole CPUs.
# A binary with no measurement yet runs as a single-threaded one with room.
DEFAULT_TEST_REQUEST = [1, 2097152, ""]

def _test_budget(cpu, memory_kb, cpu_class):
    return "pool_test_" + pool_budget_name(cpu, memory_kb, cpu_class).removeprefix("pool_")

def test_budget_name(binary):
    return _test_budget(*TEST_REQUESTS.get(binary, DEFAULT_TEST_REQUEST))

# The request of a compile no measurement sized.
DEFAULT_REQUEST = [1, 1048576]

# Every Clippy twin's request (defs.bzl): Clippy is a single-threaded front end.
CLIPPY_REQUEST = [1, 524288, ""]

# `-c hirsel.probe_cpus=N` gives every CPU_CLASSES compile N CPUs (and N
# codegen units, defs.bzl) for one invocation: the calibration that a compile
# measured under one CPU cannot provide (action_sizes_from_log.py).
PROBE_CPUS = int(read_root_config("hirsel", "probe_cpus", "0"))
PROBE_MEMORY_KB = 4194304
if PROBE_CPUS < 0 or PROBE_CPUS > 8:
    fail("hirsel.probe_cpus must be from 1 to 8, or 0 for the measured requests")

def pool_budget_name(cpu, memory_kb, cpu_class = ""):
    name = "pool_{}_{}".format(cpu, memory_kb)
    return name + "_" + cpu_class if cpu_class else name

def compile_request(profile, name):
    """A compile's `[cpu, memory_kb, cpu_class]` in `profile` (dev or release)."""
    cpu, memory_kb = COMPILE_REQUESTS[profile].get(name, DEFAULT_REQUEST)
    cpu_class = COMPILE_CLASSES[profile].get(name, "")
    if PROBE_CPUS and cpu_class:
        cpu, memory_kb = PROBE_CPUS, max(memory_kb, PROBE_MEMORY_KB)
    return [cpu, memory_kb, cpu_class]

LOCAL = read_root_config("hirsel", "execution", "remote") == "local"

def budget_constraint(name):
    return "root//tools/buck2:" + name

# The pool's own platform properties, `key=value` pairs separated by commas, from
# the gitignored `.buckconfig.local` (tools/buck2/prepare.py).
REMOTE_PROPERTIES = read_root_config("hirsel", "remote_properties", "")

def pool_properties(cpu, memory_kb, cpu_class = ""):
    properties = {
        "cpu_count": str(cpu),
        "memory_kb": str(memory_kb),
        "cpu_arch": "x86_64",
        "OSFamily": "linux",
    }
    for pair in REMOTE_PROPERTIES.split(","):
        if pair.strip():
            key, sep, value = pair.partition("=")
            if not sep:
                fail("hirsel.remote_properties must be key=value pairs: " + REMOTE_PROPERTIES)
            properties[key.strip()] = value.strip()
    if cpu_class:
        properties["cpu_class"] = cpu_class
    return properties

def _platforms(ctx):
    base = {
        key: value
        for dep in ctx.attrs.constraints
        for key, value in dep[ConfigurationInfo].constraints.items()
    }
    platforms = []
    for spec, budget in ctx.attrs.budgets.items():
        name, cpu, memory_kb, cpu_class = spec.split("|")
        platforms.append(ExecutionPlatformInfo(
            label = budget.label.raw_target(),
            configuration = ConfigurationInfo(
                constraints = base | budget[ConfigurationInfo].constraints,
                values = {},
            ),
            executor_config = CommandExecutorConfig(
                local_enabled = LOCAL,
                remote_enabled = not LOCAL,
                priority = TEST_PRIORITY if name.startswith("pool_test_") else COMPILE_PRIORITY,
                remote_execution_properties = pool_properties(cpu, memory_kb, cpu_class),
                remote_execution_use_case = "hirsel",
                remote_output_paths = "output_paths",
            ),
        ))

    # The invoking host, for tests that need its tools (qemu, cross compilers,
    # Valgrind). Registered last with its own budget value, so only a target that
    # names `host` resolves to it.
    host = ctx.attrs.host
    platforms.append(ExecutionPlatformInfo(
        label = host.label.raw_target(),
        configuration = ConfigurationInfo(
            constraints = base | host[ConfigurationInfo].constraints,
            values = {},
        ),
        executor_config = CommandExecutorConfig(
            local_enabled = True,
            remote_enabled = False,
            remote_cache_enabled = False,
        ),
    ))
    return [DefaultInfo(), ExecutionPlatformRegistrationInfo(platforms = platforms)]

_platforms_rule = rule(
    impl = _platforms,
    attrs = {
        "budgets": attrs.dict(attrs.string(), attrs.dep(providers = [ConfigurationInfo])),
        "constraints": attrs.list(attrs.dep(providers = [ConfigurationInfo])),
        "host": attrs.dep(providers = [ConfigurationInfo]),
    },
)

def declare_platforms(name, constraints):
    native.constraint_setting(name = "budget")
    budgets = {}
    requests = [tuple(b) for b in POOL_BUDGETS] + [tuple(CLIPPY_REQUEST)]
    if PROBE_CPUS:
        requests += [tuple(compile_request(profile, target)) for profile, targets in COMPILE_CLASSES.items() for target in targets]
    # The first platform is what a target without a budget (third-party
    # compiles) resolves to: the default request.
    default = tuple(DEFAULT_REQUEST + [""])
    for cpu, memory_kb, cpu_class in [default] + sorted(set(requests) - set([default])):
        budget = pool_budget_name(cpu, memory_kb, cpu_class)
        native.constraint_value(name = budget, constraint_setting = ":budget", visibility = ["PUBLIC"])
        budgets["{}|{}|{}|{}".format(budget, cpu, memory_kb, cpu_class)] = ":" + budget
    for cpu, memory_kb, cpu_class in sorted(set([tuple(r) for r in TEST_REQUESTS.values()] + [tuple(DEFAULT_TEST_REQUEST)])):
        test = _test_budget(cpu, memory_kb, cpu_class)
        native.constraint_value(name = test, constraint_setting = ":budget", visibility = ["PUBLIC"])
        budgets["{}|{}|{}|{}".format(test, cpu, memory_kb, cpu_class)] = ":" + test
    native.constraint_value(name = "host", constraint_setting = ":budget", visibility = ["PUBLIC"])
    _platforms_rule(name = name, budgets = budgets, constraints = constraints, host = ":host")
