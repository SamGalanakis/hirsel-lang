//! The Cranelift parity gate: every case the LLVM backend passes must pass as a
//! Cranelift executable, observing exactly what the reference does. A case runs
//! once as an unoptimized LLVM executable and once as a Cranelift one, both
//! linked with the diagnostic archive (poisoning, header checks and the exit
//! leak audit). A case LLVM does not pass is outside the gate.
//!
//! The cases LLVM passes and Cranelift does not are rows of
//! `native-tests/regressions/cranelift-parity.tsv`, each with its reason. A
//! failure without a row, a row whose reason changed and a row whose case now
//! passes all fail the gate, so parity only grows.

use super::{Case, CaseResult, Class, exclusion};
use hirsel_driver::native::{
    Configuration, Engine, OptLevel, Options, Outcome, Partition, Runtime, TargetSpec,
};
use std::collections::BTreeMap;
use std::path::Path;

pub const PARITY: &str = "native-tests/regressions/cranelift-parity.tsv";
/// The cases the per-push native smoke also runs as Cranelift executables.
pub const SMOKE: &str = "native-tests/regressions/cranelift-smoke.txt";

/// The LLVM configuration a Cranelift run is held to, and the Cranelift one.
pub fn matrix() -> Vec<Configuration> {
    let options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
    vec![
        Configuration {
            options: options.clone(),
            engine: Engine::Native {
                opt: OptLevel::None,
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
        },
        cranelift(options),
    ]
}

/// The Cranelift configuration of `options`: one object, diagnostic archive.
pub fn cranelift(options: Options) -> Configuration {
    Configuration {
        options,
        engine: Engine::Cranelift {
            partition: Partition::Whole,
            runtime: Runtime::Diagnostic,
        },
    }
}

/// The parity set: the native baseline and every program under
/// `native-tests/regressions` and `native-tests/library`.
pub fn cases(root: &Path) -> Result<Vec<Case>, String> {
    let mut cases = super::baseline(root)?;
    for dir in ["native-tests/regressions", "native-tests/library"] {
        let mut paths: Vec<_> = std::fs::read_dir(root.join(dir))
            .map_err(|e| format!("{dir}: {e}"))?
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|path| path.extension().is_some_and(|e| e == "hir"))
            .collect();
        paths.sort();
        for path in paths {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
            let id = format!("{dir}/{name}");
            let source = std::fs::read_to_string(&path).map_err(|e| format!("{id}: {e}"))?;
            cases.push(Case {
                id,
                source,
                path: Some(path),
            });
        }
    }
    Ok(cases)
}

/// Whether the case is in the gate (LLVM agrees), and if so Cranelift's
/// failure reason, or `None` when it agrees too.
pub fn verdict(result: &CaseResult) -> Option<Option<String>> {
    let outcome = |class| {
        result
            .outcomes
            .iter()
            .find(|(_, c, _)| *c == class)
            .map(|(_, _, outcome)| outcome)
    };
    if result.snapshot.is_some() || !matches!(outcome(Class::Llvm), Some(Outcome::Agree)) {
        return None;
    }
    Some(match outcome(Class::Cranelift) {
        Some(outcome) => exclusion(outcome).map(|e| format!("{}: {}", e.class, e.reason)),
        None => Some("no Cranelift run".into()),
    })
}

/// `case -> reason` rows of the parity list.
pub fn parse(text: &str) -> Result<BTreeMap<String, String>, String> {
    let mut rows = BTreeMap::new();
    for (index, line) in text.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (case, reason) = line
            .split_once('\t')
            .ok_or_else(|| format!("{PARITY}:{}: not `case<TAB>reason`", index + 1))?;
        if rows.insert(case.to_string(), reason.to_string()).is_some() {
            return Err(format!("{PARITY}:{}: `{case}` listed twice", index + 1));
        }
    }
    Ok(rows)
}

const HEADER: &str = "# Cases the LLVM backend passes and a Cranelift executable does not, with the\n\
# reason (`native --cranelift`). Each row is unfinished backend work, not a waiver;\n\
# a row whose case now passes fails the gate until it is removed.\n\
# case\treason\n";

/// The parity list `results` imply.
pub fn render(results: &[CaseResult]) -> String {
    let mut out = HEADER.to_string();
    for result in results {
        if let Some(Some(reason)) = verdict(result) {
            out.push_str(&format!("{}\t{reason}\n", result.id));
        }
    }
    out
}

/// What a parity run found against the list.
#[derive(Debug, Default)]
pub struct Report {
    /// Cases LLVM passes, and how many of them Cranelift passes.
    pub gated: usize,
    pub passing: usize,
    /// Failures without a row, or with another reason than their row's.
    pub unlisted: Vec<(String, String)>,
    /// Rows whose case now passes.
    pub stale: Vec<String>,
}

impl Report {
    pub fn ok(&self) -> bool {
        self.unlisted.is_empty() && self.stale.is_empty()
    }
}

/// Judge `results` against the `listed` rows of the cases they ran.
pub fn judge(results: &[CaseResult], listed: &BTreeMap<String, String>) -> Report {
    let mut report = Report::default();
    for result in results {
        let Some(failure) = verdict(result) else {
            continue;
        };
        report.gated += 1;
        match (failure, listed.get(&result.id)) {
            (None, None) => report.passing += 1,
            (None, Some(_)) => {
                report.passing += 1;
                report.stale.push(result.id.clone());
            }
            (Some(reason), row) if row != Some(&reason) => {
                report.unlisted.push((result.id.clone(), reason))
            }
            (Some(_), _) => {}
        }
    }
    report
}

/// One case's Ready program as Cranelift IR text (`native --cranelift --clif`).
pub fn clif(case: &Case) -> Result<String, String> {
    let mut engine = hirsel_engine::QueryEngine::new(
        hirsel_engine::default_environment(),
        hirsel_driver::compiler_adapters(),
    );
    let (validated, _, _) = super::checked(&mut engine, &case.source, case.path.as_deref())?;
    let program = hirsel_driver::native::build(
        &validated,
        &Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
    )
    .map_err(|e| e.to_string())?;
    hirsel_driver::native::emit_clif(&program).map_err(|e| e.to_string())
}

/// One case's emission wall time per backend, in the order `BACKENDS` names
/// them: the same Ready program lowered to one object by each.
pub fn compile_times(case: &Case) -> Result<Vec<std::time::Duration>, String> {
    use hirsel_driver::native::{Backend, build, emit_backend_objects};
    let mut engine = hirsel_engine::QueryEngine::new(
        hirsel_engine::default_environment(),
        hirsel_driver::compiler_adapters(),
    );
    let (validated, _, _) = super::checked(&mut engine, &case.source, case.path.as_deref())?;
    let program = build(
        &validated,
        &Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
    )
    .map_err(|e| e.to_string())?;
    BACKENDS
        .iter()
        .map(|(_, backend, opt)| {
            let started = std::time::Instant::now();
            emit_backend_objects(&program, *backend, *opt, Partition::Whole, false, None)
                .map_err(|e| e.to_string())?;
            Ok(started.elapsed())
        })
        .collect()
}

/// The backends `compile_times` times.
pub const BACKENDS: [(&str, hirsel_driver::native::Backend, OptLevel); 3] = [
    (
        "cranelift",
        hirsel_driver::native::Backend::Cranelift,
        OptLevel::None,
    ),
    (
        "llvm-O0",
        hirsel_driver::native::Backend::Llvm,
        OptLevel::None,
    ),
    (
        "llvm-O2",
        hirsel_driver::native::Backend::Llvm,
        OptLevel::Default,
    ),
];
