//! Filesystem/event transport over immutable compiler requests. Source
//! observation is shared engine code; this facade keeps only its input cursor,
//! explicit policy and work counters, never a loaded or checked program.
use super::{Host, RequestError};
use crate::{
    requests::{CompileOperation, CompileOutput, CompileRequest, CompileResult, CompilerReadiness},
    session::{FileChange, SessionInputWork, SessionStats},
};
use hirsel_engine::{
    CheckOutput, CompileConfig, Compiler, ToolError, WorkStats,
    loader_inputs::{InputLimits, LoaderInputs, Operation},
    memo::MemoKey,
    requests::{InputObserver, RequestEngine},
    runtime::AdmittedRuntime,
};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
};

pub struct ObservedSession<E>
where
    E: RequestEngine<
            Request = CompileRequest,
            Error = ToolError,
            Runtime = AdmittedRuntime,
            Readiness = CompilerReadiness,
        >,
{
    host: Host<E>,
    compiler: Compiler,
    config: CompileConfig,
    entry: PathBuf,
    observer: InputObserver,
    limits: InputLimits,
    attempts: usize,
    stats: SessionStats,
    input_work: Cell<SessionInputWork>,
    work: WorkStats,
    hold_bytes: u64,
    store_adoptions: u64,
    recomputations: u64,
    polled_metadata: RefCell<BTreeSet<PathBuf>>,
    touched: Vec<MemoKey>,
    delivery_error: Option<std::io::Error>,
    pending_discovery_delivery: bool,
    logical_entry: Option<hirsel_syntax::DefPath>,
}

