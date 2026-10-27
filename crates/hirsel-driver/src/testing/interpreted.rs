//! Simulation over settled immutable TIR. Generated entry text is an ordinary
//! source overlay, checked by the same request graph as the selected package.
use super::{Test, TestDiscovery};
use crate::requests::{
    CompileOperation, CompileRequest, CompileResult, CompilerRequests, TirBatchExecution,
    TirExecution,
};
use hirsel_engine::{
    InterpreterPhase, ToolError,
    loader_inputs::{FrozenInputs, LoaderInputs},
    requests::{OutputReadiness, RequestEngine},
    runtime::QueryRuntime,
};
use hirsel_syntax::DefPath;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

pub(super) struct Executions {
    path: PathBuf,
    inputs: FrozenInputs,
    selector: CompilerRequests,
    entry: Option<(DefPath, Arc<TirExecution>)>,
    roots: Vec<DefPath>,
    batch: Option<Arc<TirBatchExecution>>,
    steps: Option<u64>,
}
impl Executions {
    pub fn new(
        path: &Path,
        discovery: &TestDiscovery,
        selector: CompilerRequests,
        steps: Option<u64>,
    ) -> Result<Self, ToolError> {
        let limit = selector.input_limits().bytes;
        let mut replacements: BTreeMap<PathBuf, Vec<u8>> = BTreeMap::new();
        let mut added = 0usize;
        for (source, path) in discovery.sources.iter().zip(&discovery.source_paths) {
            added = added
                .checked_add(source.text.len() + 1)
                .filter(|&n| n <= limit)
                .ok_or_else(|| {
                    ToolError::Runtime("test wrappers exceed source admission".into())
                })?;
            let path = discovery
                .inputs
                .known_path(path)
                .unwrap_or_else(|| path.clone());
            if !replacements.contains_key(&path) {
                replacements.insert(path.clone(), discovery.inputs.read(&path)?.to_vec());
            }
            let bytes = replacements.get_mut(&path).unwrap();
            bytes.push(b'\n');
            bytes.extend_from_slice(source.text.as_bytes());
        }
        let replacements = replacements
            .into_iter()
            .map(|(path, bytes)| (path, Arc::from(bytes)))
            .collect();
        let inputs = discovery.inputs.replace_sources(&replacements, limit)?;
        let mut roots = discovery
            .tests
            .iter()
            .flat_map(|test| {
                std::iter::once(test.entry.clone())
                    .chain(test.loops.iter().map(|(_, path)| path.clone()))
            })
            .collect::<Vec<_>>();
        roots.sort();
        roots.dedup();
        Ok(Self {
            path: path.to_owned(),
            inputs,
            selector,
            entry: None,
            roots,
            batch: None,
            steps,
        })
    }
    pub fn prepare_all(mut self) -> Result<Self, ToolError> {
        let operation = if self.roots.is_empty() {
            CompileOperation::Check {
                lowering: hirsel_engine::LoweringPolicy::Reachable,
            }
        } else {
            CompileOperation::TirBatch {
                definitions: self.roots.clone(),
            }
        };
        let mut selected = self.selector.select(&CompileRequest {
            path: self.path.clone(),
            inputs: self.inputs.clone(),
            operation,
        })?;
        let completed = {
            let mut runtime = selected.runtime.lock().unwrap_or_else(|e| e.into_inner());
            let request = runtime.request(&selected.roots);
            runtime.wait(request)
        }
        .map_err(|reference| {
            selected
                .output
                .failed(reference)
                .unwrap_or_else(|| ToolError::Runtime("interpreted test TIR is unavailable".into()))
        })?;
        let output = selected.output.ready(&completed)?;
        match &output.result {
            CompileResult::TirBatch(batch) => self.batch = Some(batch.clone()),
            CompileResult::Check(checked) => super::accepted_check(checked)?,
            _ => {
                return Err(ToolError::Runtime(
                    "interpreted test selected another operation".into(),
                ));
            }
        }
        Ok(self)
    }
    pub fn prepare(&mut self, entry: &DefPath) -> Result<Arc<TirExecution>, ToolError> {
        if let Some((selected, input)) = &self.entry
            && selected == entry
        {
            return Ok(input.clone());
        }
        self.entry = None;
        let input = self
            .batch
            .as_ref()
            .ok_or_else(|| ToolError::Runtime("interpreted test batch has not settled".into()))?
            .entry(entry)?;
        self.entry = Some((entry.clone(), input.clone()));
        Ok(input)
    }
    pub fn run(
        &mut self,
        test: &Test,
    ) -> Result<(Arc<TirExecution>, hirsel_engine::ObservedExecution), ToolError> {
        let input = self.prepare(&test.entry)?;
        let mut adapter = hirsel_engine::InterpreterAdapter {
            limits: hirsel_interp::Limits {
                steps: self.steps,
                ..Default::default()
            },
            ..Default::default()
        };
        adapter.prepare(
            input.signature.clone(),
            (*input.runtime).clone(),
            hirsel_engine::SimulationConfig {
                test_seed: test.seed,
                scheduler: test.scheduler.clone(),
                debug: test.simulation_debug,
                ..Default::default()
            },
        );
        let config = hirsel_tir::RunConfiguration {
            entry: test.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let observed = adapter
            .run_observed_query(&config, &mut input.provider(), &input.sources)
            .map_err(|error| match error {
                hirsel_engine::RuntimeError::Query(error) => ToolError::Query {
                    error,
                    phase: Some(hirsel_engine::QueryKind::Interpret),
                    sources: input.sources.clone(),
                },
                hirsel_engine::RuntimeError::Tool(message) => ToolError::Runtime(message),
                hirsel_engine::RuntimeError::Io(error) => ToolError::Io(error),
            })?;
        Ok((input, observed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_selected_test_header_returns_diagnostics_before_wrapper_planning() {
        hirsel_engine::with_compiler_stack(|| {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("suite.json");
            std::fs::write(
                directory.path().join("suite.hir"),
                "@test\nfn invalid(n:u8){}\n",
            )
            .unwrap();
            std::fs::write(&path, r#"{"schema":1,"package":{"name":"invalid","version":"1"},"sources":[{"path":"suite.hir","module":[]}],"dependencies":[]}"#).unwrap();
            let error = match crate::testing::discover_with_selector(
                &path,
                &[],
                None,
                crate::testing::Selection::Tests,
                hirsel_engine::Verify::On,
            ) {
                Ok(_) => panic!("invalid test signature was accepted"),
                Err(error) => error,
            };
            let ToolError::Query {
                error: hirsel_engine::QueryError::Diagnostics(diagnostics),
                ..
            } = error
            else {
                panic!("expected signature diagnostics, got {error:?}");
            };
            assert!(
                diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.rule.as_str() == "LANG-9")
            );
        });
    }
    #[test]
    fn empty_test_selection_still_rejects_an_invalid_unselected_body() {
        hirsel_engine::with_compiler_stack(|| {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("suite.json");
            std::fs::write(
                directory.path().join("suite.hir"),
                "@test\nfn valid(){}\nfn broken()->u8{\"wrong type\"}\n",
            )
            .unwrap();
            std::fs::write(&path, r#"{"schema":1,"package":{"name":"empty","version":"1"},"sources":[{"path":"suite.hir","module":[]}],"dependencies":[]}"#).unwrap();
            let (discovery, mut selector) = crate::testing::discover_with_selector(
                &path,
                &["absent".into()],
                None,
                crate::testing::Selection::Tests,
                hirsel_engine::Verify::On,
            )
            .unwrap();
            assert!(discovery.tests.is_empty());
            assert!(
                crate::testing::check_empty_selection(&path, &discovery, &mut selector).is_err()
            );
            assert!(
                Executions::new(&path, &discovery, selector, None)
                    .unwrap()
                    .prepare_all()
                    .is_err()
            );
        });
    }
    #[test]
    fn immutable_test_entries_keep_captured_text_seed_isolation_and_step_limits() {
        hirsel_engine::with_compiler_stack(|| {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("suite.json");
            let source = directory.path().join("suite.hir");
            std::fs::write(&source, "@test\nfn seeded(t:Test){test.check(t,test.seed(t)==17u64)}\n@test\nfn rejected(t:Test){test.check(t,false)}\n@test\nfn unbounded(){while true {}}\n").unwrap();
            std::fs::write(
                &path,
                serde_json::json!({
                    "schema":1,"package":{"name":"immutable_tests","version":"1"},
                    "sources":[{"path":"suite.hir","module":[]}],"dependencies":[]
                })
                .to_string(),
            )
            .unwrap();
            let (discovery, selector) = crate::testing::discover_with_selector(
                &path,
                &[],
                Some(17),
                crate::testing::Selection::Tests,
                hirsel_engine::Verify::On,
            )
            .unwrap();
            std::fs::write(&source, "invalid newer disk text").unwrap();
            let mut execution = Executions::new(&path, &discovery, selector, Some(1000))
                .unwrap()
                .prepare_all()
                .unwrap();
            let seeded = discovery
                .tests
                .iter()
                .find(|test| test.definition.0.name == "seeded")
                .unwrap();
            let (input, observed) = execution.run(seeded).unwrap();
            assert_eq!(
                observed.outcome.as_ref().unwrap(),
                &hirsel_engine::RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
            );
            let mut different_seed = seeded.clone();
            different_seed.seed = 18;
            let (_, observed) = execution.run(&different_seed).unwrap();
            assert!(
                crate::testing::outcome_failure(observed.outcome.as_ref().unwrap(), &input.sources)
                    .is_some()
            );
            let rejected = discovery
                .tests
                .iter()
                .find(|test| test.definition.0.name == "rejected")
                .unwrap();
            let (input, observed) = execution.run(rejected).unwrap();
            assert!(
                crate::testing::outcome_failure(observed.outcome.as_ref().unwrap(), &input.sources)
                    .is_some()
            );
            let (_, observed) = execution.run(seeded).unwrap();
            assert_eq!(
                observed.outcome.as_ref().unwrap(),
                &hirsel_engine::RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
            );
            let unbounded = discovery
                .tests
                .iter()
                .find(|test| test.definition.0.name == "unbounded")
                .unwrap();
            let (_, observed) = execution.run(unbounded).unwrap();
            assert!(matches!(
                observed.outcome,
                Ok(hirsel_engine::RunOutcome::ToolingLimit(_))
            ));
        });
    }
}
