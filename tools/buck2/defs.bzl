"""First-party Rust targets, as tools/buck2/sync.py writes them into the root BUCK.

Every first-party target is in the root package, so `srcs` keep their
repository paths inside a compile action and relative `include_str!` paths
resolve as they do under Cargo.

Tests run as Cargo runs them, from their package's directory. Every compiled
path Cargo makes absolute is relative to that directory instead (set with
rustc's `--env-set`, which wins over the absolute sandbox path the prelude
would give): `env!("CARGO_MANIFEST_DIR")` is `../../crates/<crate>`, which
names the crate from any package directory, locally or on the pool, and so do
CARGO_TARGET_TMPDIR and CARGO_BIN_EXE_<name>.
"""

load(":platforms.bzl", "CLIPPY_REQUEST", "PRECHECK_REQUEST", "PROBE_CPUS", "budget_constraint", "compile_request", "doctest_properties", "dist_compile_request", "pool_budget_name", "test_budget_name", "test_executor", "test_request", "test_memory_limit")
load(":test_requirements.bzl", "EXACT_TEST_CPUS", "ISOLATED_TEST_CASES")
load(":exec_sizes.bzl", "COMPILE_CLASSES", "COMPILE_REQUESTS")
load(":profile.bzl", "runtime_profile_flags", "workspace_profile_flags")
load("@prelude//linking:link_info.bzl", "ArchiveLinkable", "LinkStrategy", "ObjectsLinkable")
load("@prelude//rust:link_info.bzl", "RustLinkInfo", "dfs_dedupe_by_label")

# Cargo gives integration tests an existing, private scratch directory; this is
# its repository-relative stand-in, created by the test launcher.
TEST_TMPDIR = "buck-out/tmp/cargo-target-tmpdir"

# An integration test's CARGO_BIN_EXE_<name> is compiled in as
# `<BIN_EXE_DIR>/<harness>/<name>`, which the test launcher links to the
# binary the test run carries. With `$(location)` in the harness's flags each
# binary was an input of the harness compile: the conformance harness link
# waited for five whole-compiler links and fetched their 974 MB on its worker,
# though rustc reads only the path.
BIN_EXE_DIR = "buck-out/tmp/cargo-bin-exe"

# `-c hirsel.tests=host` explicitly opts into execution on the invoking host;
# `-c hirsel.tests=focus` uses one CPU unless a target requires an exact count.
TESTS_ON_HOST = read_root_config("hirsel", "tests", "pool") == "host"
# The seed of every randomly seeded test (PROPTEST_RNG_SEED). It is part of the
# test's environment, so of its cached verdict: a pass is reused only under the
# same seed, and main's hourly run draws a new one each run (tools/main-runs).
TEST_SEED = read_root_config("hirsel", "test_seed", "1")

def _request(binary):
    return [str(field) for field in test_request(binary)]

def _test_platform(binary):
    """The invoking host, or the pool request measured for `binary` (its shards share it)."""
    return [budget_constraint("host" if TESTS_ON_HOST else test_budget_name(binary))]

def _from_package(manifest_dir, path):
    """`path`, relative to the repository root, as seen from `manifest_dir`."""
    return "../" * len(manifest_dir.split("/")) + path

def _cargo_env(package_name, crate, version):
    numbers = version.split("-")[0].split(".")
    return {
        "CARGO_CRATE_NAME": crate,
        "CARGO_PKG_NAME": package_name,
        "CARGO_PKG_VERSION": version,
        "CARGO_PKG_VERSION_MAJOR": numbers[0],
        "CARGO_PKG_VERSION_MINOR": numbers[1],
        "CARGO_PKG_VERSION_PATCH": numbers[2],
        # Clippy looks for clippy.toml from CLIPPY_CONF_DIR, else
        # CARGO_MANIFEST_DIR, upwards. The workspace has none, so the action's
        # root gives Clippy's defaults, as under Cargo; compiled code sees the
        # `--env-set` CARGO_MANIFEST_DIR.
        "CLIPPY_CONF_DIR": ".",
    }

# A first-party binary finds libLLVM (and the libraries bundled for it) in the
# pinned tree's source directory, from its own buck-out location
# (buck-out/v2/<art|gen>/<cell>/<configuration>/__<target>__/<binary>), also
# when a test runs it with a cleared environment. DT_RPATH, unlike DT_RUNPATH,
# also serves libLLVM's own dependencies. Tests on the pool carry the
# directory (toolchains//:llvm-libraries).
LLVM_RPATH = [
    "-Clink-arg=-Wl,--disable-new-dtags",
    "-Clink-arg=-Wl,-rpath,$ORIGIN/../../../../../../tools/buck2/toolchains/llvm-files/lib",
]

# Line tables were 61% of a whole-compiler executable (128 of 210 MB), which
# every link uploads to the cache and every test run fetches. lld compresses
# them; std's backtraces read zlib sections (zstd ones lose file and line), so
# failing tests keep their locations.
EXECUTABLE_LINK_FLAGS = LLVM_RPATH + ["-Clink-arg=-Wl,--compress-debug-sections=zlib"]

def _env_set(key, value):
    return "--env-set={}={}".format(key, value)

def _compile_constraints(name):
    return select({
        "root//tools/buck2:profile_release": [budget_constraint(pool_budget_name(*compile_request("release", name)))],
        "root//tools/buck2:profile_dist": [budget_constraint(pool_budget_name(*dist_compile_request(name)))],
        "root//tools/buck2:profile_runtime": [budget_constraint(pool_budget_name(*compile_request("release", name)))],
        "root//tools/buck2:profile_profiling": [budget_constraint(pool_budget_name(*compile_request("release", name)))],
        "DEFAULT": [budget_constraint(pool_budget_name(*compile_request("dev", name)))],
    })

def _release_codegen_units(name):
    # A measured release compile's codegen units match its reservation, and a
    # one-CPU compile keeps two: its slot share covers them
    # (action_sizes_from_log.py). Unmeasured targets keep Cargo's defaults.
    if name not in COMPILE_REQUESTS["release"] and not (PROBE_CPUS and name in COMPILE_CLASSES["release"]):
        return []
    return ["-Ccodegen-units={}".format(max(2, compile_request("release", name)[0]))]

