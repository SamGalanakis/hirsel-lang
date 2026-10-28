//! `hirsel check` lowers and validates exactly the bodies compile-time evaluation
//! executes; run, test and build lower every reachable body.
use hirsel_driver::*;
use hirsel_engine::*;
use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

type Calls = std::sync::Arc<std::sync::Mutex<BTreeSet<String>>>;
thread_local! {
    static CALLS: std::cell::RefCell<Option<Calls>> = const { std::cell::RefCell::new(None) };
}
struct CountLower {
    inner: std::sync::Arc<dyn LoweringPhase>,
    calls: Calls,
}
impl LoweringPhase for CountLower {
    fn lower(
        &self,
        t: &hirsel_types::TypeInterner,
        m: &hirsel_types::TypeMemo,
        body: &AnalyzedBody,
        comptime: &Comptime,
        signature: &hirsel_types::Signature,
    ) -> Result<QueryOutput<hirsel_tir::Definition>, QueryError> {
        self.calls
            .lock()
            .unwrap()
            .insert(signature.definition.path().to_string());
        self.inner.lower(t, m, body, comptime, signature)
    }
    fn lower_evaluation(
        &self,
        t: &hirsel_types::TypeInterner,
        m: &hirsel_types::TypeMemo,
        body: &AnalyzedBody,
        staged: &[(hirsel_syntax::ExprId, hirsel_types::TypeId)],
        signature: &hirsel_types::Signature,
        expression: hirsel_syntax::ExprId,
        result: hirsel_types::TypeId,
        constants: &[(hirsel_syntax::ExprId, hirsel_tir::Materialized)],
    ) -> Result<hirsel_tir::Definition, QueryError> {
        self.calls
            .lock()
            .unwrap()
            .insert(signature.definition.path().to_string());
        self.inner
            .lower_evaluation(t, m, body, staged, signature, expression, result, constants)
    }
    fn entry_body(&self) -> hirsel_tir::BodyId {
        self.inner.entry_body()
    }
}
fn counted_adapters() -> PhaseAdapters {
    let mut adapters = compiler_adapters();
    let calls = CALLS.with(|calls| calls.borrow().as_ref().unwrap().clone());
    adapters.lowering = Some(std::sync::Arc::new(CountLower {
        inner: adapters.lowering.take().unwrap(),
        calls,
    }));
    adapters
}

/// The definitions lowered when `source` checks under `lowering`, by name.
fn lowered(source: &str, lowering: LoweringPolicy) -> BTreeSet<String> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "hirsel-check-lowering-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path: PathBuf = dir.join("main.hir");
    std::fs::write(&path, source).unwrap();
    // This oracle measures only the supplied program's lowering, without any
    // dependency's compile-time bodies or prechecks.
    std::fs::write(
        path.with_extension("hir.json"),
        serde_json::json!({
            "schema": 1, "package": {"name":"standalone", "version":"0"},
            "sources": [{"path":"main.hir", "module":[]}],
            "dependencies": [], "entry": {"module":[], "name":"main"},
        })
        .to_string(),
    )
    .unwrap();
    let names = with_compiler_stack(|| {
        use hirsel_driver::requests::{CompileOperation, CompileResult, CompilerRequests};
        let calls = Calls::default();
        CALLS.with(|slot| *slot.borrow_mut() = Some(calls.clone()));
        let config = CompileConfig {
            lowering,
            ..Default::default()
        };
        let compiler = COMPILER.with_config(config.clone()).with_phase_adapters(
            counted_adapters,
            hirsel_syntax::ContentHash::of_bytes(b"hirsel.check-lowering.physical-calls.v1").0,
        );
        let mut requests = CompilerRequests::for_host(compiler, config);
        let output = requests
            .execute(&path, CompileOperation::Check { lowering })
            .unwrap();
        let CompileResult::Check(checked) = &output.result else {
            panic!("expected source Check");
        };
        assert_eq!(checked.completion, CheckCompletion::Complete);
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        CALLS.with(|slot| slot.borrow_mut().take());
        let calls = calls.lock().unwrap();
        // Initializer roots have no original source Check memo. Verify their
        // owners separately; the original source-body set below stays exact.
        let initializers: BTreeSet<_> = calls
            .iter()
            .filter_map(|name| name.strip_suffix("$generator"))
            .map(source_name)
            .collect();
        assert!(initializers.is_subset(&names(&["clamp", "width$instance"])));
        calls
            .iter()
            .filter(|name| !name.ends_with("$generator"))
            // A generic instance's path ends in its arguments' digest.
            .map(|path| source_name(path))
            .collect()
    });
    std::fs::remove_dir_all(&dir).unwrap();
    names
}

fn source_name(path: &str) -> String {
    match path.split_once("$instance$") {
        Some((declaration, _)) => format!("{declaration}$instance"),
        None => path.into(),
    }
}

fn names(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|n| format!("standalone@0/{n}")).collect()
}

const UNEXECUTED: &str = r#"
fn unused() -> u64 {
  helper()
}

fn helper() -> u64 {
  2
}

pub fn main() -> Result[(), ()] {
  let _ = unused()
  Ok(())
}
"#;

const COMPTIME: &str = r#"
const LIMIT: u64 = comptime triple(2)

fn triple(x: u64) -> u64 {
  x * 3
}

fn square(x: u64) -> u64 {
  x * x
}

fn folded() -> u64 {
  comptime square(4)
}

fn reader() -> u64 {
  LIMIT
}

fn runtime_only() -> u64 {
  7
}

fn sized(limit: u64) -> Result[Body[fn(u64) -> u64 uses {}], String] {
  Ok(codegen.body("fn(x: u64) -> u64 uses {} { x }"))
}

fn clamp(n: u64) -> u64 uses {} = comptime sized(LIMIT)

type Point {
  x: u64,
}

fn width_body[T](s: TypeShape[T]) -> Result[Body[fn(T) -> u64 uses {}], String] uses {} {
  Ok(codegen.body("fn(v: standalone@0/Point) -> u64 uses {} { v.x }"))
}

fn width[T](v: T) -> u64 uses {} = comptime width_body(shape(T))

pub fn main() -> Result[(), ()] {
  let _ = folded() + reader() + runtime_only() + clamp(1) + width(Point(x: 2))
  Ok(())
}
"#;

#[test]
fn check_lowers_exactly_the_bodies_compile_time_evaluation_executes() {
    // The constant and the function its evaluation calls, the body holding a
    // `comptime` expression and the function it calls, the body materializing
    // the constant, the generators and the generic instance CT-5 selects. A
    // generated body is checked from its text, so a non-generic one (`clamp`)
    // lowers only when the program runs.
    let executed = names(&[
        "LIMIT",
        "folded",
        "reader",
        "sized",
        "square",
        "triple",
        "width$instance",
        "width_body",
    ]);
    assert_eq!(lowered(COMPTIME, LoweringPolicy::Comptime), executed);
    let mut reachable = executed;
    reachable.extend(names(&["clamp", "main", "runtime_only"]));
    assert_eq!(lowered(COMPTIME, LoweringPolicy::Reachable), reachable);
}
