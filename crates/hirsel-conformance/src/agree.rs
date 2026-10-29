//! A finite catalogue of single syntax edits, replayed through the real driver.
use hirsel_engine::{CheckCompletion, DefinitionQueries, QueryEngine, QueryError, ToolError};
use hirsel_span::Span;
use hirsel_syntax::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    panic::{AssertUnwindSafe, catch_unwind},
    path::Path,
    sync::{
        Condvar, Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Classification {
    Rejected,
    Valid,
    Unavailable,
    RuntimeLimit,
    Bug { kind: String, detail: String },
}
impl Classification {
    fn key(&self) -> Option<&str> {
        match self {
            Self::Bug { kind, .. } => Some(kind),
            _ => None,
        }
    }
}
/// Variants can introduce infinite loops. A deterministic instruction budget
/// bounds execution without confusing ordinary limits with interpreter bugs.
const STEPS: u64 = 10_000;
fn engine() -> QueryEngine {
    let mut adapters = hirsel_driver::compiler_adapters();
    let mut interpreter = hirsel_engine::InterpreterAdapter::default();
    interpreter.limits.steps = Some(STEPS);
    adapters.interpreter = Some(Box::new(interpreter));
    // The environment hashes the spec; every variant shares one.
    static ENVIRONMENT: OnceLock<hirsel_engine::QueryEnvironment> = OnceLock::new();
    let environment = ENVIRONMENT.get_or_init(hirsel_engine::default_environment);
    QueryEngine::new(environment.clone(), adapters)
        .with_config(crate::compiler::verifying_compiler().configuration())
}
fn violation(operation: &str, message: &str) -> Classification {
    let kind = if operation.contains("TIR validation") {
        // Driver retains the entire report; cluster by its first violation, without
        // slot/body numbers or the enclosing definition's spelling.
        let first = message.split("kind: ").nth(1).unwrap_or(message);
        let kind = first
            .split(['(', '{', ',', ' '])
            .next()
            .unwrap_or("unknown");
        let entity = first
            .split("entity: \"")
            .nth(1)
            .and_then(|s| s.split('"').next());
        entity.map_or_else(|| format!("TIR/{kind}"), |e| format!("TIR/{kind}/{e}"))
    } else {
        format!("internal/{operation}")
    };
    Classification::Bug {
        kind,
        detail: format!("{operation}: {message}"),
    }
}
fn tool_error(error: ToolError) -> Classification {
    match error {
        ToolError::Query { error, .. } => query_error(error),
        other => violation("interpretation", &other.to_string()),
    }
}
fn query_error(error: QueryError) -> Classification {
    match error {
        QueryError::Diagnostics(_) => Classification::Rejected,
        QueryError::NotImplemented => Classification::Unavailable,
        QueryError::Operational(e) => violation(&e.operation, &e.message),
    }
}
/// What a variant's run executes. A failure path is a run like any other, so
/// `fail` and `blocked` programs run their entry as `run` programs do.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Execution {
    /// Checking only: an `accept` program.
    None,
    /// The entry: a `run`, `fail` or `blocked` program.
    Entry,
    /// Each `@test` function, as `hirsel test` launches it: a `test` program.
    Tests,
}
impl Execution {
    pub fn of(expect: &crate::Expect) -> Self {
        match expect {
            crate::Expect::Run { .. }
            | crate::Expect::Fail { .. }
            | crate::Expect::Blocked { .. }
            | crate::Expect::Signaled { .. } => Self::Entry,
            crate::Expect::Test => Self::Tests,
            crate::Expect::Accept | crate::Expect::Reject { .. } | crate::Expect::RefuseEntry => {
                Self::None
            }
        }
    }
    /// A reproducer carries no header: one that declares a `@test` function
    /// runs its tests, any other its entry.
    pub fn of_source(source: &str) -> Self {
        if source.lines().any(|line| line.trim() == "@test") {
            Self::Tests
        } else {
            Self::Entry
        }
    }
}

/// Runtime language failures and artificial execution limits are ordinary variant
/// outcomes. Only interpreter/tool errors are bugs; changed stdout is expected.
/// A variant that runs is run again under checked execution, which must observe
/// the same outcome (failure rule and site included) and output, or trap; either
/// difference is a bug.
pub fn classify(source: &str, execution: Execution) -> Classification {
    classify_native(source, execution, None)
}

/// The runtime extension: the configurations a
/// variant that the interpreter runs is also run under, on the heap executor
/// and as native code, and what they did.
pub struct Native {
    pub matrix: Vec<hirsel_driver::native::Configuration>,
    pub toolchain: hirsel_driver::native::Toolchain,
    pub dir: std::path::PathBuf,
    /// The baseline's named exclusions: a program's variants do not run
    /// under a configuration the program itself is listed for.
    pub exclusions: crate::native::Exclusions,
    pub stats: std::sync::Mutex<NativeStats>,
    next: std::sync::atomic::AtomicUsize,
    pub selection: Option<crate::native::diet::Selection>,
    seed: u64,
}

#[derive(Default, Debug, Clone)]
pub struct NativeStats {
    /// Variant and configuration pairs that agreed with the reference.
    pub agreed: usize,
    /// Pairs a stage reported unavailable, by reason.
    pub unavailable: BTreeMap<String, usize>,
}

impl Native {
    pub fn new(
        matrix: Vec<hirsel_driver::native::Configuration>,
        toolchain: hirsel_driver::native::Toolchain,
        dir: std::path::PathBuf,
        exclusions: crate::native::Exclusions,
    ) -> Self {
        Self {
            matrix,
            toolchain,
            dir,
            exclusions,
            stats: Default::default(),
            next: Default::default(),
            selection: None,
            seed: 0,
        }
    }
    pub fn with_selection(mut self, selection: crate::native::diet::Selection, seed: u64) -> Self {
        self.selection = Some(selection);
        self.seed = seed;
        self
    }
    /// The configurations `path`'s variants run under: those the baseline
    /// lists no exclusion of `path` for, by the configuration's seedless name.
    fn for_program(&self, path: &str) -> Vec<hirsel_driver::native::Configuration> {
        let listed = |configuration: String| {
            self.exclusions
                .contains_key(&(path.to_string(), configuration))
        };
        if listed(crate::native::SNAPSHOT.into()) {
            return Vec::new();
        }
        let matrix = if self.selection.is_some() {
            hirsel_driver::native::pairwise_matrix(&self.matrix, path, self.seed)
        } else {
            self.matrix.clone()
        };
        matrix
            .iter()
            .filter(|c| {
                let mut baseline = (*c).clone();
                baseline.options.launch = Default::default();
                !listed(baseline.name())
            })
            .cloned()
            .collect()
    }
    /// Compare the engine's snapshot under `matrix` with `reference`.
    fn compare(
        &self,
        engine: &mut QueryEngine,
        matrix: &[hirsel_driver::native::Configuration],
        reference: &hirsel_driver::native::Observation,
        entry: &hirsel_engine::DefPath,
        fixture: &hirsel_driver::native::Fixture,
    ) -> Option<Classification> {
        use hirsel_driver::native::Outcome;
        let unavailable = |reason: String| {
            *self
                .stats
                .lock()
                .unwrap()
                .unavailable
                .entry(reason)
                .or_default() += 1;
        };
        let validated = match engine
            .export_native_input()
            .and_then(|input| engine.import_native_input(input))
        {
            Ok(validated) => validated,
            Err(_) => {
                unavailable("snapshot: no native input".into());
                return None;
            }
        };
        let needs_files = matrix
            .iter()
            .any(|c| crate::native::Class::of(c) == crate::native::Class::Llvm);
        let dir = self.dir.join(format!(
            "v{}",
            self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        if needs_files {
            std::fs::create_dir_all(&dir).expect("scratch directory");
        }
        let outcomes = hirsel_driver::native::differential_in(
            &validated,
            entry,
            reference,
            matrix,
            &self.toolchain,
            &dir,
            fixture,
        );
        if needs_files {
            let _ = std::fs::remove_dir_all(&dir);
        }
        let mut bug = None;
        for ((name, outcome), configuration) in outcomes.into_iter().zip(matrix) {
            let class = crate::native::Class::of(configuration).name();
            let found = match &outcome {
                Outcome::Agree => {
                    self.stats.lock().unwrap().agreed += 1;
                    continue;
                }
                Outcome::Mismatch(actual) => Some((
                    format!("native/{class}/mismatch"),
                    format!(
                        "under {name}: expected\n{}actual\n{}",
                        reference.render(),
                        actual.render()
                    ),
                )),
                Outcome::Unavailable(error) => {
                    let exclusion = crate::native::exclusion(&outcome).expect("not agreement");
                    match exclusion.class.as_str() {
                        // A heap violation or a stage validator's refusal of
                        // a pass's output is a compiler bug in a valid program.
                        "known-bug" | "compiler-error" => Some((
                            format!("native/{class}/{}", exclusion.reason),
                            format!("under {name}: {error}"),
                        )),
                        _ => {
                            unavailable(exclusion.reason);
                            None
                        }
                    }
                }
            };
            if let (None, Some((kind, detail))) = (&bug, found) {
                bug = Some(Classification::Bug { kind, detail });
            }
        }
        bug
    }
}

/// `classify`, with the valid executed variant also compared under `native`'s
/// configurations.
pub fn classify_native(
    source: &str,
    execution: Execution,
    native: Option<(&Native, &[hirsel_driver::native::Configuration])>,
) -> Classification {
    classify_package_native(source, execution, native, None, 0)
}

/// Replay one exact program, including its package sidecar, through agreement.
/// Explicit reproducers use the requested matrix without corpus exclusions.
pub fn replay_program_with_hash_seed(
    path: &Path,
    native: Option<&Native>,
    hash_seed: u64,
) -> Result<Classification, String> {
    let source = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(classify_package_native(
        &source,
        Execution::of_source(&source),
        native.map(|n| (n, n.matrix.as_slice())),
        Some(path),
        hash_seed,
    ))
}

/// `source` replaces the primary source of `path`'s program when there is a
/// path (a package sidecar's program, or a test program: discovery needs a
/// loaded program); otherwise it is checked alone.
fn classify_package_native(
    source: &str,
    execution: Execution,
    native: Option<(&Native, &[hirsel_driver::native::Configuration])>,
    path: Option<&Path>,
    hash_seed: u64,
) -> Classification {
    match catch_unwind(AssertUnwindSafe(|| {
        let (mut program, mut bare) = (None, None);
        let (checked, entry) = if let Some(path) = path {
            let mut loaded = match crate::compiler::verifying_compiler()
                .load_program_source(path, Some(source))
            {
                Ok(p) => p,
                Err(e) => return tool_error(e),
            };
            let mut interpreter = hirsel_engine::InterpreterAdapter::default();
            interpreter.limits.steps = Some(STEPS);
            loaded.engine.adapters.interpreter = Some(Box::new(interpreter));
            let checked = loaded.check(hirsel_engine::LoweringPolicy::Reachable);
            let entry = loaded.entry.clone();
            program = Some(loaded);
            (checked, entry)
        } else {
            let mut engine = engine();
            let context = hirsel_engine::default_context();
            let file = engine
                .sources
                .add("agree.hir", source.into())
                .expect("corpus-sized source");
            let checked = hirsel_engine::check_progress(&mut engine, file, &context);
            bare = Some(engine);
            (checked, crate::selected_entry(source, &context))
        };
        match checked {
            Err(e) => return query_error(e),
            Ok(output) => {
                if output
                    .diagnostics
                    .iter()
                    .any(|d| d.severity == hirsel_diag::Severity::Error)
                {
                    return Classification::Rejected;
                }
                if output.completion != CheckCompletion::Complete {
                    return Classification::Unavailable;
                }
            }
        }
        match execution {
            Execution::None => {}
            Execution::Entry => {
                let engine = match (&mut program, &mut bare) {
                    (Some(program), _) => &mut program.engine,
                    (None, Some(engine)) => engine,
                    (None, None) => unreachable!("one engine checked the source"),
                };
                let (simulation, fixture) =
                    match (crate::simulation(source), crate::fixture(source)) {
                        (Ok(simulation), Ok(fixture)) => (simulation, fixture),
                        (Err(error), _) | (_, Err(error)) => return violation("fixture", &error),
                    };
                let config = hirsel_engine::RunConfiguration {
                    entry,
                    hash_seed,
                    inputs: vec![],
                };
                if let Some(outcome) =
                    compare_engines(engine, &config, simulation, native, &fixture)
                {
                    return outcome;
                }
            }
            Execution::Tests => {
                let Some(program) = &mut program else {
                    return violation("fixture", "a test program is loaded through its file");
                };
                let tests = match hirsel_driver::testing::discover(
                    program,
                    &[],
                    None,
                    hirsel_driver::testing::Selection::Tests,
                ) {
                    Ok(tests) => tests,
                    Err(e) => return tool_error(e),
                };
                // `hirsel test`'s launch: a fresh Test root with the test's
                // own seed and no other grant (testing::execute).
                for test in &tests {
                    let config = hirsel_engine::RunConfiguration {
                        entry: test.entry.clone(),
                        hash_seed,
                        inputs: vec![],
                    };
                    let simulation = hirsel_engine::SimulationConfig {
                        test_seed: test.seed,
                        ..Default::default()
                    };
                    let matrix = native.map(|(native, matrix)| {
                        let mut matrix = matrix.to_vec();
                        for configuration in &mut matrix {
                            configuration.options.launch.test_seed = test.seed;
                        }
                        (native, matrix)
                    });
                    if let Some(outcome) = compare_engines(
                        &mut program.engine,
                        &config,
                        simulation,
                        matrix
                            .as_ref()
                            .map(|(native, matrix)| (*native, matrix.as_slice())),
                        &Default::default(),
                    ) {
                        return outcome;
                    }
                }
            }
        }
        Classification::Valid
    })) {
        Ok(result) => result,
        Err(payload) => {
            let message = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).into()))
                .unwrap_or_else(|| "non-string panic".into());
            Classification::Bug {
                kind: format!("panic/{message}"),
                detail: message,
            }
        }
    }
}

