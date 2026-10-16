"""Canonical Rust host configuration.

Proc macros and build scripts must not acquire the calling target's pool budget:
the same serde metadata can be loaded by compiles using different reservations.
"""

def _host_transition_impl(ctx):
    budget = ctx.attrs.budget[ConstraintSettingInfo]
    panic = ctx.attrs.panic[ConstraintSettingInfo]
    profile = ctx.attrs.profile[ConstraintValueInfo]
    cpu = ctx.attrs.cpu[ConstraintValueInfo]
    def apply(platform):
        constraints = dict(platform.configuration.constraints)
        constraints.pop(budget.label, None)
        constraints.pop(panic.label, None)
        for value in [profile, cpu]:
            constraints[value.setting.label] = value
        return PlatformInfo(
            label = "hirsel-rust-host",
            configuration = ConfigurationInfo(constraints = constraints, values = platform.configuration.values),
        )
    return [DefaultInfo(), TransitionInfo(impl = apply)]

host_transition = rule(
    impl = _host_transition_impl,
    attrs = {
        "budget": attrs.dep(providers = [ConstraintSettingInfo]),
        "panic": attrs.dep(providers = [ConstraintSettingInfo]),
        "profile": attrs.dep(providers = [ConstraintValueInfo]),
        "cpu": attrs.dep(providers = [ConstraintValueInfo]),
    },
    is_configuration_rule = True,
)
