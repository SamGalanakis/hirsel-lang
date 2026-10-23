"""Cargo's profiles (the root Cargo.toml) as rustc flags per Buck2 target platform.

The toolchain's own flags are the dev profile of a workspace member. A release
target platform (`--target-platforms //tools/buck2:release`) appends Cargo's
`[profile.release]`; rustc takes the last of each `-C` option.
"""

RELEASE_FLAGS = [
    "-Copt-level=3",
    "-Cdebuginfo=0",
    "-Cdebug-assertions=no",
    "-Coverflow-checks=no",
    "-Ccodegen-units=16",
]

# tsrs notes/perf-pgo.md: fat LTO requires bitcode in every target rlib.
# Proc macros and build scripts keep host_transition's canonical dev profile.
DIST_PGO = read_root_config("hirsel", "dist_pgo", "off")
if DIST_PGO not in ["off", "generate", "use"]:
    fail("hirsel.dist_pgo must be off, generate or use")

# Keep the same target platform, reservations, metadata and symbol names in
# both passes (tsrs notes/perf-pgo.md:157-169). Only the action inputs change.
PGO_FLAGS = {
    "off": [],
    "generate": ["-Cprofile-generate"],
    "use": ["-Cprofile-use=$(location root//tools/toolchain-dist:profile)", "-Cllvm-args=-pgo-warn-missing-function"],
}
DIST_FLAGS = RELEASE_FLAGS + ["-Cembed-bitcode=yes", "-Clto=fat", "-Ccodegen-units=1"] + PGO_FLAGS[DIST_PGO]

# Keep hot runtime operations at O3; only launch/configuration codecs use Os.
RUNTIME_FLAGS = RELEASE_FLAGS + ["-Cembed-bitcode=yes", "-Clto=fat", "-Ccodegen-units=1"]
COLD_RUNTIME_CRATES = [
    "hirsel_run_config",
    "hirsel_run_schema",
    "serde",
    "serde_core",
    "serde_json",
    "serde_path_to_error",
]

def runtime_profile_flags(crate):
    return select({
        "root//tools/buck2:profile_runtime": ["-Copt-level=s"] if crate in COLD_RUNTIME_CRATES else [],
        "DEFAULT": [],
    })

# Native runtime platforms add LTO and cold-crate size overrides to Cargo's
# panic-abort release settings. Ordinary compiler profiles stay independent.
# Cargo leaves proc macros and build scripts unwinding; so does this. Cargo's
# staticlib is position independent, as the native link (a PIE) requires; the
# prelude's staticlib and its dependencies default to the static model.
PANIC_ABORT = select({
    "root//tools/buck2:panic_abort": ["-Cpanic=abort", "-Crelocation-model=pic"],
    "DEFAULT": [],
})

# [profile.checked] inherits release, with the oracles' assertions and overflow
# checks restored. Host code keeps its canonical dev configuration.
def workspace_profile_flags(host_code = False):
    return select({
        "root//tools/buck2:profile_release": RELEASE_FLAGS,
        "root//tools/buck2:profile_runtime": RUNTIME_FLAGS,
        "root//tools/buck2:profile_dist": DIST_FLAGS,
        "root//tools/buck2:profile_profiling": RELEASE_FLAGS + ["-Cdebuginfo=line-tables-only", "-Cforce-frame-pointers=yes"],
        "root//tools/buck2:profile_checked": RELEASE_FLAGS + ["-Cdebug-assertions=yes", "-Coverflow-checks=yes"],
        "DEFAULT": [],
    }) + ([] if host_code else PANIC_ABORT)

# [profile.dev.package."*"] opt-level = 1: every non-workspace package.
def third_party_profile_flags(host_code = False):
    return select({
        "root//tools/buck2:profile_release": RELEASE_FLAGS,
        "root//tools/buck2:profile_runtime": RUNTIME_FLAGS,
        "root//tools/buck2:profile_dist": DIST_FLAGS,
        "root//tools/buck2:profile_profiling": RELEASE_FLAGS + ["-Cdebuginfo=line-tables-only", "-Cforce-frame-pointers=yes"],
        "root//tools/buck2:profile_checked": RELEASE_FLAGS + ["-Cdebug-assertions=yes", "-Coverflow-checks=yes"],
        "DEFAULT": ["-Copt-level=1"],
    }) + ([] if host_code else PANIC_ABORT)
