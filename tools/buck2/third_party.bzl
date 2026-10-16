"""The rules Reindeer's third-party/BUCK calls (tools/buck2/reindeer.toml)."""

load("@prelude//rust:cargo_buildscript.bzl", "buildscript_run")
load("@prelude//rust:cargo_package.bzl", "cargo", "get_reindeer_platforms")
load("@prelude//utils:selects.bzl", "selects")
load(":profile.bzl", "third_party_profile_flags")

def third_party_http_archive(name, **kwargs):
    # Unpacking a crate archive is a fraction of a second over a file the
    # daemon just downloaded; it runs on the invoking host.
    native.http_archive(name = name, exec_compatible_with = ["root//tools/buck2:host"], **kwargs)

def _compatible(platforms, kwargs):
    compatible = kwargs.pop("target_compatible_with", [])
    if kwargs.get("proc_macro", False) or len(platforms) == 0:
        return compatible
    return selects.apply(
        get_reindeer_platforms(),
        lambda platform: compatible if platform in platforms else ["prelude//:none"],
    )

def _flags(kwargs):
    host_code = kwargs.get("proc_macro", False) or kwargs.get("crate", "").startswith("build_script")
    kwargs["rustc_flags"] = third_party_profile_flags(host_code) + kwargs.get("rustc_flags", [])
    return kwargs

def third_party_rust_library(name, platform = {}, **kwargs):
    compatible = _compatible(platform, kwargs)
    if kwargs.get("proc_macro", False):
        kwargs["incoming_transition"] = "root//tools/buck2:host_transition"
    cargo.rust_library(name = name, platform = platform, target_compatible_with = compatible, **_flags(kwargs))

def third_party_rust_binary(name, platform = {}, **kwargs):
    compatible = _compatible(platform, kwargs)
    if kwargs.get("crate", "").startswith("build_script"):
        kwargs["incoming_transition"] = "root//tools/buck2:host_transition"
    cargo.rust_binary(name = name, platform = platform, target_compatible_with = compatible, **_flags(kwargs))

def third_party_buildscript_run(name, **kwargs):
    buildscript_run(name = name, **kwargs)

def third_party_cxx_library(name, platform = {}, **kwargs):
    compatible = _compatible(platform, kwargs)
    native.cxx_library(name = name, target_compatible_with = compatible, **kwargs)

def third_party_prebuilt_cxx_library(name, platform = {}, **kwargs):
    compatible = _compatible(platform, kwargs)
    native.prebuilt_cxx_library(name = name, target_compatible_with = compatible, **kwargs)
