"""Canonical Rust configurations for host code and compiler test tools.

Proc macros and build scripts must not acquire the calling target's pool budget:
the same serde metadata can be loaded by compiles using different reservations.
"""

def _host_transition_impl(ctx):
    budget = ctx.attrs.budget[ConstraintSettingInfo]
    panic = ctx.attrs.panic[ConstraintSettingInfo]
    sanitizer = ctx.attrs.sanitizer[ConstraintSettingInfo]
    preserve_sanitizer = ctx.attrs.preserve_sanitizer
    profile = ctx.attrs.profile[ConstraintValueInfo]
    cpu = ctx.attrs.cpu[ConstraintValueInfo]
    configuration_name = ctx.attrs.configuration_name
    def apply(platform):
        constraints = dict(platform.configuration.constraints)
        constraints.pop(budget.label, None)
        constraints.pop(panic.label, None)
        if not preserve_sanitizer:
            constraints.pop(sanitizer.label, None)
        for value in [profile, cpu]:
            constraints[value.setting.label] = value
        return PlatformInfo(
            label = configuration_name,
            configuration = ConfigurationInfo(constraints = constraints, values = platform.configuration.values),
        )
    return [DefaultInfo(), TransitionInfo(impl = apply)]

host_transition = rule(
    impl = _host_transition_impl,
    attrs = {
        "sanitizer": attrs.dep(default = "root//tools/buck2:sanitizer", providers = [ConstraintSettingInfo]),
        "preserve_sanitizer": attrs.bool(default = False),
        "configuration_name": attrs.string(default = "hirsel-rust-host"),
        "budget": attrs.dep(providers = [ConstraintSettingInfo]),
        "panic": attrs.dep(providers = [ConstraintSettingInfo]),
        "profile": attrs.dep(providers = [ConstraintValueInfo]),
        "cpu": attrs.dep(providers = [ConstraintValueInfo]),
    },
    is_configuration_rule = True,
)