impl<E> ObservedSession<E>
where
    E: RequestEngine<
            Request = CompileRequest,
            Error = ToolError,
            Runtime = AdmittedRuntime,
            Readiness = CompilerReadiness,
        >,
{
    /// The caller captures one policy and constructs its selector from the same
    /// compiler/config. Limits come from the selector's admitted host policy.
    pub fn new(
        entry: PathBuf,
        compiler: Compiler,
        config: CompileConfig,
        engine: E,
        limits: InputLimits,
        attempts: usize,
    ) -> Self {
        Self {
            host: Host::new(engine),
            compiler: compiler.with_config(config.clone()).with_owned_engines(),
            config,
            entry,
            observer: InputObserver::default(),
            limits,
            attempts,
            stats: SessionStats::default(),
            input_work: Cell::new(SessionInputWork::default()),
            work: WorkStats::default(),
            hold_bytes: 0,
            store_adoptions: 0,
            recomputations: 0,
            polled_metadata: RefCell::new(BTreeSet::new()),
            touched: vec![],
            delivery_error: None,
            pending_discovery_delivery: false,
            logical_entry: None,
        }
    }
    /// Capture a replacement immutable compiler policy while preserving delivered
    /// source observations. Previously returned outputs retain their own owners.
    pub fn replace_engine(
        &mut self,
        compiler: Compiler,
        config: CompileConfig,
        engine: E,
        limits: InputLimits,
        attempts: usize,
    ) {
        self.host = Host::new(engine);
        self.compiler = compiler.with_config(config.clone()).with_owned_engines();
        self.config = config;
        self.limits = limits;
        self.attempts = attempts;
    }
    pub fn entry(&self) -> &Path {
        &self.entry
    }
    pub fn configuration(&self) -> &CompileConfig {
        &self.config
    }
    pub fn stats(&self) -> &SessionStats {
        &self.stats
    }
    pub fn input_work(&self) -> SessionInputWork {
        self.input_work.get()
    }
    pub fn work_stats(&self) -> WorkStats {
        self.work
    }
    /// Retained-body allowance reported by the last selected front owner.
    pub fn hold_bytes(&self) -> u64 {
        self.hold_bytes
    }
    pub fn store_adoptions(&self) -> u64 {
        self.store_adoptions
    }
    pub fn recomputations(&self) -> u64 {
        self.recomputations
    }
    pub fn touched_memos(&self) -> &[MemoKey] {
        &self.touched
    }
    pub fn watched(&self) -> impl Iterator<Item = &Path> {
        std::iter::once(self.entry.as_path())
            .chain(
                self.observer
                    .current()
                    .into_iter()
                    .flat_map(|inputs| inputs.operations().map(Operation::path)),
            )
            .collect::<BTreeSet<_>>()
            .into_iter()
    }
    /// Events are wakeups only. Compare bounded fresh observations, including
    /// negative and directory operations; file timestamps establish nothing.
    pub fn poll(&self) -> Vec<FileChange> {
        let Some(prior) = self.observer.current() else {
            return vec![];
        };
        let Ok(now) = prior.refresh(self.limits) else {
            return vec![FileChange {
                path: self.entry.clone(),
                bytes: None,
            }];
        };
        let (files, bytes) = now.read_work();
        let mut work = self.input_work.get();
        work.polled_files += files;
        work.polled_bytes += bytes;
        self.input_work.set(work);
        let changed = prior.changed_operations(&now);
        *self.polled_metadata.borrow_mut() = changed
            .iter()
            .filter(|operation| !matches!(operation, Operation::Read(_)))
            .map(|operation| operation.path().to_owned())
            .collect();
        changed
            .into_iter()
            .map(|operation| operation.path().to_owned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|path| FileChange {
                bytes: now.read(&path).ok().map(|bytes| bytes.to_vec()),
                path,
            })
            .collect()
    }
    pub fn apply(&mut self, changes: Vec<FileChange>) -> u64 {
        let metadata = self.polled_metadata.take();
        let changes = changes
            .into_iter()
            .filter(|change| {
                let Some(inputs) = self.observer.current() else {
                    return true;
                };
                let observed = inputs.operations().any(
                    |operation| matches!(operation, Operation::Read(path) if path == &change.path),
                );
                metadata.contains(&change.path)
                    || !observed
                    || inputs.read(&change.path).ok().map(|bytes| bytes.to_vec()) != change.bytes
            })
            .collect::<Vec<_>>();
        if changes.is_empty() {
            return self.stats.revision;
        }
        if changes.iter().any(|change| {
            metadata.contains(&change.path)
                || change.bytes.is_none()
                || !self.observer.is_source(&change.path)
        }) {
            self.observer.rediscover();
            self.pending_discovery_delivery = true;
        } else {
            let replacements: BTreeMap<_, Arc<[u8]>> = changes
                .iter()
                .map(|change| {
                    (
                        change.path.clone(),
                        change.bytes.as_ref().unwrap().clone().into(),
                    )
                })
                .collect();
            match self
                .observer
                .apply_sources(&replacements, self.limits.bytes)
            {
                Ok(Some(_)) => {
                    self.stats.replaced += replacements.len() as u64;
                    let mut work = self.input_work.get();
                    work.applied_sources += replacements.len() as u64;
                    work.applied_bytes += replacements
                        .values()
                        .map(|bytes| bytes.len() as u64)
                        .sum::<u64>();
                    self.input_work.set(work);
                }
                Ok(None) => self.pending_discovery_delivery = true,
                Err(error) => {
                    self.delivery_error = Some(error);
                    return self.stats.revision;
                }
            }
        }
        self.stats.revision = self
            .stats
            .revision
            .checked_add(1)
            .expect("input revision exhausted");
        self.stats.revision
    }
    fn observe(&mut self) -> Result<(), ToolError> {
        let prior = self.observer.current().cloned();
        let compiler = &self.compiler;
        let entry = &self.entry;
        let (discovery, observed) = self
            .observer
            .observe(self.limits, self.attempts, |capture| {
                compiler
                    .load_check_package_inputs(entry, capture, false)
                    .map(|program| program.source_paths())
            })
            .map_err(ToolError::Io)?;
        self.stats.loads += observed.work.discoveries;
        let mut work = self.input_work.get();
        work.polled_files += observed.work.file_reads;
        work.polled_bytes += observed.work.file_bytes;
        self.input_work.set(work);
        if let Some(prior) = prior {
            if prior != observed.inputs {
                if !self.pending_discovery_delivery {
                    self.stats.revision = self
                        .stats
                        .revision
                        .checked_add(1)
                        .expect("input revision exhausted");
                }
                if observed.work.discoveries == 0 {
                    self.stats.replaced += prior
                        .changed_operations(&observed.inputs)
                        .iter()
                        .filter_map(|operation| match operation {
                            Operation::Read(path) => {
                                Some(prior.known_path(path).unwrap_or_else(|| path.clone()))
                            }
                            _ => None,
                        })
                        .collect::<BTreeSet<_>>()
                        .len() as u64;
                }
            }
        }
        self.pending_discovery_delivery = false;
        // Notifications describe the previous observation. Once disk has been
        // reobserved, a later same-byte delivery is again an idempotent edit.
        self.polled_metadata.get_mut().clear();
        discovery
    }
    /// Observe the actual filesystem before selecting the shared graph.
    pub fn check(&mut self) -> Result<CheckOutput, ToolError> {
        self.delivery_error = None;
        self.observe()
            .and_then(|()| self.check_applied())
            .or_else(check_error)
    }
    pub fn check_safe(
        &mut self,
    ) -> Result<
        (
            CheckOutput,
            Option<(hirsel_syntax::DefPath, Option<hirsel_engine::UnsafeReach>)>,
        ),
        ToolError,
    > {
        self.request(CompileOperation::SafeCheck)
            .and_then(|output| match &output.result {
                CompileResult::SafeCheck { checked, reach } => Ok((checked.clone(), reach.clone())),
                _ => Err(ToolError::Runtime(
                    "safe-check request returned another operation".into(),
                )),
            })
            .or_else(|error| check_error(error).map(|checked| (checked, None)))
    }
    /// Explicitly delivered source bytes select their own immutable graph,
    /// without polling newer unrelated filesystem state.
    pub fn check_applied(&mut self) -> Result<CheckOutput, ToolError> {
        self.stats.touched = 0;
        self.stats.memo_operations = 0;
        self.touched.clear();
        self.check_applied_inner().or_else(check_error)
    }
    fn check_applied_inner(&mut self) -> Result<CheckOutput, ToolError> {
        let output = self.request_applied(CompileOperation::Check {
            lowering: self.config.lowering,
        })?;
        let CompileResult::Check(checked) = &output.result else {
            return Err(ToolError::Runtime(
                "check request returned another operation".into(),
            ));
        };
        self.stats.touched = output.work.touched_memos.len() as u64;
        self.stats.memo_operations = output.work.memo_operations;
        self.touched = output.work.touched_memos.clone();
        Ok(checked.clone())
    }
    pub fn request(
        &mut self,
        operation: CompileOperation,
    ) -> Result<Arc<CompileOutput>, ToolError> {
        self.delivery_error = None;
        self.observe()?;
        self.request_applied(operation)
    }
    pub fn request_applied(
        &mut self,
        operation: CompileOperation,
    ) -> Result<Arc<CompileOutput>, ToolError> {
        if let Some(error) = self.delivery_error.take() {
            return Err(ToolError::Io(error));
        }
        if self.observer.requires_discovery() {
            self.observe()?;
        }
        let request = CompileRequest {
            path: self.entry.clone(),
            inputs: self.observer.current().unwrap().clone(),
            operation,
        };
        let output = self
            .host
            .request(&request)?
            .wait()
            .map_err(|error| match error {
                RequestError::Output(error) => error,
                RequestError::Cancelled => ToolError::Runtime("compiler request cancelled".into()),
                RequestError::Query(reference) => ToolError::Runtime(format!(
                    "compiler request diagnostic unavailable: {reference:?}"
                )),
            })?;
        self.logical_entry = Some(output.entry.clone());
        self.work.accumulate(output.work.front);
        self.hold_bytes = output.work.hold_bytes;
        self.store_adoptions = self
            .store_adoptions
            .saturating_add(output.work.store_adoptions);
        self.recomputations = self
            .recomputations
            .saturating_add(output.work.recomputations);
        Ok(output)
    }
    pub fn logical_entry(&self) -> Option<&hirsel_syntax::DefPath> {
        self.logical_entry.as_ref()
    }
    pub fn checked_products(
        &mut self,
        canonical: bool,
    ) -> Result<Arc<hirsel_engine::CheckedProducts>, ToolError> {
        let output = self.request_applied(CompileOperation::CheckedProducts {
            lowering: self.config.lowering,
            canonical,
        })?;
        match &output.result {
            CompileResult::CheckedProducts { products, .. } => Ok(products.clone()),
            _ => Err(ToolError::Runtime(
                "checked-products request returned another operation".into(),
            )),
        }
    }
    pub fn recompute_counts(
        &mut self,
        queries: Vec<(hirsel_syntax::DefPath, hirsel_engine::QueryKind)>,
    ) -> Result<Vec<(hirsel_syntax::DefPath, hirsel_engine::QueryKind, u64)>, ToolError> {
        let output = self.request_applied(CompileOperation::RecomputeCounts { queries })?;
        match &output.result {
            CompileResult::RecomputeCounts(counts) => Ok(counts.clone()),
            _ => Err(ToolError::Runtime(
                "counter request returned another operation".into(),
            )),
        }
    }
}

