#[path = "maps/model.rs"]
pub(super) mod model;

#[test]
fn mixed_traces_native_seeds_and_early_exit_cleanup() {
    let traces = model::traces();
    super::rc_maps_native::model_seed_harness(&model::source(&traces), &traces);
}