# A dev compile splits its code into 16 units, Cargo's non-incremental default,
# whatever its reservation: LLVM then spreads the units over the reserved CPUs
# instead of waiting on the largest of two to four. A compile that reserves
# more than one CPU also runs rustc's front end on that many threads. On types
# edits, alternated with the reservation-sized units (median execution): the rc
# rlib 37 -> 20 s, validate 48 -> 35 s, lir 30 -> 22 s, types 15 -> 8.5 s, the
# driver `it` harness 14.5 -> 10 s, rc metadata 8 -> 4 s.
DEV_CODEGEN_UNITS = 16

def _dev_codegen(name):
    cpus = compile_request("dev", name)[0]
    return ["-Ccodegen-units={}".format(DEV_CODEGEN_UNITS)] + (["-Zthreads={}".format(cpus)] if cpus > 1 else [])

def _codegen_flags(name):
    return select({
        "root//tools/buck2:profile_release": _release_codegen_units(name),
        "root//tools/buck2:profile_dist": [],
        "root//tools/buck2:profile_runtime": [],
        "root//tools/buck2:profile_profiling": _release_codegen_units(name),
        "DEFAULT": _dev_codegen(name),
    })

def _flags(manifest_dir, extra = [], host_code = False, target = None, opt_level = None):
    # A member's `[profile.dev.package.<name>] opt-level`; the release flags
    # that follow override it, as Cargo applies the entry to dev only.
    dev = ["-Copt-level={}".format(opt_level)] if opt_level != None else []
    return ["-Zunstable-options", _env_set("CARGO_MANIFEST_DIR", _from_package(manifest_dir, manifest_dir))] + dev + extra + workspace_profile_flags(host_code) + (_codegen_flags(target) if target else []) + runtime_profile_flags(manifest_dir.split("/")[-1].replace("-", "_"))

def _compile_env(package_name, crate, version, build_script):
    env = _cargo_env(package_name, crate, version)
    if build_script:
        env["OUT_DIR"] = "$(location {})".format(build_script)
    return env

# `-c hirsel.clippy=true` (tools/buck2/clippy.py) declares every Rust target's
# Clippy twin `<name>-clippy`: the same rule and attributes on a measured 1 CPU
# platform. Buck2 gives a target one execution platform, so Clippy
# on the target itself reserves its whole compile request. The twin shares its
# target's dependencies and sources: building its `[clippy.txt]` runs Clippy
# and nothing the build has not already run. Without the option the twins do
# not exist, so `buck2 build //...` never compiles them.
CLIPPY_TWINS = read_root_config("hirsel", "clippy", "false") == "true"

def _rust(rule, name, **attrs):
    if attrs.get("crate") in ["hirsel_llvm", "hirsel_cranelift", "cpu_pool_4", "cpu_pool_8"] and read_root_config("hirsel", "native_tsan", "false") == "true":
        attrs["features"] = attrs.get("features", []) + ["native-tsan"]
    # Runtime diagnostic features are explicit build selections. Ordinary
    # archives omit ownership hooks and their private observer state.
    if attrs.get("crate") == "hirsel_rt":
        if read_root_config("hirsel", "operation_latency", "false") == "true":
            attrs["features"] = attrs.get("features", []) + ["operation-latency"]
        if name == "hirsel-rt" and read_root_config("hirsel", "ownership_trace", "false") == "true" and "ownership-trace" not in attrs.get("features", []):
            attrs["features"] = attrs.get("features", []) + ["ownership-trace"]
        attrs["features"] = attrs.get("features", []) + select({
            "root//tools/buck2:sanitizer_thread": ["tsan"],
            "DEFAULT": [],
        })
    rule(name = name, **attrs)
    if CLIPPY_TWINS:
        twin = attrs | dict(exec_compatible_with = [budget_constraint(pool_budget_name(*CLIPPY_REQUEST))], visibility = ["PUBLIC"])
        twin.pop("doctests", None)
        twin.pop("labels", None)
        rule(name = name + "-clippy", **twin)

def hirsel_library(name, crate, crate_root, srcs, features, deps, manifest_dir, package_name, version, edition, build_script = None, staticlib = False, doctests = False, opt_level = None):
    """A workspace library. With `doctests` it is also the test of its
    documentation examples, as `cargo test` runs them (label `doctest`)."""
    _rust(
        native.rust_library,
        name = name,
        crate = crate,
        crate_root = crate_root,
        srcs = srcs,
        features = features,
        deps = deps,
        edition = edition,
        env = _compile_env(package_name, crate, version, build_script),
        rustc_flags = _flags(manifest_dir, target = name, opt_level = opt_level),
        exec_compatible_with = _compile_constraints(name),
        doctests = doctests,
        doc_remote_execution = doctest_properties() if doctests else None,
        labels = ["doctest"] if doctests else [],
        visibility = ["PUBLIC"],
    )

def hirsel_binary(name, bin_name, crate, crate_root, srcs, features, deps, manifest_dir, package_name, version, edition, build_script = None, opt_level = None):
    env = _compile_env(package_name, crate, version, build_script)
    env["CARGO_BIN_NAME"] = bin_name
    env["HIRSEL_SANITIZER"] = select({"root//tools/buck2:sanitizer_thread": "thread", "DEFAULT": ""})
    _rust(
        native.rust_binary,
        name = name,
        crate = crate,
        crate_root = crate_root,
        srcs = srcs,
        features = features,
        deps = deps,
        edition = edition,
        env = env,
        rustc_flags = _flags(manifest_dir, EXECUTABLE_LINK_FLAGS, target = name, opt_level = opt_level),
        exec_compatible_with = _compile_constraints(name),
        visibility = ["PUBLIC"],
    )

def native_tool_inputs():
    """The unpacked toolchains a native test reads, as `HIRSEL_TOOLCHAIN_<TARGET>`: the
    launcher sets the variables of `native-toolchain/environment.tsv` from them."""
    return {
        "HIRSEL_TOOLCHAIN_" + name.replace("-", "_").upper(): "root//tools/buck2/native-toolchain:" + name
        for name in ["arm", "qemu", "valgrind", "kit", "glibc_floor", "linker", "runtime-auditor"]
    }

