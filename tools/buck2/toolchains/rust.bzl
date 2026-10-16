load("@prelude//rust:rust_toolchain.bzl", "PanicRuntime", "RustToolchainInfo")

def _rust(ctx):
    tree = ctx.actions.copied_dir("rust", ctx.attrs.srcs)
    return [
        DefaultInfo(),
        RustToolchainInfo(
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
            ],
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
        "srcs": attrs.dict(attrs.string(), attrs.source()),
        "target_triple": attrs.string(),
    },
    is_toolchain_rule = True,
)
