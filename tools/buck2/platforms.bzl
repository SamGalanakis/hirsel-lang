"""Execution platforms: one per remote pool budget, plus the invoking host.

`-c hirsel.execution=local` (no `.buckconfig.local` needed) runs every action
on this host instead. Otherwise actions go to the remote-execution pool that
`.buckconfig.local` configures; its scheduler matches the platform properties
below, plus the pool's own `hirsel.remote_properties`, and reserves the
requested CPUs and memory for the action.

A budget is (CPU, memory, CPU class). Only the critical-path compiles named in
CPU_CLASSES (tools/buck2/action_sizes_from_log.py), the std precheck
(PRECHECK_REQUEST) and the conformance shards (TEST_REQUESTS) ask for
`cpu_class=fast`; every other compile, test run and gate shard runs on any worker.

Every hirsel remote action, compile or test run, gate, focus or full, queues
at priority 50 (land's at 75, below): below the pool's lane and CI class (100), so that work
dispatches first, and above its warming class (0), which never gets a
reserved worker. It is one priority because the scheduler merges identical
actions and keeps the first submitter's: a test run once sent at 0 held later
gates' runs of the same digest at 0 for up to 113 minutes. The scheduler reads the REAPI execution policy's priority, which Buck2 sends from a
`CommandExecutorConfig`: a platform's for its actions, and a test's own
`default_executor` (`test_executor`) for its run. The priority is not a platform
property, so it is not part of an action key.

`-c hirsel.priority=N` sets it for one invocation. tools/land builds at 75:
its batch is the one serial step every lane waits on, and at 50 its few
hundred compiles queued behind the gates' test runs for 85-94% of each build
(2026-10-20). An action a gate already queued keeps the gate's 50 (the
merge rule above); the batch's own combinations are new digests.
"""

load(":exec_sizes.bzl", "COMPILE_CLASSES", "COMPILE_REQUESTS", "POOL_BUDGETS", "SPECIAL_REQUESTS")
load(":test_sizes.bzl", "TEST_REQUESTS", "TEST_GUARD_LIMITS")
load(":test_requirements.bzl", "EXACT_TEST_CPUS")

PRIORITY = int(read_root_config("hirsel", "priority", "50"))
if PRIORITY < 0 or PRIORITY > 99:
    fail("hirsel.priority must be from 0 to 99, below the pool's lane and CI class")

# A pool test binary's request, `[cpu, memory_kb, cpu_class]`, is measured
# from the workers' usage logs (TEST_REQUESTS, test_sizes_from_usage.py): one
# CPU for a binary that uses about one core, more only where it measured more.
# The test launcher runs as many test threads as the request's whole CPUs.
# A binary with no measurement yet runs as a single-threaded one with room.
DEFAULT_TEST_REQUEST = [1, 2097152, ""]
# Focus uses one CPU unless the binary declares a semantic exact requirement.
TEST_MODE = read_root_config("hirsel", "tests", "pool")
FOCUS = TEST_MODE == "focus"

def _test_request(cpu, memory_kb, cpu_class, exact_cpus = 0):
    """A test request's budget name and effective `[cpu, memory_kb, cpu_class]`."""
    if exact_cpus:
        if exact_cpus < 1:
            fail("exact test CPUs must be positive")
        return "pool_test_exact_" + pool_budget_name(exact_cpus, memory_kb, cpu_class).removeprefix("pool_"), [exact_cpus, memory_kb, cpu_class]
    if FOCUS:
        return "pool_focus_" + pool_budget_name(1, memory_kb, cpu_class).removeprefix("pool_"), [1, memory_kb, cpu_class]
    return "pool_test_" + pool_budget_name(cpu, memory_kb, cpu_class).removeprefix("pool_"), [cpu, memory_kb, cpu_class]

def _binary_test_request(binary):
    cpu, memory_kb, cpu_class = TEST_REQUESTS.get(binary, DEFAULT_TEST_REQUEST)
    return _test_request(cpu, memory_kb, cpu_class, EXACT_TEST_CPUS.get(binary, 0))

def test_budget_name(binary):
    return _binary_test_request(binary)[0]

def test_request(binary):
    """A test binary's effective pool request `[cpu, memory_kb, cpu_class]`."""
    return _binary_test_request(binary)[1]

def test_memory_limit(binary, memory_kb):
    # The data/address-space guard includes unused thread stacks; it is not
    # the process group's enforced PSS, which alone sets the reservation.
    return max(1048576, 2 * memory_kb, TEST_GUARD_LIMITS.get(binary, 0))

def test_executor(cpu, memory_kb, cpu_class, binary = ""):
    """A pool test run's executor: Buck2 runs a test with its own executor, not its platform's."""
    return CommandExecutorConfig(
        local_enabled = LOCAL,
        remote_enabled = not LOCAL,
        priority = PRIORITY,
        remote_execution_properties = pool_properties(cpu, memory_kb, cpu_class, exact_cpus = EXACT_TEST_CPUS.get(binary, 0)),
        remote_execution_use_case = "hirsel",
    )

def doctest_properties():
    """Rustdoc's explicit executor uses the libtest priority and a single CPU."""
    local = LOCAL or TEST_MODE == "host"
    return {
        "capabilities": pool_properties(1, 524288, ""),
        "use_case": "hirsel",
        "local_enabled": local,
        "remote_enabled": not local,
        "remote_cache_enabled": not local,
        "priority": PRIORITY,
    }