def hirsel_test(name, crate, crate_root, srcs, features, deps, manifest_dir, package_name, version, edition, build_script = None, bin_exes = {}, data = [], runtime_archives = None, products = None, shards = 1, costs = None, opt_level = None, seeded = False, skips = []):
    """A libtest harness (`rustc --test`) and the test that runs it.

    `bin_exes` are the package's binaries, as Cargo's CARGO_BIN_EXE_<name>.
    `data` are the files the test reads at run time; `runtime_archives` the
    native runtime archives it links programs with; `products` the ordinary
    std query products its programs load. `shards` splits
    the cases over that many test targets `<name>-shard-<i>`, run in parallel,
    balanced by the measured seconds per case in the `costs` table.
    `seeded` binaries draw random cases and run under `TEST_SEED`.
    `skips` are exact case names every
    run skips except the full gate's (`-c hirsel.full=true`).
    """
    if package_name == "hirsel-rt" and read_root_config("hirsel", "ownership_trace", "false") == "true":
        features = features + ["ownership-trace"]
    harness = name + "-harness"
    bin_exe_dir = "{}/{}".format(BIN_EXE_DIR, harness)
    exes = [_env_set("CARGO_BIN_EXE_" + bin, _from_package(manifest_dir, "{}/{}".format(bin_exe_dir, bin))) for bin in sorted(bin_exes)]
    _rust(
        native.rust_binary,
        name = harness,
        incoming_transition = "root//tools/buck2:test_tool_transition",
        crate = crate,
        crate_root = crate_root,
        srcs = srcs,
        features = features,
        deps = deps,
        edition = edition,
        env = _compile_env(package_name, crate, version, build_script),
        rustc_flags = _flags(manifest_dir, ["--test", _env_set("CARGO_TARGET_TMPDIR", _from_package(manifest_dir, TEST_TMPDIR))] + exes + EXECUTABLE_LINK_FLAGS, target = harness, opt_level = opt_level),
        exec_compatible_with = _compile_constraints(harness),
        visibility = ["PUBLIC"],
    )
    common = dict(
        harness = ":" + harness,
        data = data,
        bin_exes = bin_exes,
        bin_exe_dir = bin_exe_dir,
        runtime_archives = runtime_archives,
        products = products,
        product_definitions = read_root_config("hirsel", "product_definitions", ""),
        llvm = "root//tools/buck2/native-toolchain:llvm",
        native_tools = native_tool_inputs() if runtime_archives else {},
        cwd = manifest_dir,
        test_binary = name,
        request = _request(name),
        seed = TEST_SEED if seeded else "",
        args = [] if FULL_GATE else [argument for case in skips for argument in ("--skip", case)],
        visibility = ["PUBLIC"],
    )
    isolated = ISOLATED_TEST_CASES.get(name, [])
    for shard in range(shards):
        _rust_test(
            name = name if shards == 1 else "{}-shard-{}".format(name, shard + 1),
            shard_index = shard,
            shards = shards if shards > 1 else 0,
            costs = costs,
            labels = [],
            exec_compatible_with = _test_platform(name),
            pool = not TESTS_ON_HOST,
            extra_env = {"HIRSEL_TEST_ISOLATED_CASES": " ".join(isolated)} if isolated else {},
            **common
        )
    isolated_common = {key: value for key, value in common.items() if key != "args"}
    for index, case in enumerate(isolated):
        _rust_test(
            name = "{}-isolated-{}".format(name, index + 1),
            args = common["args"] + ["--exact", case],
            costs = costs,
            labels = [],
            exec_compatible_with = _test_platform(name),
            pool = not TESTS_ON_HOST,
            extra_env = {"HIRSEL_TEST_DEDICATED_CASE": case},
            **isolated_common
        )

def _rust_test_impl(ctx):
    llvm = ctx.attrs.llvm[DefaultInfo].default_outputs[0]
    binaries = dict(ctx.attrs.bin_exes)
    binaries.update(ctx.attrs.release_bin_exes)
    exes = {bin: dep[DefaultInfo].default_outputs[0] for bin, dep in binaries.items()}
    inputs = ctx.attrs.data + [exe for _, exe in sorted(exes.items())] + [llvm, ctx.attrs._test_reporter]
    inputs += ctx.attrs._llvm_libraries[DefaultInfo].default_outputs
    tools = {name: dep[DefaultInfo].default_outputs[0] for name, dep in ctx.attrs.native_tools.items()}
    inputs += list(tools.values())
    env = {
        "HIRSEL_TEST_CWD": ctx.attrs.cwd,
        "HIRSEL_VALIDATE_STAGES": "1",
        "HIRSEL_TEST_TMPDIR": TEST_TMPDIR,
        # The same effective request as the executor, including focus mode.
        "HIRSEL_TEST_CPUS": ctx.attrs.request[0],
        "HIRSEL_TEST_PRODUCT_DEFINITIONS": ctx.attrs.product_definitions,
        "LLVM_SYS_231_PREFIX": cmd_args(llvm),
        # A pool worker's action supervisor can attribute a test run's CPU to
        # these Bazel test variables. They are stable per
        # target and shard, as they are part of the action key.
        "TEST_TARGET": str(ctx.label.raw_target()),
        # Resident PSS sets the reservation. The allocation guard can also
        # include unused thread stacks (TEST_GUARD_LIMITS), without reserving
        # those virtual bytes on the pool.
        "HIRSEL_MEMORY_LIMIT": "{}K".format(test_memory_limit(ctx.attrs.test_binary, int(ctx.attrs.request[1]))),
    }
    env.update(ctx.attrs.extra_env)
    if ctx.attrs.test_binary in EXACT_TEST_CPUS:
        env["HIRSEL_TEST_EXACT_CPUS"] = str(EXACT_TEST_CPUS[ctx.attrs.test_binary])
    env.update({name: cmd_args(tree) for name, tree in tools.items()})
    if tools:
        inputs.append(ctx.attrs._private_native_inputs)
        env["HIRSEL_PRIVATE_NATIVE_INPUTS"] = cmd_args(ctx.attrs._private_native_inputs)
        inputs.append(ctx.attrs._native_environment)
        env["HIRSEL_NATIVE_ENVIRONMENT"] = cmd_args(ctx.attrs._native_environment)
    if exes:
        env["HIRSEL_TEST_BIN_EXE_DIR"] = ctx.attrs.bin_exe_dir
        env["HIRSEL_TEST_BIN_EXES"] = cmd_args([cmd_args(bin, "=", exe, delimiter = "") for bin, exe in sorted(exes.items())], delimiter = " ")
    if ctx.attrs.test_binary:
        env["HIRSEL_TEST_BINARY"] = ctx.attrs.test_binary
    if ctx.attrs.shards:
        # Bazel's sharding variables: the 0-based shard of TEST_TOTAL_SHARDS.
        env["TEST_SHARD_INDEX"] = str(ctx.attrs.shard_index)
        env["TEST_TOTAL_SHARDS"] = str(ctx.attrs.shards)
    if ctx.attrs.costs:
        inputs.append(ctx.attrs.costs)
        env["HIRSEL_TEST_COSTS"] = cmd_args(ctx.attrs.costs)
    for name, file in ctx.attrs.files_env.items():
        inputs.append(file)
        env[name] = cmd_args(file)
    if ctx.attrs.pool:
        env["HIRSEL_TEST_POOL"] = "1"
        env["HIRSEL_TEST_REPORTER"] = cmd_args(ctx.attrs._test_reporter)
    if ctx.attrs.relocate:
        env["HIRSEL_TEST_RELOCATE"] = "1"
    if ctx.attrs.seed:
        env["PROPTEST_RNG_SEED"] = ctx.attrs.seed
    if ctx.attrs.runtime_archives:
        archives = ctx.attrs.runtime_archives[DefaultInfo].default_outputs[0]
        inputs.append(archives)
        env["HIRSEL_RUNTIME_ARCHIVES"] = cmd_args(archives)
    if ctx.attrs.products:
        products = ctx.attrs.products[DefaultInfo].default_outputs[0]
        inputs.append(products)
        env["HIRSEL_TEST_PRODUCTS"] = cmd_args(products)
        env["HIRSEL_PRODUCTS"] = cmd_args(products)
    if ctx.attrs.native_selection:
        selection = ctx.attrs.native_selection[DefaultInfo].default_outputs[0]
        inputs.append(selection)
        env["HIRSEL_NATIVE_SELECTION"] = cmd_args(selection)
    command = cmd_args(ctx.attrs._launcher, ctx.attrs.harness[RunInfo], ctx.attrs.args, hidden = inputs)
    if ctx.attrs.fixture_output:
        fixture = ctx.actions.declare_output("fixture", dir = True)
        env["HIRSEL_REPLAY_FIXTURE_OUT"] = cmd_args(fixture.as_output())
        ctx.actions.run(command, env = env, category = "deadline_replay_fixture")
        return [DefaultInfo(default_output = fixture)]
    return [
        DefaultInfo(default_output = ctx.attrs.harness[DefaultInfo].default_outputs[0]),
        RunInfo(args = command),
        ExternalRunnerTestInfo(
            type = "rust",
            command = [command],
            env = env,
            labels = ctx.attrs.labels,
            run_from_project_root = True,
            use_project_relative_paths = True,
            # A pool run's passing verdict is cached by its action key (command,
            # environment and inputs), so a rerun on unchanged inputs does not
            # execute; a failure is never cached. The host platform has no cache.
            supports_test_execution_caching = ctx.attrs.pool,
            default_executor = test_executor(int(ctx.attrs.request[0]), int(ctx.attrs.request[1]), ctx.attrs.request[2], ctx.attrs.test_binary) if ctx.attrs.pool else None,
        ),
    ]

