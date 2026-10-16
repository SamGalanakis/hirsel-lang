"""The pinned LLVM 21.1.8 tree (tools/buck2/bootstrap_llvm.py) as action inputs."""

load("@prelude//cxx:cxx_toolchain_types.bzl", "LinkerType")
load("@prelude//toolchains:cxx.bzl", "CxxToolsInfo")

def _tree(ctx):
    tree = ctx.actions.copied_dir(ctx.label.name, ctx.attrs.srcs)
    return [DefaultInfo(default_output = tree)]

llvm_tree = rule(impl = _tree, attrs = {"srcs": attrs.dict(attrs.string(), attrs.source())})

def _source_files(ctx):
    return [DefaultInfo(default_outputs = ctx.attrs.srcs)]

# Source files as they are, at their repository paths in an action's inputs:
# a binary's `$ORIGIN`-relative RUNPATH reaches them (tools/buck2/defs.bzl).
source_files = rule(impl = _source_files, attrs = {"srcs": attrs.list(attrs.source())})

def _tools(ctx):
    # The tree's wrappers run clang with its bundled libraries on the loader
    # path (tools/buck2/bootstrap_llvm.py); the whole tree is a hidden input.
    llvm = ctx.attrs.tree[DefaultInfo].default_outputs[0]
    clang = cmd_args(llvm.project("bin/hirsel-clang"), hidden = llvm)
    clangxx = cmd_args(llvm.project("bin/hirsel-clang++"), hidden = llvm)
    return [
        DefaultInfo(),
        CxxToolsInfo(
            compiler = clang,
            compiler_type = "clang",
            cxx_compiler = clangxx,
            asm_compiler = clang,
            asm_compiler_type = "clang",
            rc_compiler = None,
            cvtres_compiler = None,
            archiver = cmd_args(llvm.project("bin/hirsel-llvm-ar"), hidden = llvm),
            archiver_type = "gnu",
            # The C driver: Rust links need no libstdc++, which a minimal
            # worker image does not carry for linking.
            linker = clang,
            linker_type = LinkerType("gnu"),
        ),
    ]

llvm_cxx_tools = rule(impl = _tools, attrs = {"tree": attrs.exec_dep()})
