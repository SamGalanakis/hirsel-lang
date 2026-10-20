//! Mismatch artifacts: what a disagreement needs to be replayed
//! and reduced. An artifact is a directory holding `source.hir` and
//! `case.txt` (the case, configuration, matrix and observation versions, the
//! input digest, and both observations as text). Neither holds a path, an
//! arena index or an address, so it replays from any directory and in an
//! engine whose arenas are numbered differently.
use hirsel_driver::native::{
    Configuration, MATRIX_VERSION, OBSERVATION_SCHEMA, Observation, Outcome, Toolchain,
    full_matrix, topology_matrix,
};
use hirsel_run_config::hex;
use std::path::Path;

/// What two engines disagreed about, independent of line shifts: which parts
/// differ, and the rule and definition of each failure on both sides. A
/// reduction must keep this, not merely keep some disagreement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Discrepancy {
    pub exit: Option<(Option<i32>, Option<i32>)>,
    pub output_differs: bool,
    pub expected_failures: Vec<(String, String)>,
    pub actual_failures: Vec<(String, String)>,
    /// Per failure in order: whether line and column agree.
    pub sites_agree: Vec<(bool, bool)>,
}

impl Discrepancy {
    pub fn between(expected: &Observation, actual: &Observation) -> Option<Self> {
        if expected == actual {
            return None;
        }
        let rules = |o: &Observation| {
            o.failures
                .iter()
                .map(|f| (f.rule.clone(), f.definition.0.name.clone()))
                .collect()
        };
        Some(Self {
            exit: (expected.exit_code != actual.exit_code)
                .then_some((expected.exit_code, actual.exit_code)),
            output_differs: expected.output != actual.output,
            expected_failures: rules(expected),
            actual_failures: rules(actual),
            sites_agree: expected
                .failures
                .iter()
                .zip(&actual.failures)
                .map(|(e, a)| (e.line == a.line, e.column == a.column))
                .collect(),
        })
    }
}

/// An engine pair under test: the expected and actual observation of a source,
/// or None when either engine produced none.
pub trait Probe {
    fn observe(&self, source: &str) -> Option<(Observation, Observation)>;
}

/// The reference observation of `source`, in an engine that first checked
/// `warm` when one is given. None when the source does not check or run.
pub fn reference(source: &str, warm: Option<&str>) -> Option<Observation> {
    let mut engine = hirsel_engine::QueryEngine::new(
        hirsel_engine::default_environment(),
        hirsel_driver::compiler_adapters(),
    );
    if let Some(warm) = warm {
        let file = engine.sources.add("warm.hir", warm.into())?;
        let _ = hirsel_engine::check_progress(&mut engine, file, &hirsel_engine::default_context());
    }
    super::checked(&mut engine, "<reference>", source, None)
        .ok()
        .map(|c| c.reference)
}

/// The reference interpreter against one native configuration.
pub struct NativeProbe<'a> {
    pub configuration: Configuration,
    pub toolchain: &'a Toolchain,
    pub dir: &'a Path,
    /// A source checked first in the same engine, so the case's files, types
    /// and bodies are numbered differently than in a fresh engine.
    pub warm: Option<&'a str>,
    pub source_path: Option<std::path::PathBuf>,
}

impl Probe for NativeProbe<'_> {
    fn observe(&self, source: &str) -> Option<(Observation, Observation)> {
        let mut engine = hirsel_engine::QueryEngine::new(
            hirsel_engine::default_environment(),
            hirsel_driver::compiler_adapters(),
        );
        if let Some(warm) = self.warm {
            let file = engine.sources.add("warm.hir", warm.into())?;
            let _ =
                hirsel_engine::check_progress(&mut engine, file, &hirsel_engine::default_context());
        }
        let super::Prepared {
            input: validated,
            entry,
            reference,
            ..
        } = super::checked_with_config(
            &mut engine,
            "<artifact>",
            source,
            self.source_path.as_deref(),
            self.configuration.options.launch.clone(),
            false,
        )
        .ok()?;
        let outcomes = hirsel_driver::native::differential(
            &validated,
            &entry,
            &reference,
            std::slice::from_ref(&self.configuration),
            self.toolchain,
            self.dir,
        );
        match outcomes.into_iter().next()?.1 {
            Outcome::Agree => Some((reference.clone(), reference)),
            Outcome::Mismatch(actual) => Some((reference, actual)),
            Outcome::Unavailable(_) => None,
        }
    }
}

/// The configuration a stored name denotes, in this matrix version.
pub fn configuration(name: &str) -> Option<Configuration> {
    let mut base = name;
    let mut launch = hirsel_driver::native::LaunchInputs::default();
    while let Some((prefix, field)) = base.rsplit_once('/') {
        if let Some(value) = field.strip_prefix("hash-") {
            launch.hash_seed = value.parse().ok()?;
        } else if let Some(value) = field.strip_prefix("random-") {
            launch.random_seed = value.parse().ok()?;
        } else if let Some(value) = field.strip_prefix("test-") {
            launch.test_seed = value.parse().ok()?;
        } else {
            break;
        }
        base = prefix;
    }
    let mut config = full_matrix()
        .into_iter()
        .chain(topology_matrix(20))
        .chain(super::cranelift::matrix(
            hirsel_driver::native::TargetSpec::x86_64_unknown_linux_gnu(),
        ))
        .chain(super::cranelift::matrix(
            hirsel_driver::native::TargetSpec::aarch64_unknown_linux_gnu(),
        ))
        .find(|c| c.name() == base)?;
    config.options.launch = launch;
    // Stored names must be canonical: no duplicate, reordered or zero suffixes.
    (config.name() == name).then_some(config)
}