_rust_test = rule(
    impl = _rust_test_impl,
    attrs = {
        "args": attrs.list(attrs.string(), default = []),
        "bin_exe_dir": attrs.string(default = ""),
        "bin_exes": attrs.dict(attrs.string(), attrs.transition_dep(cfg = "root//tools/buck2:test_tool_transition"), default = {}),
        "release_bin_exes": attrs.dict(attrs.string(), attrs.transition_dep(cfg = "root//tools/buck2:release_tool_transition"), default = {}),
        "costs": attrs.option(attrs.source(), default = None),
        "cwd": attrs.string(),
        # Environment variables naming input files.
        "files_env": attrs.dict(attrs.string(), attrs.source(), default = {}),
        "extra_env": attrs.dict(attrs.string(), attrs.string(), default = {}),
        "fixture_output": attrs.bool(default = False),
        "data": attrs.list(attrs.source(), default = []),
        "harness": attrs.transition_dep(cfg = "root//tools/buck2:test_tool_transition", providers = [RunInfo]),
        "labels": attrs.list(attrs.string(), default = []),
        "llvm": attrs.dep(),
        "native_selection": attrs.option(attrs.dep(), default = None),
        "native_tools": attrs.dict(attrs.string(), attrs.dep(), default = {}),
        "pool": attrs.bool(default = False),
        "relocate": attrs.bool(default = False),
        # A pool run's `[cpu, memory_kb, cpu_class]`, for its executor (platforms.bzl).
        "request": attrs.list(attrs.string(), default = []),
        "runtime_archives": attrs.option(attrs.dep(), default = None),
        "products": attrs.option(attrs.dep(), default = None),
        "product_definitions": attrs.string(default = ""),
        "seed": attrs.string(default = ""),
        "shard_index": attrs.int(default = 0),
        "shards": attrs.int(default = 0),
        "test_binary": attrs.string(default = ""),
        "_launcher": attrs.source(default = "root//tools/buck2:test_launcher.sh"),
        "_test_reporter": attrs.source(default = "root//tools/buck2:test_report.py"),
        "_private_native_inputs": attrs.source(default = "root//tools/buck2:private_native_inputs.py"),
        "_llvm_libraries": attrs.dep(default = "toolchains//:llvm-libraries"),
        "_native_environment": attrs.source(default = "root//tools/buck2/native-toolchain:environment.tsv"),
    },
)

# The full gate's campaigns are test targets only under `-c hirsel.full=true`
# (tools/main-runs), so `buck2 test //...` never starts one.
FULL_GATE = read_root_config("hirsel", "full", "false") == "true"

def _stage(name):
    """A gate stage's name without its `-shard-<i>`: its shards share one request."""
    stage, sep, shard = name.rpartition("-shard-")
    return stage if sep and shard.isdigit() else name

def selected_paths(key, index):
    """Slice `index` (1-based) of the corpus paths a lane chose, from
    `-c hirsel.<key>=<slice>;<slice>...`, each slice `<path>|<path>...`; None
    past the last slice."""
    slices = [part for part in read_root_config("hirsel", key, "").split(";") if part]
    if index > len(slices):
        return None
    return slices[index - 1].split("|")

