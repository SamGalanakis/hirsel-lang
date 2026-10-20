//! Schedule-sensitive access candidates, not a happens-before race detector.
//! Hirsel is data-race-free; these pairs help explain application-level races.
use super::Library;
use crate::{OperationContext, TraceEvent, observation::SourceSite};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Access {
    pub task: Vec<u32>,
    /// Access occurrence within this task, independent of worker placement.
    pub occurrence: u64,
    pub operation: String,
    pub source: SourceSite,
    pub file: String,
    pub write: bool,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RaceCandidate {
    pub object_kind: String,
    pub object: u64,
    pub first: Access,
    pub second: Access,
}
impl std::fmt::Display for RaceCandidate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "race candidate on {} #{} (schedule-sensitive accesses):",
            self.object_kind, self.object
        )?;
        for access in [&self.first, &self.second] {
            writeln!(
                f,
                "  task {:?}, access {}: {} at {}:{}:{} ({})",
                access.task,
                access.occurrence,
                access.operation,
                access.file,
                access.source.line,
                access.source.column,
                access.source.definition
            )?;
        }
        Ok(())
    }
}

/// Keep each task's last read and write, so intervening reads never hide a write.
/// The report is bounded; it is a debugging hint, not an exhaustive proof.
type TaskAccesses = BTreeMap<Vec<u32>, [Option<Access>; 2]>;
#[derive(Default)]
pub(super) struct RaceTracker {
    accesses: BTreeMap<(&'static str, u64), TaskAccesses>,
    occurrences: BTreeMap<Vec<u32>, u64>,
    reported: Vec<RaceCandidate>,
}
const MAX_CANDIDATES: usize = 16;
impl RaceTracker {
    fn access(
        &mut self,
        kind: &'static str,
        object: u64,
        mut access: Access,
    ) -> Vec<RaceCandidate> {
        if self.reported.len() == MAX_CANDIDATES {
            return vec![];
        }
        let occurrence = self.occurrences.entry(access.task.clone()).or_default();
        *occurrence += 1;
        access.occurrence = *occurrence;
        let previous = self.accesses.entry((kind, object)).or_default();
        let mut found = vec![];
        for (task, slots) in previous.iter() {
            if *task == access.task {
                continue;
            }
            for first in slots.iter().flatten() {
                if !first.write && !access.write {
                    continue;
                }
                let candidate = RaceCandidate {
                    object_kind: kind.into(),
                    object,
                    first: first.clone(),
                    second: access.clone(),
                };
                // Repeated loop iterations at the same pair of source sites need one hint.
                if self.reported.iter().any(|old| {
                    old.object_kind == candidate.object_kind
                        && old.object == object
                        && old.first.task == candidate.first.task
                        && old.second.task == candidate.second.task
                        && old.first.source == candidate.first.source
                        && old.second.source == candidate.second.source
                        && old.first.operation == candidate.first.operation
                        && old.second.operation == candidate.second.operation
                }) {
                    continue;
                }
                self.reported.push(candidate.clone());
                found.push(candidate);
                if self.reported.len() == MAX_CANDIDATES {
                    return found;
                }
            }
        }
        let slot = usize::from(access.write);
        let task = access.task.clone();
        previous.entry(task).or_default()[slot] = Some(access);
        found
    }
}
impl Library {
    pub(super) fn race_access(
        &mut self,
        kind: &'static str,
        object: u64,
        operation: &str,
        write: bool,
        context: &mut OperationContext<'_>,
    ) {
        if !self.simulation.config.debug.races {
            return;
        }
        let Some(anchor) = context.anchor() else {
            return;
        };
        let Some(source) = anchor.source_site(context.sources) else {
            return;
        };
        let Some(file) = context.sources.file(anchor.site.file) else {
            return;
        };
        let access = Access {
            task: self.tasks.path(),
            occurrence: 0,
            operation: operation.into(),
            source,
            file: file.path.display().to_string(),
            write,
        };
        context.trace.extend(
            self.races
                .access(kind, object, access)
                .into_iter()
                .map(TraceEvent::RaceCandidate),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn access(task: u32, operation: &str, write: bool) -> Access {
        Access {
            task: vec![task],
            occurrence: 0,
            operation: operation.into(),
            source: SourceSite {
                definition: hirsel_syntax::DefPath::core("example"),
                line: task + 1,
                column: 1,
            },
            file: "example.hir".into(),
            write,
        }
    }
    #[test]
    fn conflicts_name_both_tasks_and_operations_without_losing_writes_to_reads() {
        let mut tracker = RaceTracker::default();
        assert!(
            tracker
                .access("Atomic", 1, access(0, "atomic.store", true))
                .is_empty()
        );
        assert!(
            tracker
                .access("Atomic", 1, access(0, "atomic.load", false))
                .is_empty()
        );
        let found = tracker.access("Atomic", 1, access(1, "atomic.load", false));
        assert_eq!(found.len(), 1);
        let text = found[0].to_string();
        for name in [
            "Atomic #1",
            "task [0]",
            "task [1]",
            "atomic.store",
            "atomic.load",
            "example.hir:1:1",
            "example.hir:2:1",
        ] {
            assert!(text.contains(name), "{text}");
        }
        assert!(
            tracker
                .access("Atomic", 1, access(1, "atomic.load", false))
                .is_empty()
        );
    }
    #[test]
    fn reads_same_task_and_distinct_objects_are_not_candidates() {
        let mut tracker = RaceTracker::default();
        tracker.access("Atomic", 1, access(0, "atomic.load", false));
        assert!(
            tracker
                .access("Atomic", 1, access(1, "atomic.load", false))
                .is_empty()
        );
        assert!(
            tracker
                .access("Atomic", 2, access(1, "atomic.store", true))
                .is_empty()
        );
        assert!(
            tracker
                .access("Shared", 1, access(1, "shared.release", true))
                .is_empty()
        );
        assert!(
            tracker
                .access("Atomic", 2, access(1, "atomic.load", false))
                .is_empty()
        );
    }
    #[test]
    fn reports_are_bounded() {
        let mut tracker = RaceTracker::default();
        let mut count = 0;
        for task in 0..100 {
            count += tracker
                .access("Atomic", 1, access(task, "atomic.store", true))
                .len();
        }
        assert_eq!(count, MAX_CANDIDATES);
    }
}