/// Write the artifact of one mismatch.
pub fn write(
    dir: &Path,
    case: &str,
    source: &str,
    configuration: &str,
    expected: &Observation,
    actual: &Observation,
) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let write = |name: &str, text: &str| {
        std::fs::write(dir.join(name), text).map_err(|e| format!("{name}: {e}"))
    };
    write("source.hir", source)?;
    write(
        "case.txt",
        &format!(
            "case {case}\nconfiguration {configuration}\nmatrix {MATRIX_VERSION}\n\
             observation-schema {OBSERVATION_SCHEMA}\nllvm {}\nsource-digest {}\n\
             --- expected\n{}--- actual\n{}",
            hirsel_driver::native::llvm_version(),
            hex(&hirsel_syntax::ContentHash::of_bytes(source.as_bytes()).0),
            expected.render(),
            actual.render()
        ),
    )
}

/// A stored artifact.
pub struct Artifact {
    pub source: String,
    pub configuration: String,
    pub expected: String,
    pub actual: String,
}

pub fn read(dir: &Path) -> Result<Artifact, String> {
    let read = |name: &str| {
        std::fs::read_to_string(dir.join(name))
            .map_err(|e| format!("{}/{name}: {e}", dir.display()))
    };
    let case = read("case.txt")?;
    let field = |key: &str| {
        case.lines()
            .find_map(|l| l.strip_prefix(key))
            .map(str::to_string)
            .ok_or_else(|| format!("case.txt has no `{}` line", key.trim()))
    };
    for (key, current) in [
        ("matrix ", MATRIX_VERSION),
        ("observation-schema ", OBSERVATION_SCHEMA),
    ] {
        if field(key)? != current.to_string() {
            return Err(format!(
                "the artifact was recorded under another `{}` version",
                key.trim()
            ));
        }
    }
    let (_, rest) = case
        .split_once("--- expected\n")
        .ok_or("case.txt has no expected observation")?;
    let (expected, actual) = rest
        .split_once("--- actual\n")
        .ok_or("case.txt has no actual observation")?;
    Ok(Artifact {
        source: read("source.hir")?,
        configuration: field("configuration ")?,
        expected: expected.to_string(),
        actual: actual.to_string(),
    })
}

/// Replay a stored artifact with `probe`: both engines must observe exactly
/// what the artifact recorded.
pub fn replay(artifact: &Artifact, probe: &dyn Probe) -> Result<(), String> {
    let (expected, actual) = probe
        .observe(&artifact.source)
        .ok_or("an engine produced no observation on replay")?;
    if expected.render() != artifact.expected {
        return Err(format!(
            "the expected observation changed:\n{}recorded:\n{}",
            expected.render(),
            artifact.expected
        ));
    }
    if actual.render() != artifact.actual {
        return Err(format!(
            "the actual observation changed:\n{}recorded:\n{}",
            actual.render(),
            artifact.actual
        ));
    }
    Ok(())
}

/// Reduce `source` by deleting whole lines while `probe` reports the same
/// discrepancy: delta debugging over halves down to single lines, repeated
/// until no deletion keeps it.
pub fn reduce(source: &str, probe: &dyn Probe) -> Result<String, String> {
    let discrepancy = |text: &str| {
        probe
            .observe(text)
            .and_then(|(expected, actual)| Discrepancy::between(&expected, &actual))
    };
    let wanted = discrepancy(source).ok_or("the source shows no discrepancy to preserve")?;
    let mut lines: Vec<&str> = source.lines().collect();
    let mut chunk = lines.len().div_ceil(2).max(1);
    loop {
        let mut removed = false;
        let mut start = 0;
        while start < lines.len() {
            let end = (start + chunk).min(lines.len());
            let mut candidate = lines[..start].to_vec();
            candidate.extend_from_slice(&lines[end..]);
            let text = candidate.join("\n") + "\n";
            if !candidate.is_empty() && discrepancy(&text).as_ref() == Some(&wanted) {
                lines = candidate;
                removed = true;
            } else {
                start = end;
            }
        }
        if chunk == 1 && !removed {
            // Lines that only go together (a function's head, body and
            // brace): try each window of two to four lines once more.
            let mut together = false;
            for width in 2..=4 {
                let mut start = 0;
                while start + width <= lines.len() {
                    let mut candidate = lines[..start].to_vec();
                    candidate.extend_from_slice(&lines[start + width..]);
                    let text = candidate.join("\n") + "\n";
                    if !candidate.is_empty() && discrepancy(&text).as_ref() == Some(&wanted) {
                        lines = candidate;
                        together = true;
                    } else {
                        start += 1;
                    }
                }
            }
            if !together {
                break;
            }
        }
        if !removed {
            chunk = chunk.div_ceil(2);
        }
    }
    Ok(lines.join("\n") + "\n")
}
