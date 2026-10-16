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

# The runtime archive is built as Cargo's release profile with
# CARGO_PROFILE_RELEASE_PANIC=abort (crates/hirsel-driver/src/native/link.rs).
# Cargo leaves proc macros and build scripts unwinding; so does this. Cargo's
# staticlib is position independent, as the native link (a PIE) requires; the
# prelude's staticlib and its dependencies default to the static model.
PANIC_ABORT = select({
    "root//tools/buck2:panic_abort": ["-Cpanic=abort", "-Crelocation-model=pic"],
    "DEFAULT": [],
})

def workspace_profile_flags(host_code = False):
    return select({
        "root//tools/buck2:profile_release": RELEASE_FLAGS,
        "DEFAULT": [],
    }) + ([] if host_code else PANIC_ABORT)

# [profile.dev.package."*"] opt-level = 1: every non-workspace package.
def third_party_profile_flags(host_code = False):
    return select({
        "root//tools/buck2:profile_release": RELEASE_FLAGS,
        "DEFAULT": ["-Copt-level=1"],
    }) + ([] if host_code else PANIC_ABORT)
