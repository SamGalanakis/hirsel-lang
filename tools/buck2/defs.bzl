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

load(":platforms.bzl", "CLIPPY_REQUEST", "PROBE_CPUS", "budget_constraint", "compile_request", "pool_budget_name", "test_budget_name", "test_executor", "test_request")
load(":exec_sizes.bzl", "COMPILE_CLASSES", "COMPILE_REQUESTS")
load(":profile.bzl", "workspace_profile_flags")
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
# `-c hirsel.tests=focus` keeps them on the pool at one CPU each (platforms.bzl).
TESTS_ON_HOST = read_root_config("hirsel", "tests", "pool") == "host"
# The seed of every randomly seeded test (PROPTEST_RNG_SEED). It is part of the
# test's environment, so of its cached verdict: a pass is reused only under the
# same seed, and the full gate draws a new one each run (`tools/gate --full`).
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
        "DEFAULT": _dev_codegen(name),
    })

def _flags(manifest_dir, extra = [], host_code = False, target = None, opt_level = None):
    # A member's `[profile.dev.package.<name>] opt-level`; the release flags
    # that follow override it, as Cargo applies the entry to dev only.
    dev = ["-Copt-level={}".format(opt_level)] if opt_level != None else []
    return ["-Zunstable-options", _env_set("CARGO_MANIFEST_DIR", _from_package(manifest_dir, manifest_dir))] + dev + extra + workspace_profile_flags(host_code) + (_codegen_flags(target) if target else [])

def _compile_env(package_name, crate, version, build_script):
    env = _cargo_env(package_name, crate, version)
    if build_script:
        env["OUT_DIR"] = "$(location {})".format(build_script)
    return env

# `-c hirsel.clippy=true` (tools/buck2/clippy.py) declares every Rust target's
# Clippy twin `<name>-clippy`: the same rule and attributes on a 1 CPU/512 MiB
# platform. Buck2 gives a target one execution platform, so Clippy
# on the target itself reserves its whole compile request. The twin shares its
# target's dependencies and sources: building its `[clippy.txt]` runs Clippy
# and nothing the build has not already run. Without the option the twins do
# not exist, so `buck2 build //...` never compiles them.
CLIPPY_TWINS = read_root_config("hirsel", "clippy", "false") == "true"

def _rust(rule, name, **attrs):
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
        labels = ["doctest"] if doctests else [],
        visibility = ["PUBLIC"],
    )

def hirsel_binary(name, bin_name, crate, crate_root, srcs, features, deps, manifest_dir, package_name, version, edition, build_script = None, opt_level = None):
    env = _compile_env(package_name, crate, version, build_script)
    env["CARGO_BIN_NAME"] = bin_name
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
        "HIRSEL_TOOLCHAIN_" + name.upper(): "root//tools/buck2/native-toolchain:" + name
        for name in ["arm", "qemu", "valgrind"]
    }

def hirsel_test(name, crate, crate_root, srcs, features, deps, manifest_dir, package_name, version, edition, build_script = None, bin_exes = {}, data = [], runtime_archives = None, shards = 1, costs = None, opt_level = None, seeded = False, skips = []):
    """A libtest harness (`rustc --test`) and the test that runs it.

    `bin_exes` are the package's binaries, as Cargo's CARGO_BIN_EXE_<name>.
    `data` are the files the test reads at run time; `runtime_archives` the
    native runtime archives it links programs with. `shards` splits
    the cases over that many test targets `<name>-shard-<i>`, run in parallel,
    balanced by the measured seconds per case in the `costs` table.
    `seeded` binaries draw random cases and run under `TEST_SEED`.
    `skips` are exact case names every
    run skips except the full gate's (`-c hirsel.full=true`).
    """
    harness = name + "-harness"
    bin_exe_dir = "{}/{}".format(BIN_EXE_DIR, harness)
    exes = [_env_set("CARGO_BIN_EXE_" + bin, _from_package(manifest_dir, "{}/{}".format(bin_exe_dir, bin))) for bin in sorted(bin_exes)]
    _rust(
        native.rust_binary,
        name = harness,
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
        llvm = "root//tools/buck2/native-toolchain:llvm",
        native_tools = native_tool_inputs() if runtime_archives else {},
        cwd = manifest_dir,
        test_binary = name,
        request = _request(name),
        seed = TEST_SEED if seeded else "",
        args = [] if FULL_GATE else [argument for case in skips for argument in ("--skip", case)],
        visibility = ["PUBLIC"],
    )
    for shard in range(shards):
        _rust_test(
            name = name if shards == 1 else "{}-shard-{}".format(name, shard + 1),
            shard_index = shard,
            shards = shards if shards > 1 else 0,
            costs = costs if shards > 1 else None,
            labels = [],
            exec_compatible_with = _test_platform(name),
            pool = not TESTS_ON_HOST,
            **common
        )