def hirsel_command_test(name, binary, args, data, cwd, runtime_archives = None, products = None, full = False, native_selection = None, changed_native = False, selected = None, measured_as = None):
    """A gate stage as a test: `binary args...` run from its package directory `cwd`.

    On the pool its result is cached by its inputs, so an unchanged corpus and
    compiler pass without running. The binary uses the checked, optimized
    checked profile. A `full` stage exists only
    in the full gate. Its pool request is the one measured for its stage,
    the name without `-shard-<i>` (TEST_REQUESTS in platforms.bzl).
    """
    if full and not FULL_GATE:
        return
    # Test-runner --filter/--case arguments arrive after graph construction.
    # Whole campaign wrappers explicitly opt into the cached global cover;
    # direct filtered shard invocations must have no eager collection dep.
    if read_root_config("hirsel", "native_selection", "false") not in ["true", "derive"] and not read_root_config("hirsel", "native_selection_file", ""):
        native_selection = None
    if selected:
        # A slice of the lane's chosen programs; the setting is part of the
        # test command and therefore its cache key. No slice, no target.
        key, flag, index = selected
        paths = selected_paths(key, index)
        if paths == None:
            return
        args = args + [arg for path in paths for arg in [flag, path]]
    if "--corpus" in args:
        # The LLVM slice rotates with each full run's fresh seed.
        args = args + ["--seed", TEST_SEED]
    if changed_native:
        slices = [part for part in read_root_config("hirsel", "native_filters", "").split(";") if part]
        if not slices:
            slices = ["__no_changed_native_programs__"]
        for index, part in enumerate(slices):
            shard_name = name if len(slices) == 1 else name + "-shard-" + str(index + 1)
            _native_changed_test(shard_name, binary, args, data, cwd, runtime_archives, products,
                                 native_selection, measured_as or name, part.split("|"))
        return
    _rust_test(
        name = name,
        harness = binary,
        args = args,
        data = data,
        cwd = cwd,
        llvm = "root//tools/buck2/native-toolchain:llvm",
        native_tools = native_tool_inputs() if runtime_archives else {},
        runtime_archives = runtime_archives,
        products = products,
        labels = ["gate"],
        exec_compatible_with = _test_platform(measured_as or _stage(name)),
        pool = not TESTS_ON_HOST,
        request = _request(measured_as or _stage(name)),
        relocate = True,
        native_selection = native_selection,
        seed = TEST_SEED if full else "",
        visibility = ["PUBLIC"],
    )

def _native_changed_test(name, binary, args, data, cwd, runtime_archives, products, native_selection, measured_as, filters):
    _rust_test(
        name = name,
        harness = binary,
        args = args + [arg for path in filters for arg in ["--case", path]],
        data = data,
        cwd = cwd,
        llvm = "root//tools/buck2/native-toolchain:llvm",
        native_tools = native_tool_inputs() if runtime_archives else {},
        runtime_archives = runtime_archives,
        products = products,
        labels = ["gate"],
        exec_compatible_with = _test_platform(measured_as),
        pool = not TESTS_ON_HOST,
        request = _request(measured_as),
        relocate = True,
        native_selection = native_selection,
        visibility = ["PUBLIC"],
    )

def _script_binary_impl(ctx):
    return [DefaultInfo(default_output = ctx.attrs.script), RunInfo(args = cmd_args(ctx.attrs.script))]

_script_binary = rule(impl = _script_binary_impl, attrs = {"script": attrs.source()})

def hirsel_script_test(name, script, data, runtime_archives = None, full = True, args = [], bin_exes = {}, products = None, production = False, files_env = {}, sanitizer = None, host = False):
    """A script harness with the same hermetic inputs as native Rust tests.
    `files_env` names further input files as environment variables. `host`
    runs it on the invoking host, never the pool: for a tool built there."""
    if full and not FULL_GATE:
        return
    if sanitizer and read_root_config("hirsel", "sanitizer", "") != sanitizer:
        return
    binary = name + "-script"
    _script_binary(name = binary, script = script)
    _rust_test(
        name = name, harness = ":" + binary, args = args, data = data, cwd = ".",
        bin_exes = {} if production else bin_exes, release_bin_exes = bin_exes if production else {},
        bin_exe_dir = BIN_EXE_DIR + "/" + name, products = products, files_env = files_env,
        llvm = "root//tools/buck2/native-toolchain:llvm",
        native_tools = native_tool_inputs() if runtime_archives else {},
        runtime_archives = runtime_archives, labels = ["gate"],
        exec_compatible_with = [budget_constraint("host")] if host else _test_platform(name),
        pool = not (TESTS_ON_HOST or host),
        request = _request(name),
        visibility = ["PUBLIC"],
    )

def _link_hosts_local_impl(ctx):
    digests = ctx.actions.declare_output("digests.txt")
    tree = lambda dep: dep[DefaultInfo].default_outputs[0]
    tools = {name: tree(dep) for name, dep in ctx.attrs.tools.items()}
    command = cmd_args(
        "python3", ctx.attrs.script, "build", "--hirsel", tree(ctx.attrs.hirsel),
        "--list", ctx.attrs.listing, "--output", digests.as_output(),
        hidden = ctx.attrs.data + list(tools.values()),
    )
    env = {
        "LLVM_SYS_231_PREFIX": cmd_args(tools["llvm"]),
        "HIRSEL_LINK_KIT": cmd_args(tools["kit"]),
        "HIRSEL_RUNTIME_ARCHIVES": cmd_args(tools["archives"]),
        "HIRSEL_PRODUCTS": cmd_args(tools["products"]),
        "HIRSEL_QEMU_AARCH64": cmd_args(tools["qemu"], "/qemu-aarch64", delimiter = ""),
        "HIRSEL_AARCH64_SYSROOT": cmd_args(tools["arm"], "/aarch64-none-linux-gnu/libc", delimiter = ""),
    }
    # This host's half of the two-host gate: never the pool, whose half is the test.
    ctx.actions.run(command, env = env, local_only = True, category = "link_hosts")
    return [DefaultInfo(default_output = digests)]

_link_hosts_local = rule(impl = _link_hosts_local_impl, attrs = {
    # The same binary the test runs, so the two halves differ only in host and roots.
    "hirsel": attrs.dep(),
    "script": attrs.source(),
    "listing": attrs.source(),
    "data": attrs.list(attrs.source(), default = []),
    "tools": attrs.dict(attrs.string(), attrs.dep()),
})