# The request of a compile no measurement sized.
DEFAULT_REQUEST = [1, 1048576]

# Every Clippy twin's request (defs.bzl): Clippy is a single-threaded front end.
CLIPPY_REQUEST = SPECIAL_REQUESTS["clippy"]

# The single-threaded std precheck is on every std test's serial chain;
# its peak PSS and 20% headroom come from the same fresh usage evidence as
# compiles. Keep the fast-worker preference.
PRECHECK_REQUEST = SPECIAL_REQUESTS["precheck"]

# `-c hirsel.probe_cpus=N` gives every CPU_CLASSES compile N CPUs (and, in
# release, N codegen units, defs.bzl) for one invocation: the calibration that a compile
# measured under one CPU cannot provide (action_sizes_from_log.py).
PROBE_CPUS = int(read_root_config("hirsel", "probe_cpus", "0"))
PROBE_MEMORY_KB = 4194304
if PROBE_CPUS < 0 or PROBE_CPUS > 8:
    fail("hirsel.probe_cpus must be from 1 to 8, or 0 for the measured requests")

def pool_budget_name(cpu, memory_kb, cpu_class = ""):
    name = "pool_{}_{}".format(cpu, memory_kb)
    return name + "_" + cpu_class if cpu_class else name

# Fat-LTO links aggregate the compiler's LLVM modules on one worker. Keep this
# reservation separate from release's small incremental compile/link actions.
DIST_LINK_REQUEST = [1, 4194304, ""]

def dist_compile_request(name):
    if name == "hirsel-cli-bin-hirsel":
        return DIST_LINK_REQUEST
    return compile_request("release", name)

def compile_request(profile, name):
    """A compile's `[cpu, memory_kb, cpu_class]` in `profile` (dev or release)."""
    # If this profile has not run recently, use the other profile's measured
    # memory rather than dropping a known large crate to the generic floor.
    # CPU parallelism must be established in this profile; a fallback gets one.
    other = "release" if profile == "dev" else "dev"
    cpu, memory_kb = COMPILE_REQUESTS[profile].get(name, [1, COMPILE_REQUESTS[other].get(name, DEFAULT_REQUEST)[1]])
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

def pool_properties(cpu, memory_kb, cpu_class = "", exact_cpus = 0):
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
    if exact_cpus:
        if int(cpu) != exact_cpus:
            fail("exact CPU request differs from scheduler reservation")
        properties["cpu_affinity"] = "exact-v1"
        properties["exact_cpus"] = str(exact_cpus)
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
                priority = PRIORITY,
                # Fixture build actions use the same exact-CPU platform as their
                # consuming tests; only tests have an explicit executor override.
                remote_execution_properties = pool_properties(cpu, memory_kb, cpu_class, exact_cpus = int(cpu) if name.startswith("pool_test_exact_") else 0),
                remote_execution_use_case = "hirsel",
                remote_output_paths = "output_paths",
            ),
        ))

    # The invoking host, for tests that need its tools (qemu, cross compilers,
    # Valgrind). Registered last with its own budget value, so only a target that
    # names `host` resolves to it. Nothing the pool could run belongs here
    # (toolchain unpacking and crate archives use the default pool platform):
    # its results never reach the remote cache, whose action cache the pool
    # serves read-only to clients, so every checkout would redo them.
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
    # Startup training peaks at 1.4 GiB and exercises four physical permits.
    requests = [(4, 2097152, "")] + [tuple(b) for b in POOL_BUDGETS] + [tuple(CLIPPY_REQUEST), tuple(PRECHECK_REQUEST), tuple(DIST_LINK_REQUEST)]
    if PROBE_CPUS:
        requests += [tuple(compile_request(profile, target)) for profile, targets in COMPILE_CLASSES.items() for target in targets]
    # The first platform is what a target without a budget (third-party
    # compiles) resolves to: the default request.
    default = tuple(DEFAULT_REQUEST + [""])
    for cpu, memory_kb, cpu_class in [default] + sorted(set(requests) - set([default])):
        budget = pool_budget_name(cpu, memory_kb, cpu_class)
        native.constraint_value(name = budget, constraint_setting = ":budget", visibility = ["PUBLIC"])
        budgets["{}|{}|{}|{}".format(budget, cpu, memory_kb, cpu_class)] = ":" + budget
    tests = {}
    for request in [DEFAULT_TEST_REQUEST] + TEST_REQUESTS.values():
        test, effective = _test_request(*request)
        tests[test] = effective
    for binary, exact_cpus in EXACT_TEST_CPUS.items():
        test, effective = _binary_test_request(binary)
        tests[test] = effective
    for test, (cpu, memory_kb, cpu_class) in sorted(tests.items()):
        native.constraint_value(name = test, constraint_setting = ":budget", visibility = ["PUBLIC"])
        budgets["{}|{}|{}|{}".format(test, cpu, memory_kb, cpu_class)] = ":" + test
    native.constraint_value(name = "host", constraint_setting = ":budget", visibility = ["PUBLIC"])
    _platforms_rule(name = name, budgets = budgets, constraints = constraints, host = ":host")
