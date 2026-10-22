//! Actual front-end traversal work, independent of query recomputation.
//! These cumulative session-local counters deliberately include compact hits.
//! An edited check with few memo touches can still scan the entire program.
use crate::QueryEngine;

macro_rules! work_stats {
    ($($(#[$doc:meta])* $field:ident),* $(,)?) => {
        #[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize)]
        pub struct WorkStats {
            $($(#[$doc])* pub $field: u64,)*
        }
        impl WorkStats {
            /// Work since an earlier observation of this same engine.
            pub fn since(self, earlier: Self) -> Self {
                Self { $($field: self.$field.saturating_sub(earlier.$field),)* }
            }
        }
    }
}
work_stats! {
    /// Source bytes handed to the lexer (including failed inputs).
    parsed_bytes,
    /// Top-level syntax items returned by the parser.
    parsed_definitions,
    /// Old source items visited to find declarations to replace.
    index_owner_visits,
    /// New item ordinals visited when assigning duplicate occurrences.
    index_occurrence_visits,
    /// Predecessor candidates compared while retaining item anchors.
    index_anchor_visits,
    /// Definitions examined by declaration association preflight.
    association_visits,
    /// Parsed definitions examined to plan the program frontier.
    planning_visits,
    /// Definitions examined to detect generated declarations.
    selection_probe_visits,
    /// Root definitions examined by generated-instance selection.
    selection_root_visits,
    /// Generated-instance selection work-list entries popped.
    selection_walk_visits,
    /// Checked summaries visited to assemble the GEN-5 graph.
    gen5_visits,
    /// Source growth component verdicts computed after topology changes.
    growth_components,
    /// Canonically unchanged growth components reused after topology changes.
    growth_component_hits,
    /// Definitions examined by the package dead-arm lint.
    lint_definition_visits,
    /// Body products read by the package dead-arm lint.
    lint_body_visits,
    /// Pending frontier notifications copied, including already invalidated generations.
    frontier_observations,
    /// Compact owners actually invalidated by a new generation.
    front_invalidations,
    /// Compact selection-summary requests, including hits.
    front_summary_visits,
    /// Compact lowering-result requests, including hits.
    front_lower_visits,
    /// Compact P outcomes requested, including hits.
    front_outcome_visits,
}
impl QueryEngine {
    pub fn work_stats(&self) -> WorkStats {
        self.work
    }
}
