load("@prelude//rust:rust_toolchain.bzl", "PanicRuntime", "RustSanitizer", "RustToolchainInfo")

def _rust(ctx):
    core_prefix = "lib/rustlib/{}/lib/libcore-".format(ctx.attrs.target_triple)
    if ctx.attrs.sanitizer and not any([p.startswith(core_prefix) and p.endswith(".rlib") for p in ctx.attrs.srcs]):
        fail("ThreadSanitizer needs its instrumented std: run python3 tools/buck2/prepare_tsan.py")
    tree = ctx.actions.copied_dir("rust", ctx.attrs.srcs)
    return [
        DefaultInfo(),
        RustToolchainInfo(
            sanitizer = RustSanitizer(ctx.attrs.sanitizer) if ctx.attrs.sanitizer else None,
            compiler = RunInfo(args = cmd_args(tree.project("bin/rustc"), hidden = tree)),
            rustdoc = RunInfo(args = cmd_args(tree.project("bin/rustdoc"), hidden = tree)),
            clippy_driver = RunInfo(args = cmd_args(tree.project("bin/clippy-driver"), hidden = tree)),
            # Prelude actions name the declared sysroot instead of discovering it
            # from the executable's location.
            sysroot_path = tree,
            default_edition = "2024",
            panic_runtime = PanicRuntime("unwind"),
            # Cargo's dev profile for workspace members ([profile.dev] in Cargo.toml):
            # debug assertions and overflow checks on, line tables only. Profile
            # flags that differ per package are added by tools/buck2/defs.bzl.
            rustc_flags = [
                "-Cdebuginfo=line-tables-only",
                "-Cdebug-assertions=yes",
                "-Coverflow-checks=yes",
                "-Cembed-bitcode=no",
                "-Cstrip=none",
            ] + (["-Crelocation-model=pic"] if ctx.attrs.sanitizer else []),
            extra_rustc_flags = [],
            # The prelude gates -Zno-codegen metadata pipelining on this field;
            # stable rustc runs it under RUSTC_BOOTSTRAP.
            nightly_features = True,
            rustc_env = {"PATH": "/usr/bin:/bin"},
            rustc_target_triple = ctx.attrs.target_triple,
            doctests = False,
        ),
    ]

rust_toolchain = rule(
    impl = _rust,
    attrs = {
        "sanitizer": attrs.option(attrs.string(), default = None),
        "srcs": attrs.dict(attrs.string(), attrs.source()),
        "target_triple": attrs.string(),
    },
    is_toolchain_rule = True,
)
