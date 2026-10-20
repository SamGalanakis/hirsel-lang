//! Every executable corpus case, independent of the passing ratchet. The only
//! pre-execution omissions are named host/safety exclusions. Backend failures
//! remain executable pending obligations, checked for changed and stale reasons.
use super::{Case, CaseResult, Class, Exclusions};
use hirsel_cli_args::Shard;
use hirsel_driver::native::{Configuration, Outcome, TargetSpec, Toolchain};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

pub const OMIT: &str = "native-tests/regressions/corpus-exclusions.tsv";
pub const PENDING: &str = "native-tests/regressions/corpus-pending.tsv";

pub fn cases(root: &Path) -> Result<Vec<Case>, String> {
    let rules = crate::spec_rules(root)?;
    crate::Selection::default()
        .files(root)?
        .into_iter()
        .filter_map(|path| {
            let id = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let read = || {
                let source = std::fs::read_to_string(&path).map_err(|e| format!("{id}: {e}"))?;
                let header = crate::parse_header(Path::new(&id), &source, &rules)?;
                Ok((source, header))
            };
            match read() {
                Ok((source, header))
                    if matches!(
                        header.expect,
                        crate::Expect::Run { .. }
                            | crate::Expect::Fail { .. }
                            | crate::Expect::Blocked { .. }
                            | crate::Expect::Test
                    ) =>
                {
                    Some(Ok(Case {
                        id,
                        source,
                        path: Some(path),
                    }))
                }
                Ok(_) => None,
                Err(e) => Some(Err(e)),
            }
        })
        .collect()
}

/// A case naming runners: the native harness has no runner fixtures (sim
/// nodes, threads), so it runs only interpreted (tools/buck2/impact.py).
pub fn interpreter_only(case: &Case) -> bool {
    crate::fixture(&case.source).is_ok_and(|f| !f.runners.is_empty())
}

pub fn omissions(text: &str, cases: &[Case]) -> Result<BTreeMap<String, String>, String> {
    let ids: BTreeSet<_> = cases.iter().map(|c| c.id.as_str()).collect();
    let mut rows = BTreeMap::new();
    for line in text
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        let (id, reason) = line
            .split_once('\t')
            .ok_or("corpus exclusion needs case<TAB>reason")?;
        if !ids.contains(id)
            || reason.trim().is_empty()
            || rows.insert(id.into(), reason.into()).is_some()
        {
            return Err(format!(
                "invalid, duplicate or stale corpus exclusion: {line}"
            ));
        }
    }
    Ok(rows)
}

/// A pending list's rows: each names a case of `cases` and a configuration.
pub fn pending_rows(root: &Path, list: &str, cases: &[Case]) -> Result<Exclusions, String> {
    let rows = super::parse_exclusions(
        &std::fs::read_to_string(root.join(list)).map_err(|e| format!("{list}: {e}"))?,
    )?;
    let ids: BTreeSet<_> = cases.iter().map(|c| c.id.as_str()).collect();
    for ((id, config), row) in &rows {
        if !ids.contains(id.as_str())
            || row.reason.trim().is_empty()
            || row.class.trim().is_empty()
            || (config != super::SNAPSHOT && super::artifact::configuration(config).is_none())
        {
            return Err(format!("{list}: invalid or stale row: {id} under {config}"));
        }
    }
    Ok(rows)
}

/// Cranelift always; LLVM O0 for the rotation's 1/16 slice and for every
/// case with a pending row, so each row is judged on every run.
pub fn matrix(id: &str, rotation: u64, pending: &Exclusions) -> Vec<Configuration> {
    let mut both = super::cranelift::matrix(TargetSpec::x86_64_unknown_linux_gnu());
    let listed = pending.keys().any(|(case, _)| case == id);
    if !listed
        && !(Shard {
            index: rotation % 16 + 1,
            count: 16,
        })
        .holds(id)
    {
        both.retain(|c| matches!(c.engine, hirsel_driver::native::Engine::Cranelift { .. }));
    }
    both
}

pub fn run_case(
    case: &Case,
    rotation: u64,
    tools: &Toolchain,
    dir: &Path,
    rules: &crate::Rules,
    pending: &Exclusions,
) -> CaseResult {
    let mut result = case.run(&matrix(&case.id, rotation, pending), tools, dir);
    // A sidecar/header is an additional oracle, never a replacement for native
    // agreement with interpretation. Full observations still compare failure sites.
    if let Some(reference) = &result.reference {
        let header = crate::parse_header(Path::new(&case.id), &case.source, rules).unwrap();
        if !matches!(header.expect, crate::Expect::Test) {
            let end = if let Some(failure) = reference.failures.first() {
                crate::End::Failure {
                    rule: hirsel_diag::RuleId::new(&failure.rule).unwrap(),
                    line: failure.line as usize,
                }
            } else if reference.blocked {
                crate::End::Blocked
            } else {
                reference
                    .exit_code
                    .map_or(crate::End::Other, crate::End::Exit)
            };
            let actual = crate::Actual::Ran {
                end,
                stdout: reference.output.clone(),
            };
            if let Some(path) = &case.path {
                let sidecar = path.with_extension("stdout");
                if sidecar.exists() {
                    match std::fs::read(&sidecar) {
                        Ok(stdout) if stdout == reference.output => (),
                        other => {
                            result.snapshot = Some(format!(
                                "interpreter differs from stdout sidecar {}: {other:?}",
                                sidecar.display()
                            ))
                        }
                    }
                }
            }
            if !header.expect.matches(&actual) {
                result.snapshot = Some(format!(
                    "interpreter differs from corpus expectation: {actual:?}"
                ));
            }
        }
    }
    for (config, _, outcome) in &result.outcomes {
        if let Outcome::Mismatch(actual) = outcome {
            if let Some(reference) = &result.reference {
                let _ = super::artifact::write(
                    &dir.join("mismatch"),
                    &case.id,
                    &case.source,
                    config,
                    reference,
                    actual,
                );
            }
        }
    }
    result
}

pub fn rows(results: &[CaseResult]) -> String {
    let mut rows: Vec<_> = results
        .iter()
        .flat_map(|r| {
            r.exclusions(&[Class::Cranelift, Class::Llvm])
                .into_iter()
                .map(|(c, e)| format!("{}\t{c}\t{}\t{}\n", r.id, e.class, e.reason))
        })
        .collect();
    rows.sort();
    rows.concat()
}
