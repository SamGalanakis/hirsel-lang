"""The rules Reindeer's third-party/BUCK calls (tools/buck2/reindeer.toml)."""

load("@prelude//rust:cargo_buildscript.bzl", "buildscript_run")
load("@prelude//rust:cargo_package.bzl", "cargo", "get_reindeer_platforms")
load("@prelude//utils:selects.bzl", "selects")
load(":profile.bzl", "third_party_profile_flags")
load(":platforms.bzl", "budget_constraint", "compile_request", "pool_budget_name")

def third_party_http_archive(name, **kwargs):
    # The prelude's http_archive unpacks remotely when the archive has a
    # SHA-256 (prelude http_archive/http_archive.bzl `prefer_local`): on the
    # default pool platform, so a second checkout gets an action-cache hit and
    # never downloads or unpacks the crate on this host.
    native.http_archive(name = name, **kwargs)

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

def _compile_constraints(name):
    target = "third-party:" + name
    return select({
        "root//tools/buck2:profile_release": [budget_constraint(pool_budget_name(*compile_request("release", target)))],
        "root//tools/buck2:profile_dist": [budget_constraint(pool_budget_name(*compile_request("release", target)))],
        "root//tools/buck2:profile_profiling": [budget_constraint(pool_budget_name(*compile_request("release", target)))],
        "DEFAULT": [budget_constraint(pool_budget_name(*compile_request("dev", target)))],
    })

def third_party_rust_library(name, platform = {}, **kwargs):
    compatible = _compatible(platform, kwargs)
    if kwargs.get("proc_macro", False):
        kwargs["incoming_transition"] = "root//tools/buck2:host_transition"
    cargo.rust_library(name = name, platform = platform, target_compatible_with = compatible, exec_compatible_with = _compile_constraints(name), **_flags(kwargs))

def third_party_rust_binary(name, platform = {}, **kwargs):
    compatible = _compatible(platform, kwargs)
    if kwargs.get("crate", "").startswith("build_script"):
        kwargs["incoming_transition"] = "root//tools/buck2:host_transition"
    cargo.rust_binary(name = name, platform = platform, target_compatible_with = compatible, exec_compatible_with = _compile_constraints(name), **_flags(kwargs))

def third_party_buildscript_run(name, **kwargs):
    buildscript_run(name = name, **kwargs)

def third_party_cxx_library(name, platform = {}, **kwargs):
    compatible = _compatible(platform, kwargs)
    native.cxx_library(name = name, target_compatible_with = compatible, **kwargs)

def third_party_prebuilt_cxx_library(name, platform = {}, **kwargs):
    compatible = _compatible(platform, kwargs)
    native.prebuilt_cxx_library(name = name, target_compatible_with = compatible, **kwargs)