def hirsel_link_hosts_test(name, listing, data):
    """The two-host link gate (tools/buck2/link_hosts.py): `<name>-local` builds
    `listing`'s programs on this host, and the test builds them on a pool worker,
    under different checkout, cache and scratch roots, and compares the bytes."""
    script = "root//tools/buck2:link_hosts.py"
    _link_hosts_local(
        name = name + "-local", hirsel = "//:hirsel-cli-bin-hirsel", script = script,
        listing = listing, data = data,
        tools = {
            "llvm": "root//tools/buck2/native-toolchain:llvm",
            "kit": "root//tools/buck2/native-toolchain:kit",
            "qemu": "root//tools/buck2/native-toolchain:qemu",
            "arm": "root//tools/buck2/native-toolchain:arm",
            "archives": "//:hirsel-rt-archives",
            "products": "//:hirsel-std-products",
        },
        exec_compatible_with = ["root//tools/buck2:host"], visibility = ["PUBLIC"],
    )
    binary = name + "-script"
    _script_binary(name = binary, script = script)
    harness = ":" + binary
    _rust_test(
        name = name, harness = harness,
        args = ["check", "--hirsel", "{}/{}/hirsel".format(BIN_EXE_DIR, harness), "--list", listing],
        files_env = {"HIRSEL_LINK_HOSTS_LOCAL": ":{}-local".format(name)},
        bin_exes = {"hirsel": "//:hirsel-cli-bin-hirsel"},
        bin_exe_dir = "{}/{}".format(BIN_EXE_DIR, harness),
        data = data + [listing], cwd = ".",
        llvm = "root//tools/buck2/native-toolchain:llvm",
        native_tools = native_tool_inputs(), runtime_archives = "//:hirsel-rt-archives",
        products = "//:hirsel-std-products", labels = ["gate"],
        exec_compatible_with = _test_platform(name), pool = not TESTS_ON_HOST,
        request = _request(name),
        visibility = ["PUBLIC"],
    )

PRODUCT_MEMORY_LIMIT = "2G"

def _products_impl(ctx):
    tree = ctx.actions.symlinked_dir("package", {f.short_path: f for f in ctx.attrs.srcs})
    products = ctx.actions.declare_output("products", dir = True)
    scratch = ctx.actions.declare_output("scratch", dir = True)
    script = ctx.actions.write("precompute.sh", [
        "set -eu",
        'mkdir -p "$3"; TMPDIR="$(cd "$3" && pwd)" "$1" precompute "$2" -o "$4"',
    ])
    ctx.actions.run(
        cmd_args("/bin/sh", script, ctx.attrs.hirsel[RunInfo],
                 cmd_args(tree, format = "{}/" + ctx.attrs.manifest),
                 scratch.as_output(), products.as_output(),
                 hidden = ctx.attrs._llvm_libraries[DefaultInfo].default_outputs),
        env = {"HIRSEL_MEMORY_LIMIT": PRODUCT_MEMORY_LIMIT, "HIRSEL_JOBS": "1", "HIRSEL_DAEMON": "off", "HIRSEL_RESULTS": "none@1"},
        category = "products",
    )
    return [DefaultInfo(default_output = products)]

_hirsel_products = rule(
    impl = _products_impl,
    attrs = {
        "hirsel": attrs.exec_dep(providers = [RunInfo]),
        "manifest": attrs.string(),
        "srcs": attrs.list(attrs.source()),
        "_llvm_libraries": attrs.dep(default = "toolchains//:llvm-libraries"),
    },
)

def hirsel_products(name, hirsel, **kwargs):
    compiler = name + "-compiler"
    native.configured_alias(name = compiler, actual = hirsel, platform = "root//tools/buck2:checked")
    _hirsel_products(name = name, hirsel = ":" + compiler, exec_compatible_with = [budget_constraint(pool_budget_name(*PRECHECK_REQUEST))], **kwargs)

def _toolchain_bundle_impl(ctx):
    # Copy the executable: a symlink would make current_exe resolve outside the
    # bundle, away from the sibling ordinary products it discovers automatically.
    bundle = ctx.actions.copied_dir("hirsel-toolchain", {
        "hirsel": ctx.attrs.hirsel[DefaultInfo].default_outputs[0],
        "hirsel-std.products": ctx.attrs.products[DefaultInfo].default_outputs[0],
        "stdlib/std@1": ctx.actions.copied_dir("std@1", {src.short_path.removeprefix("stdlib/std@1/"): src for src in ctx.attrs.std}),
    })
    return [
        DefaultInfo(default_output = bundle),
        RunInfo(args = cmd_args(bundle, format = "{}/hirsel")),
    ]

hirsel_toolchain_bundle = rule(
    impl = _toolchain_bundle_impl,
    attrs = {
        "hirsel": attrs.dep(),
        "products": attrs.dep(),
        "std": attrs.list(attrs.source()),
    },
)

def _workspace_build_script_run_impl(ctx):
    # Lay the declared workspace files out at their repository paths, so the
    # script finds them from CARGO_MANIFEST_DIR as it does under Cargo.
    tree = ctx.actions.symlinked_dir("workspace", {f.short_path: f for f in ctx.attrs.inputs})
    out_dir = ctx.actions.declare_output("OUT_DIR", dir = True)
    script = ctx.actions.write(
        "run.sh",
        [
            "set -eu",
            'tree="$(cd "$1" && pwd)"; script="$(cd "$(dirname "$2")" && pwd)/$(basename "$2")"; mkdir -p "$3"; out="$(cd "$3" && pwd)"',
            'cd "$tree/{}"'.format(ctx.attrs.manifest_dir),
            'CARGO_MANIFEST_DIR="$tree/{}" OUT_DIR="$out" "$script" > /dev/null'.format(ctx.attrs.manifest_dir),
        ],
    )
    ctx.actions.run(
        cmd_args("/bin/sh", script, tree, ctx.attrs.script[RunInfo], out_dir.as_output()),
        env = {
            "CARGO_PKG_NAME": ctx.attrs.package_name,
            "CARGO_PKG_VERSION": ctx.attrs.version,
            "TARGET": "x86_64-unknown-linux-gnu",
            "HOST": "x86_64-unknown-linux-gnu",
            "PROFILE": "debug",
        } | {"CARGO_FEATURE_" + f.upper().replace("-", "_"): "1" for f in ctx.attrs.features},
        category = "build_script_run",
        allow_cache_upload = True,
    )
    return [DefaultInfo(default_output = out_dir)]