def _rust_test_impl(ctx):
    llvm = ctx.attrs.llvm[DefaultInfo].default_outputs[0]
    exes = {bin: dep[DefaultInfo].default_outputs[0] for bin, dep in ctx.attrs.bin_exes.items()}
    inputs = ctx.attrs.data + [exe for _, exe in sorted(exes.items())] + [llvm]
    inputs += ctx.attrs._llvm_libraries[DefaultInfo].default_outputs
    tools = {name: dep[DefaultInfo].default_outputs[0] for name, dep in ctx.attrs.native_tools.items()}
    inputs += list(tools.values())
    env = {
        "HIRSEL_TEST_CWD": ctx.attrs.cwd,
        "HIRSEL_TEST_TMPDIR": TEST_TMPDIR,
        "LLVM_SYS_211_PREFIX": cmd_args(llvm),
        # A pool worker's action supervisor can attribute a test run's CPU to
        # these Bazel test variables. They are stable per
        # target and shard, as they are part of the action key.
        "TEST_TARGET": str(ctx.label.raw_target()),
    }
    env.update({name: cmd_args(tree) for name, tree in tools.items()})
    if tools:
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
    if ctx.attrs.pool:
        env["HIRSEL_TEST_POOL"] = "1"
    if ctx.attrs.relocate:
        env["HIRSEL_TEST_RELOCATE"] = "1"
    if ctx.attrs.seed:
        env["PROPTEST_RNG_SEED"] = ctx.attrs.seed
    if ctx.attrs.runtime_archives:
        archives = ctx.attrs.runtime_archives[DefaultInfo].default_outputs[0]
        inputs.append(archives)
        env["HIRSEL_RUNTIME_ARCHIVES"] = cmd_args(archives)
    if ctx.attrs.native_selection:
        selection = ctx.attrs.native_selection[DefaultInfo].default_outputs[0]
        inputs.append(selection)
        env["HIRSEL_NATIVE_SELECTION"] = cmd_args(selection)
    command = cmd_args(ctx.attrs._launcher, ctx.attrs.harness[RunInfo], ctx.attrs.args, hidden = inputs)
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
            default_executor = test_executor(int(ctx.attrs.request[0]), int(ctx.attrs.request[1]), ctx.attrs.request[2]) if ctx.attrs.pool else None,
        ),
    ]

_rust_test = rule(
    impl = _rust_test_impl,
    attrs = {
        "args": attrs.list(attrs.string(), default = []),
        "bin_exe_dir": attrs.string(default = ""),
        "bin_exes": attrs.dict(attrs.string(), attrs.dep(), default = {}),
        "costs": attrs.option(attrs.source(), default = None),
        "cwd": attrs.string(),
        "data": attrs.list(attrs.source(), default = []),
        "harness": attrs.dep(providers = [RunInfo]),
        "labels": attrs.list(attrs.string(), default = []),
        "llvm": attrs.dep(),
        "native_selection": attrs.option(attrs.dep(), default = None),
        "native_tools": attrs.dict(attrs.string(), attrs.dep(), default = {}),
        "pool": attrs.bool(default = False),
        "relocate": attrs.bool(default = False),
        # A pool run's `[cpu, memory_kb, cpu_class]`, for its executor (platforms.bzl).
        "request": attrs.list(attrs.string(), default = []),
        "runtime_archives": attrs.option(attrs.dep(), default = None),
        "seed": attrs.string(default = ""),
        "shard_index": attrs.int(default = 0),
        "shards": attrs.int(default = 0),
        "test_binary": attrs.string(default = ""),
        "_launcher": attrs.source(default = "root//tools/buck2:test_launcher.sh"),
        "_llvm_libraries": attrs.dep(default = "toolchains//:llvm-libraries"),
        "_native_environment": attrs.source(default = "root//tools/buck2/native-toolchain:environment.tsv"),
    },
)

# The full gate's campaigns are test targets only under `-c hirsel.full=true`
# (`tools/gate --full`), so `buck2 test //...` never starts one.
FULL_GATE = read_root_config("hirsel", "full", "false") == "true"

def _stage(name):
    """A gate stage's name without its `-shard-<i>`: its shards share one request."""
    stage, sep, shard = name.rpartition("-shard-")
    return stage if sep and shard.isdigit() else name