/// Run `config` normally, then under checked execution, then under `native`'s
/// matrix. None when every engine observed the same; otherwise the variant's
/// classification (a step limit or an unavailable phase ends it as well).
fn compare_engines(
    engine: &mut QueryEngine,
    config: &hirsel_engine::RunConfiguration,
    simulation: hirsel_engine::SimulationConfig,
    native: Option<(&Native, &[hirsel_driver::native::Configuration])>,
    fixture: &hirsel_driver::native::Fixture,
) -> Option<Classification> {
    let normal = match hirsel_engine::run_observed_snapshot(engine, config, simulation.clone()) {
        Ok(observed) => observed,
        Err(e) => return Some(tool_error(e)),
    };
    match &normal.outcome {
        Ok(hirsel_engine::RunOutcome::ToolingLimit(_)) => {
            return Some(Classification::RuntimeLimit);
        }
        Ok(_) => {}
        Err(hirsel_engine::RuntimeError::Query(error)) => {
            return Some(query_error(error.clone()));
        }
        Err(e) => return Some(violation("interpretation", &format!("{e:?}"))),
    }
    let checked = match hirsel_engine::run_observed_checked(engine, config, simulation) {
        Ok(observed) => observed,
        Err(e) => return Some(tool_error(e)),
    };
    if let Some(detail) = crate::compiler::checked_divergence(&normal, &checked) {
        let kind = match &checked.outcome {
            Ok(hirsel_engine::RunOutcome::Trapped(trap)) => {
                format!("checked/{}", trap.rule)
            }
            _ => "checked/divergence".into(),
        };
        return Some(Classification::Bug { kind, detail });
    }
    if let (Some((native, matrix)), Some(observation)) = (native, &normal.observation) {
        let reference = hirsel_driver::native::Observation::of_reference(observation);
        return native.compare(engine, matrix, &reference, &config.entry, fixture);
    }
    None
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Edit {
    pub kind: &'static str,
    pub start: usize,
    pub end: usize,
    pub replacement: String,
}
impl Edit {
    pub fn apply(&self, source: &str) -> String {
        format!(
            "{}{}{}",
            &source[..self.start],
            self.replacement,
            &source[self.end..]
        )
    }
}
#[derive(Clone)]
struct Binding {
    name: String,
    ty: Option<String>,
    /// The written type without modifiers, brand or relation.
    base: Option<String>,
    mutable: bool,
}
struct Catalogue<'a> {
    /// The program, for the records other items declare.
    file: &'a Parsed,
    program: &'a str,
    item: &'a ItemSyntax,
    /// The item's anchor base in the program; edits are program offsets.
    base: u32,
    /// The program from the item's base, so item-relative spans index it.
    source: &'a str,
    edits: &'a mut dyn FnMut(Edit),
    /// The enclosing function returns unit, so `return ()` is an early exit.
    unit_result: bool,
}
impl Catalogue<'_> {
    /// The fields, with their written types, of the records named `ty`.
    fn record_fields(&self, ty: &str) -> Vec<(String, String)> {
        let mut fields = vec![];
        for (syntax, base) in &self.file.items {
            if syntax.item.name.as_str() != ty.trim() {
                continue;
            }
            let ItemKind::Type {
                body: TypeBody::Record(record),
                ..
            } = &syntax.item.kind
            else {
                continue;
            };
            for field in record {
                let span = syntax.tree.types[field.ty.0 as usize].origin.span;
                let text = &self.program[(base + span.start) as usize..(base + span.end) as usize];
                fields.push((field.name.as_str().to_owned(), text.to_owned()));
            }
        }
        fields
    }
    fn text(&self, span: Span) -> &str {
        &self.source[span.start as usize..span.end as usize]
    }
    fn expr_text(&self, id: ExprId) -> &str {
        self.text(self.item.tree.expressions[id.0 as usize].origin.span)
    }
    fn type_text(&self, id: TypeExprId) -> String {
        self.text(self.item.tree.types[id.0 as usize].origin.span)
            .into()
    }
    fn base_type_text(&self, id: TypeExprId) -> String {
        match self.item.tree.types[id.0 as usize].kind {
            TypeExprKind::Qualified { base, .. } => self.type_text(base),
            _ => self.type_text(id),
        }
    }
    fn add(&mut self, kind: &'static str, span: Span, replacement: String) {
        if self.text(span) != replacement {
            (self.edits)(Edit {
                kind,
                start: (self.base + span.start) as usize,
                end: (self.base + span.end) as usize,
                replacement,
            });
        }
    }
    fn insert(&mut self, kind: &'static str, at: u32, replacement: String) {
        self.add(
            kind,
            Span {
                anchor: self.item.anchor,
                start: at,
                end: at,
            },
            replacement,
        );
    }
    fn annotations(&mut self, ty: TypeExprId, scope: &[Binding]) {
        let node = &self.item.tree.types[ty.0 as usize];
        let span = node.origin.span;
        let original = self.type_text(ty);
        let base = if let TypeExprKind::Qualified { base, .. } = node.kind {
            self.type_text(base)
        } else {
            original.clone()
        };
        if base != original {
            self.add("annotation/remove", span, base.clone());
        }
        for modifier in ["unique", "once"] {
            self.add("annotation/mode", span, format!("{modifier} ({base})"));
        }
        self.add("annotation/mode", span, format!("sendable ({base})"));
        for place in ["env", "heap"] {
            self.add("annotation/mode", span, format!("({base}) in {place}"));
        }
        for b in scope {
            self.add("annotation/brand", span, format!("({base}) of {}", b.name));
            self.add(
                "annotation/relation",
                span,
                format!("({base}) in {}", b.name),
            );
        }
        // A function type nested anywhere in the written type, including in
        // a type argument, promises no authority (CLO-2, spec #92).
        let mut pending = vec![ty];
        while let Some(t) = pending.pop() {
            let node = &self.item.tree.types[t.0 as usize];
            match &node.kind {
                TypeExprKind::Named { arguments, .. } => {
                    pending.extend(arguments.iter().filter_map(|a| match a {
                        TypeArgument::Type(t) => Some(*t),
                        TypeArgument::SizeLiteral(_) => None,
                    }))
                }
                TypeExprKind::Tuple(parts) => pending.extend(parts),
                TypeExprKind::Box(inner) => pending.push(*inner),
                TypeExprKind::Qualified { base, .. } => pending.push(*base),
                TypeExprKind::Function(signature) => {
                    if t != ty && signature.uses.is_none() {
                        let span = node.origin.span;
                        let text = self.type_text(t);
                        self.add("annotation/uses", span, format!("{text} uses {{}}"));
                    }
                }
                TypeExprKind::Error => (),
            }
        }
    }
    fn signature(&mut self, sig: &FunctionSyntax, outer: &[Binding]) -> Vec<Binding> {
        let mut scope = outer.to_vec();
        for p in &sig.parameters {
            if let Some(name) = p.name.map(|n| n.as_str()) {
                scope.push(Binding {
                    name: name.into(),
                    ty: Some(self.type_text(p.ty)),
                    base: Some(self.base_type_text(p.ty)),
                    mutable: p.passing == PassingMode::Mut,
                });
            }
        }
        for p in &sig.parameters {
            self.annotations(p.ty, &scope);
            let Some(name) = p.name else { continue };
            let ty_start = self.item.tree.types[p.ty.0 as usize].origin.span.start;
            let Some(index) = self
                .item
                .tree
                .tokens
                .iter()
                .rposition(|t| t.span.end <= ty_start && t.kind == TokenKind::Ident(name))
            else {
                continue;
            };
            let token = &self.item.tree.tokens[index];
            let previous = self.item.tree.tokens[..index]
                .iter()
                .rfind(|t| !matches!(t.kind, TokenKind::Trivia(_)));
            let start = previous
                .filter(|t| matches!(t.kind, TokenKind::Keyword(Keyword::Take | Keyword::Mut)))
                .map_or(token.span.start, |t| t.span.start);
            let span = Span {
                anchor: self.item.anchor,
                start,
                end: token.span.start,
            };
            for mode in ["", "take ", "mut "] {
                self.add("parameter/passing", span, mode.into());
            }
        }
        if let Some(ty) = sig.result {
            self.annotations(ty, &scope);
        }
        scope
    }
    fn pattern(
        &self,
        id: PatternId,
        ty: Option<TypeExprId>,
        mutable: bool,
        scope: &mut Vec<Binding>,
    ) {
        match &self.item.tree.patterns[id.0 as usize].kind {
            PatternKind::Binding(n) => scope.push(Binding {
                name: n.as_str().into(),
                ty: ty.map(|t| self.type_text(t)),
                base: ty.map(|t| self.base_type_text(t)),
                mutable,
            }),
            PatternKind::Tuple(children) => {
                for child in children {
                    self.pattern(*child, None, mutable, scope);
                }
            }
            PatternKind::Constructor { fields, .. } => {
                for field in fields {
                    self.pattern(field.pattern, None, mutable, scope);
                }
            }
            // Every alternative binds the same names.
            PatternKind::Or(alternatives) => self.pattern(alternatives[0], None, mutable, scope),
            _ => {}
        }
    }
    fn unit(&self, signature: &FunctionSyntax) -> bool {
        signature
            .result
            .is_none_or(|t| self.type_text(t).trim() == "()")
    }
    /// Last-use loan shapes (LANG-3): a view of a mutable binding that is dead
    /// before the rest of the block or used after it, which for a binding that
    /// holds a view is a closure capturing that view; and an early return that
    /// drops every live holder on that path.
    fn last_use(&mut self, at: u32, rest_end: u32, scope: &[Binding]) {
        if self.unit_result {
            self.insert(
                "last-use/early-return",
                at,
                "\nif false { return () }\n".into(),
            );
        }
        if rest_end <= at {
            return;
        }
        let rest = self.source[at as usize..rest_end as usize].to_owned();
        for binding in scope.iter().filter(|b| b.mutable) {
            let view = format!("\nlet last_use_view = fn() {{ copy {} }};\n", binding.name);
            self.insert(
                "last-use/view-dead",
                at,
                format!("{view}let _ = last_use_view();\n"),
            );
            self.add(
                "last-use/view-live",
                Span {
                    anchor: self.item.anchor,
                    start: at,
                    end: rest_end,
                },
                format!("{view}{rest}\nlet _ = last_use_view();\n"),
            );
        }
    }
    fn store(&mut self, at: u32, scope: &[Binding]) {
        for target in scope {
            if target.mutable {
                for value in scope.iter().filter(|v| v.ty.is_some() && v.ty == target.ty) {
                    self.insert(
                        "store/mut",
                        at,
                        format!("\n{} = copy {};\n", target.name, value.name),
                    );
                }
            }
            let Some(ty) = &target.ty else { continue };
            // Written nominal records and sequence elements are the only shapes
            // this finite syntactic catalogue knows. Checking decides legality.
            let fields = self.record_fields(ty);
            for (field, field_ty) in fields {
                for value in scope.iter().filter(|v| v.ty.as_ref() == Some(&field_ty)) {
                    self.insert(
                        "store/field",
                        at,
                        format!("\n{}.{} = copy {};\n", target.name, field, value.name),
                    );
                }
            }
            if let Some(element) = ["Array[", "List[", "Inline["]
                .iter()
                .find_map(|p| ty.strip_prefix(p))
            {
                let element = element
                    .trim_end_matches(']')
                    .split(',')
                    .next()
                    .unwrap()
                    .trim();
                for value in scope.iter().filter(|v| v.ty.as_deref() == Some(element)) {
                    self.insert(
                        "store/element",
                        at,
                        format!("\n{}[0u64] = copy {};\n", target.name, value.name),
                    );
                }
            }
        }
    }
    /// A binding's value held as part of a sequence element (LANG-3): `let p: T =
    /// e` becomes `let agree_held_N: List[(T, u64)] = [(e, 0u64)]` and then
    /// `let (p, _) = agree_held_N[0]`, so `p` names a tuple part of a list
    /// element. What `e` holds (a view) is then held through the element
    /// while the list is live. Written modifiers and relations stay on the
    /// binding they constrain: `let p: unique T in s = e` gives
    /// `let (p, _): unique (T, u64) in s = ..`. Literal and closure
    /// initializers are held too: an element's placement, a block-local
    /// element and a closure's construction destination depend on the holder
    /// as much as a loan does. Checking decides legality.
    fn sequence_holder(
        &mut self,
        statement: Span,
        mutable: bool,
        pattern: PatternId,
        ty: Option<TypeExprId>,
        value: ExprId,
    ) {
        let held = format!("agree_held_{}", statement.start);
        // (list type, binding type, the list type also carrying the binding's
        // written relation, when it has one).
        let (list, binding, related) = ty.map_or(Default::default(), |ty| {
            let span = self.item.tree.types[ty.0 as usize].origin.span;
            let TypeExprKind::Qualified { base, relation, .. } =
                &self.item.tree.types[ty.0 as usize].kind
            else {
                return (
                    format!(": List[({}, u64)]", self.text(span)),
                    String::new(),
                    None,
                );
            };
            let base = self.item.tree.types[base.0 as usize].origin.span;
            let suffix = &self.source[base.end as usize..span.end as usize];
            (
                format!(": List[({}, u64)]", self.text(base)),
                format!(
                    ": {}({}, u64){suffix}",
                    &self.source[span.start as usize..base.start as usize],
                    self.text(base),
                ),
                relation
                    .is_some()
                    .then(|| format!(": List[({}, u64)]{suffix}", self.text(base))),
            )
        });
        let pattern = self.item.tree.patterns[pattern.0 as usize].origin.span;
        // The statement's own terminator, if any, follows the initializer.
        let end = self.item.tree.expressions[value.0 as usize].origin.span.end;
        let replacement = |list: &str| {
            format!(
                "let {held}{list} = [({}, 0u64)]\nlet {}({}, _){binding} = {held}[0]",
                self.expr_text(value),
                if mutable { "mut " } else { "" },
                self.text(pattern),
            )
        };
        let (plain, related) = (replacement(&list), related.map(|list| replacement(&list)));
        // The element is the tuple a called closure returns, so what the
        // initializer builds inside that closure (a closure literal's row is
        // local to it) reaches the list's Global element at depth.
        // A written `@noalloc` part then receives whatever evidence the
        // initializer's row carries, and an `Iter` the captures and cleanup
        // its construction fixed.
        if ty.is_some() {
            let made = plain.replacen(
                &format!("[({}, 0u64)]", self.expr_text(value)),
                &format!("[(fn() {{ ({}, 0u64) }})()]", self.expr_text(value)),
                1,
            );
            self.add("holder/closure-result", Span { end, ..statement }, made);
            // The same initializer as the whole result of a called closure:
            // the binding's written type, at every depth, receives a row
            // that no expected type shaped.
            let value = self.item.tree.expressions[value.0 as usize].origin.span;
            self.add(
                "holder/closure-value",
                value,
                format!("(fn() {{ {} }})()", self.text(value)),
            );
        }
        self.add("holder/sequence-element", Span { end, ..statement }, plain);
        // The holder itself lives where the binding's relation says: an
        // arena-placed list holds the element (PROV-4).
        if let Some(related) = related {
            self.add(
                "holder/related-sequence",
                Span { end, ..statement },
                related,
            );
        }
    }
    /// A block's value held as an element of a list the block declares:
    /// a tail `v` naming a binding of written type `T` becomes
    /// `{ let agree_tail: List[T] = [v]; agree_tail[0] }`, so the block
    /// yields a part of storage it ends (LANG-11). Checking decides legality.
    fn tail_holder(&mut self, tail: ExprId, scope: &[Binding]) {
        let ExprKind::Path(path) = &self.item.tree.expressions[tail.0 as usize].kind else {
            return;
        };
        let ([segment], None) = (&path.segments[..], &path.package) else {
            return;
        };
        let name = segment.as_str();
        // A sequence literal takes its type from the holder's annotation.
        let Some(base) = scope
            .iter()
            .rev()
            .find(|b| b.name == name)
            .and_then(|b| b.base.clone())
        else {
            return;
        };
        let span = self.item.tree.expressions[tail.0 as usize].origin.span;
        self.add(
            "holder/tail-element",
            span,
            format!("{{\nlet agree_tail: List[{base}] = [{name}]\nagree_tail[0]\n}}"),
        );
        // The tail as the result of a closure call local to that binding
        // (CLO-1: local to a borrowed parameter), and of a closure that
        // captures it and returns a counted share (MODE-1): a share of Global
        // content outlives the block, a value that borrows the block's
        // storage must not leave it.
        self.add(
            "tail/local-result",
            span,
            format!("(fn(agree_x: {base}) {{ agree_x }})({name})"),
        );
        self.add(
            "tail/closure-capture",
            span,
            format!("{{\nlet agree_f = fn() {{ {name} }}\nagree_f()\n}}"),
        );
    }
    /// Comparisons through relations: each binding a statement introduces is
    /// compared with every earlier one whose written type differs or is
    /// unwritten, so values of one base type with different relations (an
    /// inferred call result, an arena-placed binding) meet. Checking decides
    /// legality.
    fn compare(&mut self, at: u32, scope: &[Binding], introduced: usize) {
        for new in &scope[introduced.min(scope.len())..] {
            for old in &scope[..introduced.min(scope.len())] {
                if old.name != new.name
                    && (new.ty.is_none() || old.ty.is_none() || old.ty != new.ty)
                {
                    self.insert(
                        "comparison/relation",
                        at,
                        format!("\nlet _ = {} == {};\n", new.name, old.name),
                    );
                }
            }
        }
    }
    /// A relation on an intermediate binding passed to a call (#88): an
    /// owned argument naming a binding of written type is received first by
    /// a let whose relation names the new binding itself (its frame bound)
    /// or the innermost binding in scope. Checking decides legality.
    fn related_argument(&mut self, argument: ExprId, scope: &[Binding]) {
        let node = &self.item.tree.expressions[argument.0 as usize];
        let ExprKind::Unary {
            op: op @ (UnaryOp::Take | UnaryOp::Copy),
            value,
        } = node.kind
        else {
            return;
        };
        let ExprKind::Path(path) = &self.item.tree.expressions[value.0 as usize].kind else {
            return;
        };
        let [segment] = path.segments[..] else {
            return;
        };
        if path.package.is_some() {
            return;
        }
        let name = segment.as_str();
        // A written relation already covers this binding (annotation/relation).
        let Some(ty) = scope
            .iter()
            .rev()
            .find(|b| b.name == name)
            .and_then(|b| b.ty.clone())
            .filter(|ty| !ty.contains(" in "))
        else {
            return;
        };
        let span = node.origin.span;
        let mode = if op == UnaryOp::Take { "take" } else { "copy" };
        let related = format!("agree_related_{}", span.start);
        let places = std::iter::once(related.as_str()).chain(
            scope
                .iter()
                .rev()
                .find(|b| b.name != name)
                .map(|b| b.name.as_str()),
        );
        let replacements: Vec<_> = places
            .map(|place| {
                format!("{{ let {related}: ({ty}) in {place} = {mode} {name}; take {related} }}")
            })
            .collect();
        for replacement in replacements {
            self.add("argument/related-binding", span, replacement);
        }
    }
    /// The holes of the f-string `id` desugars, whose other nodes have no
    /// source text of their own to edit (SEM-3).
    fn format_holes(&self, id: ExprId) -> Option<&[ExprId]> {
        let format = self.item.tree.formats.iter().find(|f| f.expression == id)?;
        Some(&format.holes)
    }
    fn expr(&mut self, id: ExprId, scope: &[Binding]) {
        if let Some(holes) = self.format_holes(id) {
            for hole in holes.to_vec() {
                self.expr(hole, scope);
            }
            return;
        }
        let node = self.item.tree.expressions[id.0 as usize].clone();
        let span = node.origin.span;
        let text = self.expr_text(id).to_owned();
        match node.kind {
            ExprKind::Path(ref p) => {
                if p.package.is_none() && p.segments.len() == 1 {
                    let name = p.segments[0].as_str();
                    if let Some(b) = scope.iter().rev().find(|b| b.name == name) {
                        for other in scope
                            .iter()
                            .filter(|o| o.name != name && b.ty.is_some() && b.ty == o.ty)
                        {
                            self.add("operand/same-type", span, other.name.clone());
                        }
                        self.add("binding/twice", span, format!("{{ {name}; {name} }}"));
                    }
                }
            }
            ExprKind::Call { callee, arguments } => {
                self.expr(callee, scope);
                for arg in arguments {
                    let arg_node = &self.item.tree.expressions[arg.value.0 as usize];
                    let base = match arg_node.kind {
                        ExprKind::Unary {
                            op: UnaryOp::Take | UnaryOp::Mut | UnaryOp::Copy,
                            value,
                        } => self.expr_text(value),
                        _ => self.expr_text(arg.value),
                    }
                    .to_owned();
                    for mode in ["", "take ", "mut ", "copy "] {
                        self.add(
                            "argument/passing",
                            arg_node.origin.span,
                            format!("{mode}{base}"),
                        );
                    }
                    self.related_argument(arg.value, scope);
                    self.expr(arg.value, scope);
                }
            }
            ExprKind::Unary { op, value } => {
                if matches!(op, UnaryOp::Take)
                    && matches!(
                        self.item.tree.expressions[value.0 as usize].kind,
                        ExprKind::Path(_)
                    )
                {
                    let name = self.expr_text(value);
                    let temp = format!("agree_taken_{}", span.start);
                    self.add(
                        "binding/after-take",
                        span,
                        format!("{{ let {temp} = take {name}; {name}; take {temp} }}"),
                    );
                }
                self.expr(value, scope);
            }
            ExprKind::Field { base, .. } => self.expr(base, scope),
            ExprKind::Index { base, index } => {
                self.expr(base, scope);
                self.expr(index, scope);
            }
            ExprKind::Binary { left, right, .. } => {
                self.expr(left, scope);
                self.expr(right, scope);
            }
            ExprKind::Tuple(values) | ExprKind::Sequence(values) => {
                for v in values {
                    self.expr(v, scope);
                }
            }
            ExprKind::Try { value, .. } | ExprKind::Before(value) | ExprKind::Comptime(value) => {
                self.expr(value, scope)
            }
            ExprKind::Return(value) | ExprKind::Break { value, .. } => {
                if let Some(v) = value {
                    self.expr(v, scope);
                }
            }
            ExprKind::Closure { signature, body } => {
                let nested = self.signature(&signature, scope);
                let unit = self.unit(&signature);
                let outer = std::mem::replace(&mut self.unit_result, unit);
                self.expr(body, &nested);
                self.unit_result = outer;
            }
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.expr(condition, scope);
                self.expr(then_branch, scope);
                if let Some(e) = else_branch {
                    self.expr(e, scope);
                }
            }
            ExprKind::Match { value, arms } => {
                self.expr(value, scope);
                for arm in arms {
                    let mut nested = scope.to_vec();
                    self.pattern(arm.pattern, None, false, &mut nested);
                    if let Some(g) = arm.guard {
                        self.expr(g, &nested);
                    }
                    self.expr(arm.body, &nested);
                }
            }
            ExprKind::Block {
                statements, tail, ..
            } => {
                let mut nested = scope.to_vec();
                // Later statements end here: before the tail, else after the last one.
                let rest_end = match (tail, statements.last()) {
                    (Some(tail), _) => {
                        self.item.tree.expressions[tail.0 as usize]
                            .origin
                            .span
                            .start
                    }
                    (None, Some(last)) => {
                        self.item.tree.statements[last.0 as usize].origin.span.end
                    }
                    (None, None) => 0,
                };
                for pair in statements.windows(2) {
                    let a = self.item.tree.statements[pair[0].0 as usize].origin.span;
                    let b = self.item.tree.statements[pair[1].0 as usize].origin.span;
                    self.add(
                        "statement/swap",
                        Span { end: b.end, ..a },
                        format!("{}\n{}\n", self.text(b), self.text(a)),
                    );
                    let (inner, label) = match self.item.tree.statements[pair[1].0 as usize].kind {
                        StmtKind::For { body, .. } | StmtKind::While { body, .. } => {
                            (Some(body), "statement/into-loop")
                        }
                        StmtKind::Expr(e) => match self.item.tree.expressions[e.0 as usize].kind {
                            ExprKind::If { then_branch, .. } => {
                                (Some(then_branch), "statement/into-branch")
                            }
                            _ => (None, ""),
                        },
                        _ => (None, ""),
                    };
                    if let Some(inner) = inner {
                        let block = self.item.tree.expressions[inner.0 as usize].origin.span;
                        if matches!(
                            self.item.tree.expressions[inner.0 as usize].kind,
                            ExprKind::Block { .. }
                        ) {
                            // One semantic relocation, represented as one enclosing-range edit.
                            let mut replacement = self.text(b).to_owned();
                            replacement.insert_str(
                                (block.start + 1 - b.start) as usize,
                                &format!("\n{}\n", self.text(a)),
                            );
                            self.add(label, Span { end: b.end, ..a }, replacement);
                        }
                    }
                }
                for pair in statements.windows(2) {
                    let (a_roots, a_moves) = self.statement_parts(pair[0]);
                    let (b_roots, b_moves) = self.statement_parts(pair[1]);
                    let a = self.item.tree.statements[pair[0].0 as usize].origin.span;
                    let b = self.item.tree.statements[pair[1].0 as usize].origin.span;
                    if a_moves {
                        self.into_argument(pair[0], b, &b_roots, true);
                    }
                    if b_moves {
                        self.into_argument(pair[1], a, &a_roots, false);
                    }
                }
                // The tail call once more as a statement, whose result the
                // frame destroys (MEM-5).
                if let Some(tail) = tail {
                    if matches!(
                        self.item.tree.expressions[tail.0 as usize].kind,
                        ExprKind::Call { .. }
                    ) {
                        let target = self.item.tree.expressions[tail.0 as usize].origin.span;
                        self.insert(
                            "statement/discard",
                            target.start,
                            format!("{}\n", self.text(target)),
                        );
                    }
                }
                if let (Some(tail), Some(last)) = (tail, statements.last()) {
                    if self.statement_parts(*last).1 {
                        let target = self.item.tree.expressions[tail.0 as usize].origin.span;
                        self.into_argument(*last, target, &[tail], true);
                    }
                }
                for stmt_id in statements {
                    let stmt = self.item.tree.statements[stmt_id.0 as usize].clone();
                    let at = stmt.origin.span.end;
                    let introduced = nested.len();
                    match stmt.kind {
                        StmtKind::Let {
                            mutable,
                            pattern,
                            ty,
                            value,
                            otherwise,
                        } => {
                            self.expr(value, &nested);
                            if let Some(otherwise) = otherwise {
                                self.expr(otherwise, &nested);
                            }
                            self.sequence_holder(stmt.origin.span, mutable, pattern, ty, value);
                            let pattern_span =
                                self.item.tree.patterns[pattern.0 as usize].origin.span;
                            let prefix = Span {
                                end: pattern_span.start,
                                ..stmt.origin.span
                            };
                            self.add(
                                "binding/mut",
                                prefix,
                                if mutable {
                                    "let ".into()
                                } else {
                                    "let mut ".into()
                                },
                            );
                            if let Some(t) = ty {
                                self.annotations(t, &nested);
                            }
                            self.pattern(pattern, ty, mutable, &mut nested);
                        }
                        StmtKind::Assign { place, value, .. } => {
                            self.expr(place, &nested);
                            self.expr(value, &nested);
                        }
                        StmtKind::Expr(e) => {
                            self.expr(e, &nested);
                            self.insert(
                                "statement/twice",
                                at,
                                format!("\n{}\n", self.text(stmt.origin.span)),
                            );
                        }
                        StmtKind::While {
                            condition, body, ..
                        }
                        | StmtKind::For {
                            source: condition,
                            body,
                            ..
                        } => {
                            self.expr(condition, &nested);
                            let mut body_scope = nested.clone();
                            if let StmtKind::For { pattern, .. } = stmt.kind {
                                self.pattern(pattern, None, false, &mut body_scope);
                            }
                            self.expr(body, &body_scope);
                            self.move_out(stmt.origin.span, body, "statement/out-of-loop");
                        }
                        StmtKind::Function {
                            signature, body, ..
                        } => {
                            let body_scope = self.signature(&signature, &nested);
                            let unit = self.unit(&signature);
                            let outer = std::mem::replace(&mut self.unit_result, unit);
                            self.expr(body, &body_scope);
                            self.unit_result = outer;
                        }
                    }
                    self.store(at, &nested);
                    self.compare(at, &nested, introduced);
                    self.last_use(at, rest_end, &nested);
                }
                if let Some(tail) = tail {
                    self.expr(tail, &nested);
                    self.tail_holder(tail, &nested);
                    let tail_span = self.item.tree.expressions[tail.0 as usize].origin.span;
                    for binding in &nested {
                        self.add("return/local", tail_span, binding.name.clone());
                        if let Some(ty) = &binding.ty {
                            let fields = self.record_fields(ty);
                            for (field, _) in fields {
                                self.add(
                                    "return/field",
                                    tail_span,
                                    format!("{}.{}", binding.name, field),
                                );
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        if matches!(
            self.item.tree.expressions[id.0 as usize].kind,
            ExprKind::Path(_)
                | ExprKind::Literal(_)
                | ExprKind::Call { .. }
                | ExprKind::Field { .. }
                | ExprKind::Index { .. }
                | ExprKind::Unary { .. }
                | ExprKind::Binary { .. }
                | ExprKind::Tuple(_)
                | ExprKind::Sequence(_)
        ) {
            self.add(
                "expression/closure-call",
                span,
                format!("(fn() {{ {text} }})()"),
            );
        }
        if let ExprKind::If { then_branch, .. } = self.item.tree.expressions[id.0 as usize].kind {
            self.move_out(span, then_branch, "statement/out-of-branch");
        }
    }
    /// The calls an expression evaluates in its own scope: not those inside a
    /// nested block, branch, match or closure.
    fn calls(&self, id: ExprId, out: &mut Vec<ExprId>) {
        if let Some(holes) = self.format_holes(id) {
            for hole in holes {
                self.calls(*hole, out);
            }
            return;
        }
        match &self.item.tree.expressions[id.0 as usize].kind {
            ExprKind::Call { callee, arguments } => {
                out.push(id);
                self.calls(*callee, out);
                for argument in arguments {
                    self.calls(argument.value, out);
                }
            }
            ExprKind::Unary { value, .. }
            | ExprKind::Field { base: value, .. }
            | ExprKind::Try { value, .. } => self.calls(*value, out),
            ExprKind::Index { base, index } => {
                self.calls(*base, out);
                self.calls(*index, out);
            }
            ExprKind::Binary { left, right, .. } => {
                self.calls(*left, out);
                self.calls(*right, out);
            }
            ExprKind::Tuple(values) | ExprKind::Sequence(values) => {
                for value in values {
                    self.calls(*value, out);
                }
            }
            _ => {}
        }
    }
    /// The expressions a statement evaluates directly, and whether the
    /// statement is a write (an assignment or a call statement) that may move
    /// into a call argument.
    fn statement_parts(&self, id: StmtId) -> (Vec<ExprId>, bool) {
        match self.item.tree.statements[id.0 as usize].kind {
            StmtKind::Let { value, .. } => (vec![value], false),
            StmtKind::Assign { place, value, .. } => (vec![place, value], true),
            StmtKind::Expr(e) => (
                vec![e],
                matches!(
                    self.item.tree.expressions[e.0 as usize].kind,
                    ExprKind::Call { .. }
                ),
            ),
            _ => (vec![], false),
        }
    }
    /// The binding an assignment writes through an index, if any: its list
    /// may be shared, so the write detaches it (OWN-4).
    fn indexed_root(&self, id: StmtId) -> Option<String> {
        let StmtKind::Assign { place, .. } = self.item.tree.statements[id.0 as usize].kind else {
            return None;
        };
        let mut place = place;
        let mut indexed = false;
        loop {
            match &self.item.tree.expressions[place.0 as usize].kind {
                ExprKind::Field { base, .. } => place = *base,
                ExprKind::Index { base, .. } => {
                    indexed = true;
                    place = *base;
                }
                ExprKind::Path(p) if indexed && p.package.is_none() && p.segments.len() == 1 => {
                    return Some(p.segments[0].as_str().into());
                }
                _ => return None,
            }
        }
    }
    /// A write moved into a later argument of a call in the neighbouring
    /// statement or tail (`f(a, { w; b })`), so it runs while the loans of
    /// the earlier arguments are live (LANG-3). A write through an index
    /// is also moved after a copy of the indexed list, so it detaches a shared
    /// list (OWN-4). `before` says the write precedes `target` in the source.
    fn into_argument(&mut self, moved: StmtId, target: Span, roots: &[ExprId], before: bool) {
        let moved_span = self.item.tree.statements[moved.0 as usize].origin.span;
        let write = self.text(moved_span).to_owned();
        let shared = self
            .indexed_root(moved)
            .map(|root| format!("let agree_shared_{} = copy {root}\n", moved_span.start));
        let span = if before {
            Span {
                end: target.end,
                ..moved_span
            }
        } else {
            Span {
                end: moved_span.end,
                ..target
            }
        };
        let mut calls = vec![];
        for root in roots {
            self.calls(*root, &mut calls);
        }
        for call in calls {
            let ExprKind::Call { arguments, .. } =
                &self.item.tree.expressions[call.0 as usize].kind
            else {
                continue;
            };
            for argument in arguments.iter().skip(1) {
                let at = self.item.tree.expressions[argument.value.0 as usize]
                    .origin
                    .span;
                let value = self.text(at).to_owned();
                for (kind, prefix) in [
                    ("statement/into-argument", Some(String::new())),
                    ("statement/into-argument-shared", shared.clone()),
                ] {
                    let Some(prefix) = prefix else { continue };
                    let mut replacement = self.text(target).to_owned();
                    replacement.replace_range(
                        (at.start - target.start) as usize..(at.end - target.start) as usize,
                        &format!("{{\n{prefix}{write}\n{value}\n}}"),
                    );
                    self.add(kind, span, replacement);
                }
            }
        }
    }
    fn move_out(&mut self, outer: Span, body: ExprId, kind: &'static str) {
        if let ExprKind::Block { statements, .. } =
            &self.item.tree.expressions[body.0 as usize].kind
        {
            for stmt in statements {
                let span = self.item.tree.statements[stmt.0 as usize].origin.span;
                let mut replacement = self.text(outer).to_owned();
                let moved = self.text(span).to_owned();
                replacement.replace_range(
                    (span.start - outer.start) as usize..(span.end - outer.start) as usize,
                    "",
                );
                self.add(kind, outer, format!("{{ {moved}\n{replacement}\n }}"));
            }
        }
    }
}
/// Edits address AST node spans, preserving all bytes outside the edited syntax.
/// No formatter, phase internals, type inference or random token corruption.
pub fn catalogue(file: &Parsed, source: &str) -> Vec<Edit> {
    let mut edits = BTreeSet::new();
    enumerate(file, source, &mut |edit| {
        edits.insert(edit);
    });
    edits.into_iter().collect()
}
fn enumerate(file: &Parsed, source: &str, edits: &mut dyn FnMut(Edit)) {
    for (syntax, base) in &file.items {
        let mut c = Catalogue {
            file,
            program: source,
            item: syntax,
            base: *base,
            source: &source[*base as usize..],
            edits: &mut *edits,
            unit_result: false,
        };
        match &syntax.item.kind {
            ItemKind::Function {
                signature, body, ..
            } => {
                let scope = c.signature(signature, &[]);
                c.unit_result = c.unit(signature);
                if let Some(body) = body {
                    c.expr(*body, &scope);
                }
            }
            ItemKind::Const { value, .. } => {
                c.unit_result = false;
                c.expr(*value, &[])
            }
            ItemKind::Type {
                drop: Some(drop), ..
            } => {
                c.unit_result = true;
                c.expr(drop.body, &[])
            }
            _ => {}
        }
    }
}
/// A parsed program: each item with its anchor's base offset.
pub struct Parsed {
    items: Vec<(std::sync::Arc<ItemSyntax>, u32)>,
}
fn parse(source: &str) -> Result<Parsed, String> {
    let mut engine = engine();
    let file = engine
        .sources
        .add("agree.hir", source.into())
        .ok_or("source too large")?;
    let parsed = engine
        .parse_file(file, &hirsel_engine::default_context())
        .map_err(|e| format!("{e:?}"))?;
    if parsed.value.has_syntax_errors {
        return Err("syntax errors".into());
    }
    let base = |syntax: &ItemSyntax| engine.sources.anchor(syntax.anchor).map_or(0, |a| a.1);
    Ok(Parsed {
        items: parsed
            .value
            .items
            .iter()
            .map(|syntax| (syntax.clone(), base(syntax)))
            .collect(),
    })
}
/// Count syntax mutations without checking, interpreting or compiling them.
pub fn catalogue_count(source: &str) -> Result<usize, String> {
    Ok(catalogue_counts(source)?.values().sum())
}
/// Inventory edit families without classifying or executing mutations.
pub fn catalogue_counts(source: &str) -> Result<BTreeMap<&'static str, usize>, String> {
    let mut counts = BTreeMap::new();
    let mut seen = BTreeSet::new();
    enumerate(&parse(source)?, source, &mut |edit| {
        if seen.insert(edit_identity(&edit)) {
            *counts.entry(edit.kind).or_default() += 1;
        }
    });
    Ok(counts)
}
/// Greedy syntax reduction reaches a deletion fixed point for definitions and
/// statements. Preserve the same first bug kind, not just any failing outcome.
pub fn shrink(source: &str, execution: Execution, kind: &str) -> String {
    shrink_native(source, execution, kind, None)
}

/// `shrink` for a discrepancy found under `native`'s configurations.
pub fn shrink_native(
    source: &str,
    execution: Execution,
    kind: &str,
    native: Option<(&Native, &[hirsel_driver::native::Configuration])>,
) -> String {
    shrink_package_native(source, execution, kind, native, None, 0)
}
fn shrink_package_native(
    source: &str,
    execution: Execution,
    kind: &str,
    native: Option<(&Native, &[hirsel_driver::native::Configuration])>,
    path: Option<&Path>,
    hash_seed: u64,
) -> String {
    let classify = |source: &str, execution: Execution| {
        classify_package_native(source, execution, native, path, hash_seed)
    };
    let mut current = source.to_owned();
    loop {
        let Ok(file) = parse(&current) else { break };
        let mut ranges: Vec<_> = file
            .items
            .iter()
            .flat_map(|(syntax, base)| {
                let base = *base;
                let at = move |s: Span| Span {
                    start: base + s.start,
                    end: base + s.end,
                    ..s
                };
                std::iter::once(at(syntax.item.origin.span)).chain(
                    syntax
                        .tree
                        .statements
                        .iter()
                        .map(move |s| at(s.origin.span)),
                )
            })
            .collect();
        ranges.sort_by_key(|s| (std::cmp::Reverse(s.end - s.start), s.start));
        let mut changed = false;
        for span in ranges {
            let mut candidate = current.clone();
            candidate.replace_range(span.start as usize..span.end as usize, "");
            if classify(&candidate, execution).key() == Some(kind) {
                current = candidate;
                changed = true;
                break;
            }
        }
        if !changed {
            break;
        }
    }
    let cleaned = current
        .lines()
        .filter(|line| !line.trim_start().starts_with("//") && !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if classify(&cleaned, execution).key() == Some(kind) {
        cleaned
    } else {
        current
    }
}

#[derive(Default, Debug)]
pub struct Report {
    pub hash_seed: u64,
    /// Corpus programs not expected to be rejected.
    pub programs: usize,
    /// Catalogue size over all of them.
    pub generated: usize,
    /// Catalogue size after the per-program budget, before limit and sharding.
    pub eligible: usize,
    pub variants_per_program: Option<usize>,
    /// Sampled variants whose original program is not accepted today.
    pub skipped: usize,
    pub checked: usize,
    pub rejected: usize,
    pub valid: usize,
    pub unavailable: usize,
    pub runtime_limits: usize,
    /// Valid variants executed both normally and under checked execution.
    pub executed: usize,
    /// `executed` by the expectation of the program each variant edits.
    pub executions: BTreeMap<&'static str, usize>,
    pub clusters: BTreeMap<(String, &'static str), Cluster>,
    pub edits: BTreeMap<&'static str, usize>,
}
#[derive(Debug)]
pub struct Cluster {
    pub count: usize,
    pub path: String,
    pub source: String,
    pub detail: String,
    execution: Execution,
    file: Option<std::path::PathBuf>,
}
impl Report {
    /// Reduce every cluster's first variant to a minimal representative.
    pub fn shrink(&mut self) {
        self.shrink_native(None);
    }
    pub fn shrink_native(&mut self, native: Option<&Native>) {
        for ((kind, _), cluster) in &mut self.clusters {
            let matrix = native.map(|n| n.for_program(&cluster.path));
            cluster.source = shrink_package_native(
                &cluster.source,
                cluster.execution,
                kind,
                native.zip(matrix.as_deref()),
                cluster.file.as_deref(),
                self.hash_seed,
            );
        }
    }
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({ "hash_seed": self.hash_seed, "programs": self.programs, "generated": self.generated,
            "eligible": self.eligible, "variants_per_program": self.variants_per_program,
            "skipped": self.skipped, "checked": self.checked, "rejected": self.rejected,
            "valid": self.valid, "unavailable": self.unavailable,
            "runtime_limits": self.runtime_limits, "executed": self.executed,
            "executions": self.executions,
            "edits": self.edits,
            "clusters": self.clusters.iter().map(|((kind, edit), c)| serde_json::json!({
                "kind": kind, "edit": edit, "count": c.count, "path": c.path,
                "source": c.source, "detail": c.detail
            })).collect::<Vec<_>>() })
    }
}
/// SplitMix64 finaliser: explicit, wrapping, independent of host hash seeds.
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e3779b97f4a7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
    z ^ (z >> 31)
}
/// A variant's rank under a seed depends only on its own program and edit, so
/// a sample stays put when unrelated corpus files are added.
fn rank(seed: u64, path: &str, edit: &Edit) -> u64 {
    let start = edit.start.to_le_bytes();
    let end = edit.end.to_le_bytes();
    let parts: [&[u8]; 5] = [
        path.as_bytes(),
        edit.kind.as_bytes(),
        &start,
        &end,
        edit.replacement.as_bytes(),
    ];
    parts.iter().fold(mix(seed), |hash, part| {
        part.iter()
            .fold(mix(hash ^ part.len() as u64), |h, b| mix(h ^ u64::from(*b)))
    })
}
/// Independent engines on `HIRSEL_JOBS` workers; results keep the input order.
fn parallel<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let workers = hirsel_guard::jobs();
    std::thread::scope(|scope| {
        let handles: Vec<_> = items
            .chunks(items.len().div_ceil(workers).max(1))
            .map(|chunk| {
                let f = &f;
                std::thread::Builder::new()
                    .stack_size(64 << 20)
                    .spawn_scoped(scope, move || chunk.iter().map(f).collect::<Vec<_>>())
                    .unwrap_or_else(|error| hirsel_guard::spawn_failed(64 << 20, error))
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("classification worker"))
            .collect()
    })
}
/// A classification running at least this long is named on stderr while it
/// runs, and again each time its running time doubles, so a variant that never
/// terminates names itself long before its shard's timeout.
const SLOW: Duration = Duration::from_secs(30);
/// How many of the slowest classifications a stage lists when it ends.
const SLOWEST: usize = 5;
/// `parallel`, timing every item: names slow items while they run and prints
/// the stage's wall time, slowest items and the programs (a label's first
/// word) with the most time in all to stderr when it ends.
fn watched<T: Sync, R: Send>(
    stage: &str,
    items: &[T],
    label: impl Fn(&T) -> String + Sync,
    f: impl Fn(&T) -> R + Sync,
) -> Vec<R> {
    let start = Instant::now();
    let next = AtomicUsize::new(0);
    // In-flight items: start, label and the running time of the next report.
    let running = Mutex::new(BTreeMap::<usize, (Instant, String, Duration)>::new());
    let slowest = Mutex::new(Vec::<(Duration, String)>::new());
    let programs = Mutex::new(BTreeMap::<String, (Duration, usize)>::new());
    let done = (Mutex::new(false), Condvar::new());
    let results = std::thread::scope(|scope| {
        scope.spawn(|| {
            let mut stop = done.0.lock().unwrap();
            while !*stop {
                stop = done.1.wait_timeout(stop, Duration::from_secs(5)).unwrap().0;
                for (began, label, report) in running.lock().unwrap().values_mut() {
                    let elapsed = began.elapsed();
                    if elapsed >= *report {
                        eprintln!(
                            "agree: {stage}: {label} running for {} s",
                            elapsed.as_secs()
                        );
                        *report *= 2;
                    }
                }
            }
        });
        let results = parallel(items, |item| {
            let id = next.fetch_add(1, Ordering::Relaxed);
            let began = Instant::now();
            running
                .lock()
                .unwrap()
                .insert(id, (began, label(item), SLOW));
            let result = f(item);
            let (_, label, _) = running.lock().unwrap().remove(&id).expect("registered");
            let elapsed = began.elapsed();
            let program = label.split(' ').next().unwrap_or_default().to_owned();
            let mut programs = programs.lock().unwrap();
            let total = programs.entry(program).or_default();
            total.0 += elapsed;
            total.1 += 1;
            drop(programs);
            let mut slowest = slowest.lock().unwrap();
            slowest.push((elapsed, label));
            slowest.sort_by(|a, b| b.0.cmp(&a.0));
            slowest.truncate(SLOWEST);
            result
        });
        *done.0.lock().unwrap() = true;
        done.1.notify_all();
        results
    });
    eprintln!(
        "agree: {stage}: {} in {:.1} s",
        items.len(),
        start.elapsed().as_secs_f64()
    );
    for (elapsed, label) in slowest.into_inner().unwrap() {
        eprintln!("agree: {stage}: {:.2} s {label}", elapsed.as_secs_f64());
    }
    let mut programs: Vec<_> = programs.into_inner().unwrap().into_iter().collect();
    programs.sort_by(|a, b| b.1.0.cmp(&a.1.0));
    for (program, (total, count)) in programs.into_iter().take(SLOWEST) {
        eprintln!(
            "agree: {stage}: {:.2} s in all for {count} of {program}",
            total.as_secs_f64()
        );
    }
    results
}
/// The stderr line that streams a cluster's first variant: `agree: cluster `
/// and one line of JSON with the report's cluster fields, the source unreduced.
pub fn streamed_cluster(kind: &str, edit: &str, path: &str, source: &str, detail: &str) -> String {
    let cluster = serde_json::json!({
        "kind": kind, "edit": edit, "path": path, "source": source, "detail": detail
    });
    format!("agree: cluster {cluster}")
}
/// Partition by the existing stable variant rank, after global sampling.
fn in_sample_shard(rank: u64, shard: Option<crate::Shard>) -> bool {
    shard.is_none_or(|shard| rank % shard.count == shard.index - 1)
}
struct Program {
    path: String,
    source: String,
    execution: Execution,
    /// The expectation it was written under, for the execution counts.
    expectation: &'static str,
    /// The file a variant replaces the primary source of: a package sidecar's
    /// program, or a test program (`classify_package_native`).
    file: Option<std::path::PathBuf>,
}

/// Independent bounds: a per-program rotating window, then a global rank limit.
/// `within` then keeps only the sample's variants of the files whose
/// root-relative path starts with one of its prefixes (all when empty): a
/// change's part of the whole corpus's sample, which still ranks every file.
#[derive(Clone, Copy)]
pub struct Sample<'a> {
    pub limit: usize,
    pub per_program: Option<usize>,
    pub within: &'a [String],
}

/// Full campaigns stay below 200,000 variants on the current corpus. The
/// exhaustive catalogue is still available explicitly, without a budget.
pub const DEFAULT_VARIANTS_PER_PROGRAM: usize = 96;

fn edit_identity(edit: &Edit) -> hirsel_syntax::ContentHash {
    hirsel_syntax::ContentHash::of_parts(
        "hirsel.agree.edit.v1",
        [
            edit.kind.as_bytes(),
            &edit.start.to_le_bytes(),
            &edit.end.to_le_bytes(),
            edit.replacement.as_bytes(),
        ],
    )
}

fn window(
    keys: &BTreeSet<hirsel_syntax::ContentHash>,
    seed: u64,
    budget: usize,
) -> BTreeSet<hirsel_syntax::ContentHash> {
    if keys.is_empty() {
        return BTreeSet::new();
    }
    let start = ((u128::from(seed) * budget as u128) % keys.len() as u128) as usize;
    keys.iter()
        .cycle()
        .skip(start)
        .take(budget.min(keys.len()))
        .copied()
        .collect()
}

fn sampled_catalogue(
    file: &Parsed,
    source: &str,
    seed: u64,
    budget: Option<usize>,
) -> (usize, Vec<Edit>) {
    let Some(budget) = budget else {
        let edits = catalogue(file, source);
        return (edits.len(), edits);
    };
    // Keep identities, not millions of replacement strings. Most programs fit
    // in the budget and need one traversal. Larger ones need a second traversal
    // to materialise only the chosen window in fixed SHA-256 order.
    let mut keys = BTreeSet::new();
    let mut small = BTreeSet::new();
    enumerate(file, source, &mut |edit| {
        if keys.insert(edit_identity(&edit)) {
            if keys.len() <= budget {
                small.insert(edit);
            } else {
                small.clear();
            }
        }
    });
    let generated = keys.len();
    if generated <= budget {
        return (generated, small.into_iter().collect());
    }
    let chosen = window(&keys, seed, budget);
    drop(keys);
    enumerate(file, source, &mut |edit| {
        if chosen.contains(&edit_identity(&edit)) {
            small.insert(edit);
        }
    });
    (generated, small.into_iter().collect())
}

/// A finite global rank limit never needs more than that many candidates from
/// one program. Preserve the exact exhaustive ranks while bounding strings in
/// library clients that request a small global sample without a local budget.
fn ranked_catalogue(
    file: &Parsed,
    source: &str,
    path: &str,
    seed: u64,
    limit: usize,
) -> (usize, Vec<Edit>) {
    let mut identities = BTreeSet::new();
    let mut candidates = BTreeSet::new();
    enumerate(file, source, &mut |edit| {
        if identities.insert(edit_identity(&edit)) && limit > 0 {
            candidates.insert((rank(seed, path, &edit), edit));
            if candidates.len() > limit {
                candidates.pop_last();
            }
        }
    });
    (
        identities.len(),
        candidates.into_iter().map(|(_, edit)| edit).collect(),
    )
}
/// Check the `limit` lowest-ranked variants of the whole corpus under `seed`.
/// Without a limit every seed checks the entire catalogue.
pub fn run(root: &Path, seed: u64, limit: usize) -> Result<Report, String> {
    run_native(root, seed, limit, None)
}

/// `run`, with every valid executed variant also compared under `native`.
pub fn run_native(
    root: &Path,
    seed: u64,
    limit: usize,
    native: Option<&Native>,
) -> Result<Report, String> {
    run_native_with_hash_seed(
        root,
        seed,
        Sample {
            limit,
            per_program: None,
            within: &[],
        },
        native,
        0,
        &[],
        None,
    )
}
/// `run_native` with a hash seed, over the corpus files whose root-relative
/// path starts with one of `only` (every file when it is empty). A variant's
/// rank does not depend on the other files, so a filtered run checks exactly
/// the variants of those files that the whole campaign would. `shard` stripes
/// the globally ranked, truncated sample by rank, before acceptance filtering. Every
/// selected variant belongs to exactly one shard, including skipped variants.
pub fn run_native_with_hash_seed(
    root: &Path,
    seed: u64,
    sample: Sample<'_>,
    native: Option<&Native>,
    hash_seed: u64,
    only: &[String],
    shard: Option<crate::Shard>,
) -> Result<Report, String> {
    let mut paths = vec![];
    crate::discover(&root.join("conformance"), &mut paths)?;
    let mut fixtures = Vec::new();
    if native.is_some() {
        crate::discover(&root.join("native-tests/regressions"), &mut fixtures)?;
        fixtures.sort();
    }
    let unmatched: Vec<_> = only
        .iter()
        .filter(|prefix| {
            !paths.iter().chain(&fixtures).any(|path| {
                path.strip_prefix(root)
                    .is_ok_and(|path| path.to_string_lossy().starts_with(prefix.as_str()))
            })
        })
        .collect();
    if !unmatched.is_empty() {
        return Err(format!(
            "--path prefixes matched no agreement files globally: {unmatched:?}"
        ));
    }
    paths.retain(|path| {
        path.strip_prefix(root).is_ok_and(|relative| {
            let relative = relative.display().to_string();
            if only.is_empty() {
                native
                    .and_then(|n| n.selection.as_ref())
                    .is_none_or(|s| s.selected.contains(&relative))
            } else {
                only.iter()
                    .any(|prefix| relative.starts_with(prefix.as_str()))
            }
        })
    });
    paths.sort();
    let rules = crate::spec_rules(root)?;
    let mut report = Report {
        hash_seed,
        variants_per_program: sample.per_program,
        ..Default::default()
    };
    let mut programs = vec![];
    let mut variants = vec![];
    for path in paths {
        let relative = path.strip_prefix(root).map_err(|e| e.to_string())?;
        let source = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let header = crate::parse_header(relative, &source, &rules)?;
        if matches!(header.expect, crate::Expect::Reject { .. }) {
            continue;
        }
        let Ok(parsed) = parse(&source) else { continue };
        let execution = Execution::of(&header.expect);
        let file = (execution == Execution::Tests || path.with_extension("hir.json").exists())
            .then(|| path.clone());
        let path = relative.display().to_string();
        let (generated, edits) = if sample.per_program.is_none() && sample.limit != usize::MAX {
            ranked_catalogue(&parsed, &source, &path, seed, sample.limit)
        } else {
            sampled_catalogue(&parsed, &source, seed, sample.per_program)
        };
        report.generated += generated;
        report.eligible += sample.per_program.map_or(generated, |n| generated.min(n));
        for edit in edits {
            variants.push((rank(seed, &path, &edit), programs.len(), edit));
        }
        programs.push(Program {
            path,
            source,
            execution,
            expectation: header.expect.kind(),
            file,
        });
    }
    if native.is_some() {
        // Historical reduced programs are exact inputs, not sampled mutation
        // catalogues. Pinning a path in the cover alone would never execute a
        // fixture outside conformance/. Give it one identity edit so it uses
        // the same classification, clustering and stable shard partition.
        for fixture in fixtures {
            let path = fixture
                .strip_prefix(root)
                .map_err(|e| e.to_string())?
                .display()
                .to_string();
            if !only.is_empty() && !only.iter().any(|p| path.starts_with(p)) {
                continue;
            }
            let source = std::fs::read_to_string(&fixture).map_err(|e| e.to_string())?;
            let edit = Edit {
                kind: "historical/exact",
                start: 0,
                end: source.len(),
                replacement: source.clone(),
            };
            variants.push((rank(seed, &path, &edit), programs.len(), edit));
            report.generated += 1;
            report.eligible += 1;
            programs.push(Program {
                path,
                source,
                execution: Execution::Entry,
                expectation: "historical",
                file: fixture
                    .with_extension("hir.json")
                    .exists()
                    .then_some(fixture),
            });
        }
    }
    report.programs = programs.len();
    variants.sort();
    variants.truncate(sample.limit);
    let unmatched: Vec<_> = sample
        .within
        .iter()
        .filter(|prefix| {
            !programs
                .iter()
                .any(|program| program.path.starts_with(prefix.as_str()))
        })
        .collect();
    if !unmatched.is_empty() {
        return Err(format!(
            "--sample-path prefixes matched no agreement programs globally: {unmatched:?}"
        ));
    }
    if variants.is_empty() {
        return Err(format!(
            "agreement selection {only:?} contains no variants globally (limit {})",
            sample.limit
        ));
    }
    variants.retain(|(rank, program, _)| {
        in_sample_shard(*rank, shard)
            && (sample.within.is_empty()
                || sample
                    .within
                    .iter()
                    .any(|prefix| programs[*program].path.starts_with(prefix.as_str())))
    });
    // Current acceptance decides the input set, not the expectation or ratchet.
    let sampled: Vec<usize> = variants
        .iter()
        .map(|v| v.1)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let accepted: BTreeSet<usize> = sampled
        .iter()
        .copied()
        .zip(watched(
            "originals",
            &sampled,
            |p| programs[*p].path.clone(),
            |p| {
                classify_package_native(
                    &programs[*p].source,
                    Execution::None,
                    None,
                    programs[*p].file.as_deref(),
                    hash_seed,
                )
            },
        ))
        .filter_map(|(p, c)| (c == Classification::Valid).then_some(p))
        .collect();
    let sampled = variants.len();
    variants.retain(|v| accepted.contains(&v.1));
    report.skipped = sampled - variants.len();
    // Each cluster's first variant goes to stderr as soon as it is found, so a
    // run stopped before its report still names every kind it saw.
    let streamed = Mutex::new(BTreeSet::<(String, &'static str)>::new());
    let outcomes = watched(
        "variants",
        &variants,
        |(_, program, edit)| {
            format!(
                "{} {} {}..{}",
                programs[*program].path, edit.kind, edit.start, edit.end
            )
        },
        |(_, program, edit)| {
            let program = &programs[*program];
            let matrix = native.map(|n| n.for_program(&program.path));
            let source = edit.apply(&program.source);
            let outcome = classify_package_native(
                &source,
                program.execution,
                native.zip(matrix.as_deref()),
                program.file.as_deref(),
                hash_seed,
            );
            if let Classification::Bug { kind, detail } = &outcome
                && streamed.lock().unwrap().insert((kind.clone(), edit.kind))
            {
                eprintln!(
                    "{}",
                    streamed_cluster(kind, edit.kind, &program.path, &source, detail)
                );
            }
            outcome
        },
    );
    for ((_, program, edit), outcome) in variants.into_iter().zip(outcomes) {
        let program = &programs[program];
        report.checked += 1;
        *report.edits.entry(edit.kind).or_default() += 1;
        match outcome {
            Classification::Rejected => report.rejected += 1,
            Classification::Valid => {
                report.valid += 1;
                if program.execution != Execution::None {
                    report.executed += 1;
                    *report.executions.entry(program.expectation).or_default() += 1;
                }
            }
            Classification::Unavailable => report.unavailable += 1,
            Classification::RuntimeLimit => {
                report.valid += 1;
                report.runtime_limits += 1;
            }
            Classification::Bug { kind, detail } => {
                let cluster = report
                    .clusters
                    .entry((kind, edit.kind))
                    .or_insert_with(|| Cluster {
                        count: 0,
                        path: program.path.clone(),
                        source: edit.apply(&program.source),
                        detail,
                        execution: program.execution,
                        file: program.file.clone(),
                    });
                cluster.count += 1;
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unmatched_agreement_selectors_refuse_before_shard_assignment() {
        let root = std::env::temp_dir().join(format!("hirsel-agree-empty-{}", std::process::id()));
        std::fs::create_dir_all(root.join("conformance/LANG")).unwrap();
        std::fs::create_dir_all(root.join(crate::SPEC_DIR)).unwrap();
        std::fs::write(
            root.join(crate::SPEC_DIR).join("language.md"),
            "### LANG-7: Types\n",
        )
        .unwrap();
        let fixture = "conformance/LANG/LANG-7-fixture.hir";
        std::fs::write(root.join(fixture), "// rule: LANG-7\n// expect: accept\nfn probe(mut x: u64) -> u64 { let y = x; x = y; x }\n").unwrap();
        for shard in [None, Some(crate::Shard { index: 2, count: 4 })] {
            for only in [
                vec!["removed_case".into()],
                vec![fixture.into(), "removed_case".into()],
            ] {
                let error = run_native_with_hash_seed(
                    &root,
                    1,
                    Sample {
                        limit: 8,
                        per_program: Some(8),
                        within: &[],
                    },
                    None,
                    0,
                    &only,
                    shard,
                )
                .err()
                .expect("stale selector must fail");
                assert!(
                    error.contains("--path") && error.contains("removed_case"),
                    "{error}"
                );
            }
            let within = ["removed_case".into()];
            let error = run_native_with_hash_seed(
                &root,
                1,
                Sample {
                    limit: 8,
                    per_program: Some(8),
                    within: &within,
                },
                None,
                0,
                &[],
                shard,
            )
            .err()
            .expect("stale sample selector must fail");
            assert!(
                error.contains("--sample-path") && error.contains("removed_case"),
                "{error}"
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_streamed_cluster_is_one_line_of_the_report_fields() {
        let line = streamed_cluster(
            "native/heap/k",
            "statement/swap",
            "a.hir",
            "fn f() {\n}\n",
            "d",
        );
        let json = line.strip_prefix("agree: cluster ").expect("prefixed");
        assert!(!json.contains('\n'));
        let value: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(value["kind"], "native/heap/k");
        assert_eq!(value["edit"], "statement/swap");
        assert_eq!(value["path"], "a.hir");
        assert_eq!(value["source"], "fn f() {\n}\n");
        assert_eq!(value["detail"], "d");
    }
    #[test]
    fn per_program_windows_cover_the_catalogue_and_preserve_stable_ranks() {
        let keys: BTreeSet<_> = (0u32..251)
            .map(|i| hirsel_syntax::ContentHash::of_bytes(&i.to_le_bytes()))
            .collect();
        let mut visited = BTreeSet::new();
        for seed in 0..3 {
            let selected = window(&keys, seed, 96);
            assert_eq!(selected.len(), 96);
            assert_eq!(selected, window(&keys, seed, 96));
            visited.extend(selected);
        }
        assert_eq!(visited, keys);
        assert_eq!(window(&keys, u64::MAX, 999), keys);
        assert!(window(&keys, 42, 0).is_empty());
        let source = "fn f(x: u64) -> u64 { let mut a: u64 = x; let b: u64 = a; a = b; a }";
        let parsed = parse(source).unwrap();
        let full = catalogue(&parsed, source);
        for seed in 0..5 {
            for limit in [0, 1, 7, full.len(), full.len() + 1] {
                let (generated, actual) = ranked_catalogue(&parsed, source, "fixture", seed, limit);
                let mut expected: Vec<_> = full.iter().cloned().collect();
                expected.sort_by_key(|edit| (rank(seed, "fixture", edit), edit.clone()));
                expected.truncate(limit);
                assert_eq!(generated, full.len());
                assert_eq!(actual, expected);
            }
        }
        let keys: BTreeSet<_> = full.iter().map(edit_identity).collect();
        for seed in 0..5 {
            let (generated, edits) = sampled_catalogue(&parsed, source, seed, Some(7));
            assert_eq!(generated, full.len());
            assert_eq!(
                edits.iter().map(edit_identity).collect::<BTreeSet<_>>(),
                window(&keys, seed, 7)
            );
        }
        assert_eq!(
            sampled_catalogue(&parsed, source, 4, None),
            (full.len(), full.clone())
        );
        assert_eq!(
            sampled_catalogue(&parsed, source, 4, Some(full.len())),
            (full.len(), full)
        );
    }
    #[test]
    fn shards_partition_the_global_sample_once_including_empty_shards() {
        for (selected, shards) in [(20_000, 32), (19, 4), (3, 8), (0, 8)] {
            let mut catalogue: Vec<_> = (0..selected + 50).map(mix).collect();
            catalogue.sort();
            catalogue.truncate(selected as usize);
            let mut union = vec![];
            for index in 1..=shards {
                let shard = Some(crate::Shard {
                    index,
                    count: shards,
                });
                union.extend(
                    catalogue
                        .iter()
                        .copied()
                        .filter(|rank| in_sample_shard(*rank, shard)),
                );
            }
            union.sort();
            assert_eq!(union, catalogue);
        }
        // The same variant keeps its shard when unrelated variants are added.
        assert_eq!(
            (1..=32)
                .filter(|index| in_sample_shard(
                    mix(77),
                    Some(crate::Shard {
                        index: *index,
                        count: 32
                    })
                ))
                .count(),
            1
        );
    }
    #[test]
    fn agreement_shards_have_the_same_report_as_an_unsharded_fixture() {
        let root = std::env::temp_dir().join(format!(
            "hirsel-agree-shards-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("conformance/LANG")).unwrap();
        std::fs::create_dir_all(root.join(crate::SPEC_DIR)).unwrap();
        std::fs::write(
            root.join(crate::SPEC_DIR).join("language.md"),
            "### LANG-7: Types\n",
        )
        .unwrap();
        std::fs::write(root.join("conformance/LANG/LANG-7-fixture.hir"),
                      "// rule: LANG-7\n// expect: accept\nfn probe(mut x: u64) -> u64 { let y = x; x = y; x }\n").unwrap();
        let whole = run(&root, 1, 32).unwrap().json();
        assert!(whole["checked"].as_u64().unwrap() > 0);
        let mut summed = whole.clone();
        for key in [
            "checked",
            "skipped",
            "rejected",
            "valid",
            "unavailable",
            "runtime_limits",
            "executed",
        ] {
            summed[key] = 0.into();
        }
        summed["edits"] = serde_json::json!({});
        summed["clusters"] = serde_json::json!([]);
        for index in 1..=4 {
            let part = run_native_with_hash_seed(
                &root,
                1,
                Sample {
                    limit: 32,
                    per_program: None,
                    within: &[],
                },
                None,
                0,
                &[],
                Some(crate::Shard { index, count: 4 }),
            )
            .unwrap()
            .json();
            assert_eq!(part["programs"], whole["programs"]);
            assert_eq!(part["generated"], whole["generated"]);
            assert_eq!(part["clusters"], serde_json::json!([]));
            for key in [
                "checked",
                "skipped",
                "rejected",
                "valid",
                "unavailable",
                "runtime_limits",
                "executed",
            ] {
                summed[key] = (summed[key].as_u64().unwrap() + part[key].as_u64().unwrap()).into();
            }
            for (edit, count) in part["edits"].as_object().unwrap() {
                let previous = summed["edits"][edit].as_u64().unwrap_or(0);
                summed["edits"][edit] = (previous + count.as_u64().unwrap()).into();
            }
        }
        std::fs::remove_dir_all(root).unwrap();
        assert_eq!(summed, whole);
    }
    #[test]
    fn fail_and_test_variants_execute_on_every_engine() {
        let root = std::env::temp_dir().join(format!(
            "hirsel-agree-executions-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        for area in ["SEM", "LANG"] {
            std::fs::create_dir_all(root.join("conformance").join(area)).unwrap();
        }
        std::fs::create_dir_all(root.join(crate::SPEC_DIR)).unwrap();
        std::fs::write(
            root.join(crate::SPEC_DIR).join("language.md"),
            "### SEM-1: Integers\n### LANG-9: Tests\n",
        )
        .unwrap();
        std::fs::write(
            root.join("conformance/SEM/SEM-1-fixture-fail.hir"),
            "// rule: SEM-1\n// expect: fail SEM-1 at line 5\npub fn main(rt: Io, args: Args) -> Result[(), CliError] {\n  let mut k: u8 = 8u8\n  let n = 1u8 << k\n  Ok(())\n}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("conformance/LANG/LANG-9-fixture.hir"),
            "// rule: LANG-9\n// expect: test\n@test\nfn example(t: Test) {\n  let mut ok: bool = true\n  test.check(t, ok)\n}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("conformance/LANG/LANG-9-fixture.hir.json"),
            r#"{"schema":1,"package":{"name":"test_fixture","version":"0"},"sources":[{"path":"LANG-9-fixture.hir","module":[]}],"entry":{"module":[],"name":"main"},"dependencies":[]}"#,
        )
        .unwrap();
        let report = run(&root, 1, usize::MAX).unwrap();
        std::fs::remove_dir_all(root).unwrap();
        assert!(report.clusters.is_empty(), "{:?}", report.clusters);
        assert!(
            report.executions.get("fail").is_some_and(|&n| n > 0),
            "{report:?}"
        );
        assert!(
            report.executions.get("test").is_some_and(|&n| n > 0),
            "{report:?}"
        );
        assert_eq!(report.executed, report.executions.values().sum::<usize>());
    }
    #[test]
    fn a_sample_within_files_partitions_the_whole_corpus_sample() {
        let root = std::env::temp_dir().join(format!(
            "hirsel-agree-within-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("conformance/LANG")).unwrap();
        std::fs::create_dir_all(root.join(crate::SPEC_DIR)).unwrap();
        std::fs::write(
            root.join(crate::SPEC_DIR).join("language.md"),
            "### LANG-7: Types\n",
        )
        .unwrap();
        for (name, body) in [
            ("a", "fn probe(mut x: u64) -> u64 { let y = x; x = y; x }"),
            (
                "b",
                "fn probe(x: u64, y: u64) -> u64 { let z = x + y; z * 2u64 }",
            ),
        ] {
            std::fs::write(
                root.join(format!("conformance/LANG/LANG-7-{name}.hir")),
                format!("// rule: LANG-7\n// expect: accept\n{body}\n"),
            )
            .unwrap();
        }
        let sample = |within: &[String]| {
            run_native_with_hash_seed(
                &root,
                1,
                Sample {
                    limit: 24,
                    per_program: Some(DEFAULT_VARIANTS_PER_PROGRAM),
                    within,
                },
                None,
                0,
                &[],
                None,
            )
            .unwrap()
            .json()
        };
        let whole = sample(&[]);
        let selected = |report: &serde_json::Value| {
            report["checked"].as_u64().unwrap() + report["skipped"].as_u64().unwrap()
        };
        assert_eq!(selected(&whole), 24);
        let parts: Vec<_> = ["a", "b"]
            .map(|name| sample(&[format!("conformance/LANG/LANG-7-{name}.hir")]))
            .into();
        // Each file keeps its own share of the 24, not 24 of its own.
        assert!(parts.iter().all(|part| selected(part) < 24));
        assert_eq!(parts.iter().map(selected).sum::<u64>(), 24);
        for part in &parts {
            assert_eq!(part["generated"], whole["generated"]);
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn catalogue_holds_a_binding_inside_a_sequence_element() {
        let source = "type R { name: String }\nfn view(x: String) -> String in x { copy x }\nfn count(s: String) -> u64 { let r: R = R(name: view(s)); string.byte_len(r.name) }";
        let edits = catalogue(&parse(source).unwrap(), source);
        let held = edits
            .iter()
            .find(|e| e.kind == "holder/sequence-element")
            .unwrap()
            .apply(source);
        assert!(
            held.contains(
                ": List[(R, u64)] = [(R(name: view(s)), 0u64)]\nlet (r, _) = agree_held_"
            ),
            "{held}"
        );
        // The record keeps the view of `s` through the element until `r.name`
        // is read, on both checkers.
        assert_eq!(
            classify(&held, Execution::None),
            Classification::Valid,
            "{held}"
        );
    }
    #[test]
    fn catalogue_holds_literals_related_bindings_and_block_tails() {
        let source = "fn scratch(a: Arena) -> u64 { let text: String in a = \"scratch\"; string.byte_len(text) }\nfn direct() -> u64 { list.len({ let v: List[List[u64]] = [[1, 2]]; v }) }";
        let edits = catalogue(&parse(source).unwrap(), source);
        let edit = |kind| edits.iter().find(|e| e.kind == kind).unwrap().apply(source);
        // A literal initializer is held too; the related variant also places
        // the holder where the binding's relation says (PROV-4).
        assert!(edit("holder/sequence-element").contains(
            ": List[(String, u64)] = [(\"scratch\", 0u64)]\nlet (text, _): (String, u64) in a"
        ));
        let related = edit("holder/related-sequence");
        assert!(
            related.contains(": List[(String, u64)] in a = [(\"scratch\", 0u64)]"),
            "{related}"
        );
        assert_eq!(
            classify(&related, Execution::None),
            Classification::Valid,
            "{related}"
        );
        // The block would yield a part of a list it ends (LANG-11).
        let tail = edit("holder/tail-element");
        assert!(
            tail.contains("let agree_tail: List[List[List[u64]]] = [v]\nagree_tail[0]"),
            "{tail}"
        );
        assert_eq!(
            classify(&tail, Execution::None),
            Classification::Rejected,
            "{tail}"
        );
        // A closure that captures the block's Global list returns a share of
        // it, which leaves the block. A closure's result over a borrowed
        // parameter stays local to that argument (CLO-1), here to `v`.
        for (kind, text, expected) in [
            (
                "tail/closure-capture",
                "let agree_f = fn() { v }\nagree_f()",
                Classification::Valid,
            ),
            (
                "tail/local-result",
                "(fn(agree_x: List[List[u64]]) { agree_x })(v)",
                Classification::Rejected,
            ),
        ] {
            let tail = edit(kind);
            assert!(tail.contains(text), "{tail}");
            assert_eq!(classify(&tail, Execution::None), expected, "{tail}");
        }
    }
    #[test]
    fn catalogue_holds_a_called_closure_result_in_a_sequence_element() {
        // The inner literal's row is local to the called closure; its content
        // is Global, so the list's Global element receives it on both checkers.
        let source = "fn probe() -> u64 { let pair: (fn(k: u64) -> u64, u64) = (fn(k: u64) { k }, 1u64); let (f, n) = pair; f(n) }";
        let edits = catalogue(&parse(source).unwrap(), source);
        let made = edits
            .iter()
            .find(|e| e.kind == "holder/closure-result")
            .unwrap()
            .apply(source);
        assert!(
            made.contains("= [(fn() { ((fn(k: u64) { k }, 1u64), 0u64) })()]\nlet (pair, _)"),
            "{made}"
        );
        assert_eq!(
            classify(&made, Execution::None),
            Classification::Valid,
            "{made}"
        );
    }
    #[test]
    fn catalogue_makes_a_typed_initializer_the_result_of_a_called_closure() {
        // The written `@noalloc` part receives the inferred literal's row
        // from a call result: the row carries the evidence its body proves
        // (MEM-5), in a list element and under the annotation itself.
        let source = "@noalloc fn seven() -> u64 { 7 }\nfn probe() -> u64 { let pair: (@noalloc fn() -> u64 uses {}, u64) = (fn() { seven() }, 1u64); let (f, n) = pair; f() + n }";
        let edits = catalogue(&parse(source).unwrap(), source);
        for (kind, text) in [
            (
                "holder/closure-value",
                "= (fn() { (fn() { seven() }, 1u64) })(); let (f, n)",
            ),
            (
                "holder/closure-result",
                "= [(fn() { ((fn() { seven() }, 1u64), 0u64) })()]\nlet (pair, _)",
            ),
        ] {
            let made = edits.iter().find(|e| e.kind == kind).unwrap().apply(source);
            assert!(made.contains(text), "{made}");
            assert_eq!(
                classify(&made, Execution::None),
                Classification::Valid,
                "{made}"
            );
            // The twin whose literal may allocate has no evidence.
            let allocating = made.replacen("@noalloc fn seven", "fn seven", 1);
            assert_eq!(
                classify(&allocating, Execution::None),
                Classification::Rejected,
                "{allocating}"
            );
        }
    }
    #[test]
    fn nested_callables_are_received_by_position_on_both_checkers() {
        // CLO-2 (#58): a part or result may promise more than its place, a
        // parameter may ask for less. The corpus pins CLO-2-nested-uses-*;
        // the first program stays here because its holder edit seeds the
        // open relation disagreement of a called closure's closure part.
        for source in [
            "fn probe() -> u64 { let pair: (fn(k: u64) -> u64, u64) = (fn() { (fn(k: u64) { k }, 1u64) })(); 2 }",
            "fn probe() -> () { let held: (fn() -> (fn() -> () uses {}), u64) = (fn() { fn() { () } }, 0u64); () }",
            "fn probe() -> () { let h: (fn(g: fn() -> () uses {}) -> (), u64) = (fn(g: fn() -> ()) { g() }, 0u64); () }",
        ] {
            assert_eq!(
                classify(source, Execution::None),
                Classification::Valid,
                "{source}"
            );
        }
        for source in [
            "fn probe() -> () { let h: (fn(g: fn() -> ()) -> (), u64) = (fn(g: fn() -> () uses {}) { g() }, 0u64); () }",
            "fn probe(rt: Stdout) -> () { let h: (fn() -> (fn() -> () uses {}), u64) = (fn() { fn() { let _ = out.print(rt, \"x\") } }, 0u64); () }",
        ] {
            assert_eq!(
                classify(source, Execution::None),
                Classification::Rejected,
                "{source}"
            );
        }
    }
    #[test]
    fn catalogue_compares_values_through_relations() {
        let source = "fn f(a: Arena, x: Bytes in a) -> Bytes in {a, x} { x }\nfn g(a: Arena) -> bool { let h: Bytes = b\"one\"; let r = f(a, h); true }";
        let edits = catalogue(&parse(source).unwrap(), source);
        let compared = edits
            .iter()
            .find(|e| e.kind == "comparison/relation" && e.replacement.contains("r == h"))
            .unwrap()
            .apply(source);
        assert!(
            matches!(classify(&compared, Execution::None), Classification::Valid),
            "{compared}"
        );
    }
    #[test]
    fn owned_view_yield_returns_agree_with_independent_ir_checking() {
        for source in [
            include_str!("../../../conformance/SEM/SEM-4-view-return-source-run.hir"),
            include_str!("../../../conformance/SEM/SEM-4-view-return-transparent-run.hir"),
            include_str!("../../../conformance/SEM/SEM-4-view-return-direct-run.hir"),
        ] {
            for source in [
                source.to_owned(),
                source
                    .replace("return piece", "return copy piece")
                    .replace("=> piece,", "=> copy piece,"),
            ] {
                assert_eq!(
                    classify(&source, Execution::None),
                    Classification::Valid,
                    "{source}"
                );
            }
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for (path, expected) in [
            (
                "conformance/SEM/SEM-4-view-return-graphemes-run.hir",
                Classification::Valid,
            ),
            (
                "conformance/MODE/MODE-1-view-return-graphemes-bad.hir",
                Classification::Rejected,
            ),
        ] {
            let path = root.join(path);
            let source = std::fs::read_to_string(&path).unwrap();
            // The full grapheme program exceeds the campaign's instruction
            // budget; corpus/native execution covers both return forms.
            assert_eq!(
                classify_package_native(&source, Execution::None, None, Some(&path), 0),
                expected,
                "{}",
                path.display()
            );
        }
    }
}