_workspace_build_script_run = rule(
    impl = _workspace_build_script_run_impl,
    attrs = {
        "features": attrs.list(attrs.string(), default = []),
        "inputs": attrs.list(attrs.source()),
        "manifest_dir": attrs.string(),
        "package_name": attrs.string(),
        "script": attrs.exec_dep(providers = [RunInfo]),
        "version": attrs.string(),
    },
)

def hirsel_build_script(name, crate_root, srcs, deps, inputs, features, manifest_dir, package_name, version, edition):
    """A Cargo build script: compiled once, then run over its declared inputs.

    Its output directory is the library's OUT_DIR. Cargo rerun directives are
    discarded: they contain sandbox paths and directory traversal order, and
    Buck2 already tracks the declared inputs. Any other directives a script
    needs must be translated here.
    """
    _rust(
        native.rust_binary,
        name = name + "-bin",
        incoming_transition = "root//tools/buck2:host_transition",
        crate = "build_script_build",
        crate_root = crate_root,
        srcs = srcs,
        deps = deps,
        edition = edition,
        env = _cargo_env(package_name, "build_script_build", version),
        rustc_flags = _flags(manifest_dir, host_code = True, target = name + "-bin"),
        exec_compatible_with = _compile_constraints(name + "-bin"),
    )
    _workspace_build_script_run(
        name = name,
        script = ":" + name + "-bin",
        inputs = inputs + [manifest_dir + "/Cargo.toml"],
        features = features,
        manifest_dir = manifest_dir,
        package_name = package_name,
        version = version,
    )

# The runtime archive variants `RuntimeBuild::variant` names
# (crates/hirsel-driver/src/native/link.rs), in its feature order: every subset
# of RUNTIME_FEATURES for every target, and each RUNTIME_SOLO_VARIANTS feature
# alone for the targets it lists (memcheck runs only x86-64 executables).
RUNTIME_FEATURES = ["diagnostic", "system-allocator", "measurement"]
RUNTIME_SOLO_VARIANTS = {"memcheck": ["x86_64-unknown-linux-gnu"]}
RUNTIME_TARGETS = {
    "aarch64-unknown-linux-gnu": "root//tools/buck2:runtime-aarch64",
    "x86_64-unknown-linux-gnu": "root//tools/buck2:runtime-x86_64",
}

def hirsel_runtime_archives(name, crate, crate_root, srcs, features, deps, feature_deps, manifest_dir, package_name, version, edition):
    """Every runtime archive the native pipeline links.

    Panic-abort release code with LTO and cold-crate size overrides: one
    staticlib per feature subset for each target triple, and per
    solo variant for its listed triples, laid out
    as `<triple>/<features or default>/libhirsel_rt.a` (debug-stripped) and
    `.../debug/libhirsel_rt.a`, with `<triple>/libhirsel_builtins.a`: the
    directory native tests and tools read from HIRSEL_RUNTIME_ARCHIVES.
    """
    archives = {}
    variants = [
        ([f for i, f in enumerate(RUNTIME_FEATURES) if mask & (1 << i)], RUNTIME_TARGETS.keys())
        for mask in range(1 << len(RUNTIME_FEATURES))
    ] + [([feature], triples) for feature, triples in RUNTIME_SOLO_VARIANTS.items()]
    # Diagnostic products are selected explicitly; default native builds never
    # compile the ownership buffer or double their runtime archive matrix.
    if read_root_config("hirsel", "ownership_trace", "false") == "true":
        variants += [(chosen + ["ownership-trace"], triples) for chosen, triples in variants]
    else:
        # The explicit ownership fixtures use this one native diagnostic product.
        # Keep the default runtime free of hooks and avoid a second feature matrix.
        variants.append((["ownership-trace"], ["x86_64-unknown-linux-gnu"]))
    for chosen, triples in variants:
        variant = "+".join(chosen) or "default"
        library = "{}-{}".format(name, variant.replace("+", "-"))
        extra = [dep for feature in chosen for dep in feature_deps.get(feature, [])]
        _rust(
            native.rust_library,
            name = library,
            crate = crate,
            crate_root = crate_root,
            srcs = srcs,
            features = features + chosen,
            deps = deps + extra,
            edition = edition,
            env = _cargo_env(package_name, crate, version),
            rustc_flags = _flags(manifest_dir, target = library),
            exec_compatible_with = _compile_constraints(library),
        )
        for triple in triples:
            configured = "{}-{}".format(library, triple.split("-")[0])
            native.configured_alias(name = configured, actual = ":" + library, platform = RUNTIME_TARGETS[triple])
            archives["{}/{}".format(triple, variant)] = ":" + configured
    _runtime_archives(name = name, archives = archives, visibility = ["PUBLIC"])

def hirsel_tsan_runtime_archives(name):
    """One instrumented native runtime, with its matching sanitizer and std."""
    if read_root_config("hirsel", "native_tsan", "false") != "true":
        return
    native.configured_alias(name = name + "-runtime", actual = ":hirsel-rt", platform = "root//tools/buck2:thread")
    _runtime_archives(
        name = name,
        archives = {"x86_64-unknown-linux-gnu/default": ":" + name + "-runtime"},
        sanitizer_archive = "toolchains//:tsan-runtime",
        visibility = ["PUBLIC"],
    )

def hirsel_native_pool_tsan(name, width, data, products):
    if read_root_config("hirsel", "native_tsan", "false") != "true":
        return
    binary = "hirsel-cli-test-cpu_pool_{}".format(width)
    _script_binary(name = name + "-script", script = "tools/workers-tsan")
    _rust_test(
        name = name, harness = ":" + name + "-script", args = ["--native", str(width)],
        test_binary = binary, data = data, cwd = ".", products = products,
        bin_exes = {"hirsel": ":hirsel-cli-bin-hirsel"},
        bin_exe_dir = BIN_EXE_DIR + "/" + binary + "-harness",
        files_env = {"HIRSEL_NATIVE_CPU_HARNESS": ":" + binary + "-harness"},
        llvm = "root//tools/buck2/native-toolchain:llvm", native_tools = native_tool_inputs(),
        runtime_archives = ":hirsel-rt-tsan-archives", labels = ["gate"],
        exec_compatible_with = _test_platform(binary), pool = not TESTS_ON_HOST,
        request = _request(binary), visibility = ["PUBLIC"],
    )