def hirsel_command_test(name, binary, args, data, cwd, runtime_archives = None, release = False, full = False, native_selection = None):
    """A gate stage as a test: `binary args...` run from its package directory `cwd`.

    On the pool its result is cached by its inputs, so an unchanged corpus and
    compiler pass without running. `release` builds the binary with Cargo's
    release profile, as `cargo run --release` would. A `full` stage exists only
    in the full gate. Its pool request is the one measured for its stage,
    the name without `-shard-<i>` (TEST_REQUESTS in platforms.bzl).
    """
    if full and not FULL_GATE:
        return
    if release:
        configured = name + "-binary"
        native.configured_alias(name = configured, actual = binary, platform = "root//tools/buck2:release")
        binary = ":" + configured
    _rust_test(
        name = name,
        harness = binary,
        args = args,
        data = data,
        cwd = cwd,
        llvm = "root//tools/buck2/native-toolchain:llvm",
        native_tools = native_tool_inputs() if runtime_archives else {},
        runtime_archives = runtime_archives,
        labels = ["gate"],
        exec_compatible_with = _test_platform(_stage(name)),
        pool = not TESTS_ON_HOST,
        request = _request(_stage(name)),
        relocate = True,
        native_selection = native_selection,
        seed = TEST_SEED if full else "",
        visibility = ["PUBLIC"],
    )

def _script_binary_impl(ctx):
    return [DefaultInfo(default_output = ctx.attrs.script), RunInfo(args = cmd_args(ctx.attrs.script))]

_script_binary = rule(impl = _script_binary_impl, attrs = {"script": attrs.source()})

def hirsel_script_test(name, script, data, runtime_archives = None):
    """A full-gate shell harness with the same hermetic inputs as native Rust tests."""
    if not FULL_GATE:
        return
    binary = name + "-script"
    _script_binary(name = binary, script = script)
    _rust_test(
        name = name, harness = ":" + binary, args = [], data = data, cwd = ".",
        llvm = "root//tools/buck2/native-toolchain:llvm",
        native_tools = native_tool_inputs() if runtime_archives else {},
        runtime_archives = runtime_archives, labels = ["gate"],
        exec_compatible_with = _test_platform(name), pool = not TESTS_ON_HOST,
        request = _request(name),
        visibility = ["PUBLIC"],
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
            'CARGO_MANIFEST_DIR="$tree/{}" OUT_DIR="$out" "$script" > "$out/.build-script-output"'.format(ctx.attrs.manifest_dir),
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

    Its output directory is the library's OUT_DIR. The script's stdout (its
    `cargo:` directives) is kept beside it; a script whose directives matter
    beyond OUT_DIR needs them translated here.
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
    """Every runtime archive the native pipeline links, as Cargo builds them.

    `cargo build --release -p hirsel-rt --lib [--features ...]` with aborting
    panics: one staticlib per feature subset for each target triple, and per
    solo variant for its listed triples, laid out
    as `<triple>/<features or default>/libhirsel_rt.a`, the directory native
    tests and tools read from HIRSEL_RUNTIME_ARCHIVES.
    """
    archives = {}
    variants = [
        ([f for i, f in enumerate(RUNTIME_FEATURES) if mask & (1 << i)], RUNTIME_TARGETS.keys())
        for mask in range(1 << len(RUNTIME_FEATURES))
    ] + [([feature], triples) for feature, triples in RUNTIME_SOLO_VARIANTS.items()]
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
        if native_archives or objects:
            bundled = ctx.actions.declare_output(path + "/libhirsel_rt.a")
            command = cmd_args(
                "python3", ctx.attrs._bundler,
                "--ar", cmd_args(llvm.project("bin/hirsel-llvm-ar"), hidden = llvm),
                "--output", bundled.as_output(),
                "--archive", archive,
            )
            for lib in native_archives:
                command.add("--archive", lib)
            for obj in objects:
                command.add("--object", obj)
            # Thin archives need their referenced objects as action inputs.
            # They are expanded by llvm-ar ADDLIB, not appended twice.
            command.add(cmd_args(hidden = list(thin_objects)))
            ctx.actions.run(command, category = "runtime_archive", identifier = path.replace("/", "-"))
            archive = bundled
        entries[path + "/libhirsel_rt.a"] = archive
    return [DefaultInfo(default_output = ctx.actions.symlinked_dir(ctx.label.name, entries))]

_runtime_archives = rule(
    impl = _runtime_archives_impl,
    attrs = {
        "archives": attrs.dict(attrs.string(), attrs.dep()),
        "_bundler": attrs.source(default = "//tools/buck2:bundle_archive.py"),
        "_llvm": attrs.exec_dep(default = "toolchains//:clang"),
    },
)
