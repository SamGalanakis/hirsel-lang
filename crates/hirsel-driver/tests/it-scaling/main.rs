//! The scaling probes of this crate: work counters stay linear in program size.
//! They link as their own binary so a compiler edit relinks three smaller
//! harnesses in parallel.
mod scaling_authority_holders;
mod scaling_authority_invalidation;
mod scaling_branch_types;
mod scaling_call_proofs;
mod scaling_cleanup_roots;
mod scaling_cleanup_versions;
mod scaling_dataflow;
mod scaling_definition_inputs;
mod scaling_dependencies;
mod scaling_failure_states;
mod scaling_flow_state;
mod scaling_generic_declarations;
mod scaling_h_places;
mod scaling_headers;
mod scaling_lower_bindings;
mod scaling_lower_decisions;
mod scaling_nested_generics;
mod scaling_origin_dependencies;
mod scaling_ownership_initial;
mod scaling_ownership_slots;
mod scaling_ownership_source_rows;
mod scaling_places;
mod scaling_result_demand;
mod scaling_retirement;
mod scaling_scope_locals;
mod scaling_signatures;
mod scaling_source_rows;
mod scaling_successful_depth;
mod scaling_symbols;
mod scaling_v_roots;