def _runtime_archives_impl(ctx):
    entries = {}
    llvm = ctx.attrs._llvm[DefaultInfo].default_outputs[0]
    for path, dep in ctx.attrs.archives.items():
        archive = dep[DefaultInfo].sub_targets["staticlib"][DefaultInfo].default_outputs[0]
        # rustc's staticlib bundles Rust rlibs, but Buck2's native Cxx deps
        # live in a separate link graph. Follow the prelude's RustLinkInfo
        # traversal and bundle those objects too, as Cargo's staticlib does.
        native_archives = {}
        objects = {}
        thin_objects = {}
        for native in dfs_dedupe_by_label(dep[RustLinkInfo].native_link_deps):
            infos = native._infos.get(LinkStrategy("static_pic"))
            if infos == None:
                continue
            for info in infos.traverse():
                for linkable in info.default.linkables:
                    if isinstance(linkable, ArchiveLinkable):
                        native_archives[linkable.archive.artifact] = None
                        for obj in linkable.archive.external_objects:
                            thin_objects[obj] = None
                    elif isinstance(linkable, ObjectsLinkable):
                        for obj in linkable.objects or []:
                            objects[obj] = None
                    else:
                        fail("runtime staticlib has an unbundled native dependency: {}".format(linkable))
        if native_archives or objects or ctx.attrs.sanitizer_archive:
            bundled = ctx.actions.declare_output(path + "/libhirsel_rt.a")
            command = cmd_args(
                "python3", ctx.attrs._bundler,
                "--ar", cmd_args(llvm.project("bin/hirsel-llvm-ar"), hidden = llvm),
                "--output", bundled.as_output(),
                "--archive", archive,
            )
            for lib in native_archives:
                command.add("--archive", lib)
            if ctx.attrs.sanitizer_archive:
                command.add("--archive", ctx.attrs.sanitizer_archive)
                # Rust's TSan pass emits cleanup landing pads with the C
                # personality. Use compiler-rt's implementation, without
                # bundling a second copy of every arithmetic builtin.
                for member in ["gcc_personality_v0.c.o", "int_util.c.o"]:
                    command.add("--member", cmd_args(llvm.project("lib/clang/23/lib/linux/libclang_rt.builtins-x86_64.a"), hidden = llvm), member)
            for obj in objects:
                command.add("--object", obj)
            # Thin archives need their referenced objects as action inputs.
            # They are expanded by llvm-ar ADDLIB, not appended twice.
            command.add(cmd_args(hidden = list(thin_objects)))
            ctx.actions.run(command, category = "runtime_archive", identifier = path.replace("/", "-"))
            archive = bundled
        # Default links read the debug-stripped archive; `--debug` links the
        # debug-bearing one. Each triple's default archive also gives program
        # libraries its compiler builtins (runtime_parts.py).
        stripped = ctx.actions.declare_output(path + "/stripped/libhirsel_rt.a")
        command = cmd_args(
            "python3", ctx.attrs._parts, "--llvm", llvm,
            "--archive", archive, "--stripped", stripped.as_output(),
        )
        triple, variant = path.split("/")
        if variant == "default":
            builtins = ctx.actions.declare_output(triple + "/libhirsel_builtins.a")
            command.add("--builtins", builtins.as_output())
            entries[triple + "/libhirsel_builtins.a"] = builtins
        ctx.actions.run(command, category = "runtime_parts", identifier = path.replace("/", "-"))
        entries[path + "/libhirsel_rt.a"] = stripped
        entries[path + "/debug/libhirsel_rt.a"] = archive
    # Qualification consumers can request one target/variant without building
    # unrelated archives. Each view also carries that target's compiler builtins.
    views = {}
    for path in ctx.attrs.archives:
        triple, variant = path.split("/")
        selected = {key: artifact for key, artifact in entries.items()
                    if key.startswith(path + "/") or key == triple + "/libhirsel_builtins.a"}
        view = triple.split("-")[0] + "-" + variant
        views[view] = [DefaultInfo(default_output = ctx.actions.symlinked_dir(view, selected))]
    return [DefaultInfo(default_output = ctx.actions.symlinked_dir(ctx.label.name, entries), sub_targets = views)]

_runtime_archives = rule(
    impl = _runtime_archives_impl,
    attrs = {
        "archives": attrs.dict(attrs.string(), attrs.dep()),
        "sanitizer_archive": attrs.option(attrs.source(), default = None),
        "_bundler": attrs.source(default = "//tools/buck2:bundle_archive.py"),
        "_parts": attrs.source(default = "//tools/buck2:runtime_parts.py"),
        "_llvm": attrs.exec_dep(default = "toolchains//:clang"),
    },
)


def hirsel_deadline_replay_matrix(name, data, runtime_archives, products):
    """One immutable recording per backend, with independently bounded cells.

    Fixture errors are artifacts too: every dependent libtest cell reports the
    first failed phase. No width regenerates a tape or warms reference startup.
    """
    binary = "hirsel-cli-test-cli"
    common = dict(
        harness = ":" + binary + "-harness",
        data = data,
        bin_exes = {"hirsel": "//:hirsel-cli-bin-hirsel"},
        bin_exe_dir = BIN_EXE_DIR + "/" + binary + "-harness",
        runtime_archives = runtime_archives,
        products = products,
        llvm = "root//tools/buck2/native-toolchain:llvm",
        native_tools = native_tool_inputs(),
        cwd = "crates/hirsel-cli",
        test_binary = binary,
        request = _request(binary),
        exec_compatible_with = _test_platform(binary),
        pool = not TESTS_ON_HOST,
        visibility = ["PUBLIC"],
    )
    for backend in ["llvm", "cranelift"]:
        fixture = name + "-" + backend + "-fixture"
        _rust_test(
            name = fixture,
            fixture_output = True,
            extra_env = {"HIRSEL_REPLAY_BACKEND": backend},
            args = ["--ignored", "--exact", "ordered_deadline_liveness_55::fixture", "--nocapture"],
            **common
        )
        for cell in ["reference", "1", "2", "4"]:
            _rust_test(
                name = name + "-" + backend + "-" + cell,
                files_env = {"HIRSEL_REPLAY_FIXTURE": ":" + fixture},
                extra_env = {"HIRSEL_REPLAY_BACKEND": backend, "HIRSEL_REPLAY_CELL": cell},
                args = ["--ignored", "--exact", "ordered_deadline_liveness_55::cell", "--nocapture"],
                labels = [],
                **common
            )