fn check_error(error: ToolError) -> Result<CheckOutput, ToolError> {
    match error {
        ToolError::Query {
            error: hirsel_engine::QueryError::Diagnostics(diagnostics),
            sources,
            ..
        } => Ok(CheckOutput {
            diagnostics: hirsel_engine::select_diagnostics(diagnostics, Some(&sources)),
            sources,
            ..Default::default()
        }),
        error => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_artifact_store::{Action, Address, Domain};
    use hirsel_engine::{QueryError, QueryOutput, runtime::*};
    use hirsel_syntax::ContentHash;
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    struct Task(ProductRef);
    impl QueryTask for Task {
        fn id(&self) -> QueryId {
            QueryId {
                kind: 987,
                address: self.0.address,
            }
        }
        fn weight(&self) -> MemoryWeight {
            MemoryWeight {
                retained: 128,
                ..Default::default()
            }
        }
        fn poll(&mut self, _: &mut dyn QueryContext) -> QueryPoll {
            QueryPoll::Complete(self.0.clone())
        }
    }
    struct Factory(ProductRef);
    impl QueryFactory for Factory {
        fn create(
            &self,
            _: QueryId,
        ) -> Result<Box<dyn QueryTask>, hirsel_artifact_store::Reference> {
            Ok(Box::new(Task(self.0.clone())))
        }
    }
    struct FailingLexer(QueryError);
    impl hirsel_engine::LexerPhase for FailingLexer {
        fn lex(
            &self,
            _: hirsel_span::FileId,
            _: &hirsel_span::SourceMap,
        ) -> Result<QueryOutput<Vec<hirsel_syntax::Token>>, QueryError> {
            Err(self.0.clone())
        }
    }
    /// Test-only actual checker. It consumes frozen inputs and deliberately
    /// computes on every call so a host-side check cache cannot hide in tests.
    struct Checker {
        compiler: Compiler,
        budget: Arc<ProcessBudget>,
        calls: Arc<AtomicUsize>,
        failure: Option<QueryError>,
    }
    impl RequestEngine for Checker {
        type Request = CompileRequest;
        type Error = ToolError;
        type Runtime = AdmittedRuntime;
        type Readiness = CompilerReadiness;
        fn select(
            &mut self,
            request: &CompileRequest,
        ) -> Result<
            hirsel_engine::requests::SelectedRequest<AdmittedRuntime, CompilerReadiness>,
            ToolError,
        > {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let mut program =
                self.compiler
                    .load_check_package_inputs(&request.path, &request.inputs, false)?;
            if let Some(error) = &self.failure {
                program.engine.adapters.lexer = Some(Arc::new(FailingLexer(error.clone())));
            }
            program.parse_sources()?;
            let checked = program
                .check_configured()
                .map_err(|error| program.error(error))?;
            let output = Arc::new(CompileOutput::new(
                program.entry.clone(),
                request.inputs.clone(),
                Default::default(),
                CompileResult::Check(checked),
            ));
            let reference = Domain {
                kind: 987,
                schema: 1,
            }
            .reference(b"checked fixture")
            .unwrap();
            let product = crate::fixture_product_bytes(
                Address(ContentHash::of_bytes(b"fixture root")),
                Action(ContentHash::of_bytes(b"fixture action")),
                reference.domain,
                b"checked fixture",
            );
            let errors = RuntimeErrors {
                resource: reference,
                cycle: reference,
                cancelled: reference,
                protocol: reference,
                panicked: reference,
            };
            let mut runtime = AdmittedRuntime::new(
                hirsel_intern::jobs::Jobs::ONE,
                self.budget
                    .reserve((128 << 20) + (1 << 20))
                    .map_err(ToolError::Io)?,
                errors,
            )
            .map_err(ToolError::Io)?;
            runtime
                .register(987, Arc::new(Factory(product.clone())))
                .unwrap();
            Ok(hirsel_engine::requests::SelectedRequest {
                runtime: Arc::new(Mutex::new(runtime)),
                roots: vec![QueryId {
                    kind: 987,
                    address: product.address,
                }],
                output: CompilerReadiness {
                    read_scope: None,
                    materialize: Arc::new(move |_| Ok(output.clone())),
                    failure: Arc::new(|_| ToolError::Runtime("fixture query failure".into())),
                },
            })
        }
    }
    fn session(
        directory: &Path,
        failure: Option<QueryError>,
        calls: Arc<AtomicUsize>,
    ) -> ObservedSession<Checker> {
        let config = CompileConfig::default();
        let compiler = crate::COMPILER
            .clone()
            .with_config(config.clone())
            .with_owned_engines();
        ObservedSession::new(
            directory.join("hirsel.json"),
            compiler.clone(),
            config,
            Checker {
                compiler,
                budget: ProcessBudget::new(1 << 30),
                calls,
                failure,
            },
            InputLimits {
                bytes: 1 << 20,
                operations: 1000,
            },
            3,
        )
    }
    fn fixture() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("main.hir"), "pub fn main() {}\n").unwrap();
        std::fs::write(directory.path().join("hirsel.json"), r#"{"schema":1,"package":{"name":"observed","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        directory
    }
    #[test]
    fn observed_session_keeps_explicit_delivery_and_loader_watch_semantics_without_a_check_cache() {
        hirsel_engine::with_compiler_stack(|| {
            let directory = fixture();
            let calls = Arc::new(AtomicUsize::new(0));
            let mut session = session(directory.path(), None, calls.clone());
            assert!(session.check().unwrap().diagnostics.is_empty());
            assert!(session.check().unwrap().diagnostics.is_empty());
            assert_eq!(calls.load(Ordering::SeqCst), 2);
            assert_eq!(session.stats().loads, 1);
            let source = directory.path().join("main.hir");
            let edited = b"pub fn main() { missing }\n".to_vec();
            session.apply(vec![FileChange {
                path: source.clone(),
                bytes: Some(edited.clone()),
            }]);
            let work = session.input_work();
            assert!(!session.check_applied().unwrap().diagnostics.is_empty());
            assert_eq!(session.input_work(), work);
            let revision = session.stats().revision;
            session.apply(vec![FileChange {
                path: source.clone(),
                bytes: Some(edited),
            }]);
            assert_eq!(session.stats().revision, revision);
            assert!(session.check().unwrap().diagnostics.is_empty());
            assert_eq!(session.stats().loads, 1);
            std::fs::write(&source, "pub fn main(").unwrap();
            let rejected = session.check().unwrap();
            assert!(!rejected.diagnostics.is_empty());
            assert!(session.watched().any(|path| path == source));
        });
    }
    #[cfg(unix)]
    #[test]
    fn observed_session_applies_same_byte_alias_retarget_as_loader_change() {
        hirsel_engine::with_compiler_stack(|| {
            let directory = fixture();
            let source = directory.path().join("main.hir");
            let first = directory.path().join("first.hir");
            let second = directory.path().join("second.hir");
            std::fs::rename(&source, &first).unwrap();
            std::fs::copy(&first, &second).unwrap();
            std::os::unix::fs::symlink(&first, &source).unwrap();
            let mut session = session(directory.path(), None, Arc::new(AtomicUsize::new(0)));
            assert!(session.check().unwrap().diagnostics.is_empty());
            let revision = session.stats().revision;
            std::fs::remove_file(&source).unwrap();
            std::os::unix::fs::symlink(&second, &source).unwrap();
            let changes = session.poll();
            assert!(
                !changes.is_empty(),
                "a new canonical target is an input change even with equal source bytes"
            );
            session.apply(changes);
            assert!(session.check_applied().unwrap().diagnostics.is_empty());
            assert_eq!(session.stats().loads, 2);
            assert_eq!(session.stats().revision, revision + 1);
            assert!(session.poll().is_empty());
        });
    }

    #[cfg(unix)]
    #[test]
    fn observed_session_disk_refresh_consumes_prior_alias_notification() {
        hirsel_engine::with_compiler_stack(|| {
            let directory = fixture();
            let source = directory.path().join("main.hir");
            let first = directory.path().join("first.hir");
            let second = directory.path().join("second.hir");
            let bytes = std::fs::read(&source).unwrap();
            std::fs::rename(&source, &first).unwrap();
            std::fs::copy(&first, &second).unwrap();
            std::os::unix::fs::symlink(&first, &source).unwrap();
            let mut session = session(directory.path(), None, Arc::new(AtomicUsize::new(0)));
            assert!(session.check().unwrap().diagnostics.is_empty());
            std::fs::remove_file(&source).unwrap();
            std::os::unix::fs::symlink(&second, &source).unwrap();
            assert!(!session.poll().is_empty());
            assert!(session.check().unwrap().diagnostics.is_empty());
            let revision = session.stats().revision;
            session.apply(vec![FileChange {
                path: source,
                bytes: Some(bytes),
            }]);
            assert_eq!(session.stats().revision, revision);
            assert_eq!(session.stats().loads, 2);
        });
    }

    #[test]
    fn observed_session_preserves_non_diagnostic_loader_query_errors() {
        hirsel_engine::with_compiler_stack(|| {
            for expected in [
                QueryError::NotImplemented,
                QueryError::Operational(hirsel_types::OperationalFailure {
                    operation: "fixture-transport".into(),
                    message: "connection lost".into(),
                }),
            ] {
                let directory = fixture();
                let mut session = session(
                    directory.path(),
                    Some(expected.clone()),
                    Arc::new(AtomicUsize::new(0)),
                );
                let Err(ToolError::Query { error, .. }) = session.check() else {
                    panic!("non-diagnostic failure was flattened")
                };
                assert_eq!(error, expected);
            }
        });
    }
}
