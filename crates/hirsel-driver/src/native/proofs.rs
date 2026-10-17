//! `--proofs`: PS-12 sidecar evidence licenses PS-13 check removal in native
//! builds. The obligation table is this compiler's own export of the entry's
//! closure; `hirsel_proof_export::sidecar` decides which records are
//! accepted, this module maps each accepted obligation's TIR sites to their
//! removal form, and the build's `validate_discharge` confirms what import
//! removed. The interpreter never reads a sidecar.
use super::{ReadyProgram, StageName};
use hirsel_engine::{QueryEngine, ToolError, ValidatedNativeInput};
use hirsel_lir::discharge::{Discharge, Discharged, Form};
use hirsel_proof_export::sidecar::{self, Budget, Checker, Record, SIDECAR_FORMAT, Verdict};
use hirsel_proof_export::{DefinitionSection, Obligation, ObligationKind, SitePosition};
use hirsel_syntax::{BinaryOp, ContentHash, DefPath};
use serde_json::Value;
use std::path::Path;

/// A sidecar and the checkers this build trusts.
#[derive(Clone, Debug, Default)]
pub struct Evidence {
    pub records: Vec<Record>,
    pub trusted: Vec<Checker>,
}

impl Evidence {
    /// Read a `hirsel.proof.sidecar.v1` file.
    pub fn read(path: &Path, trusted: Vec<Checker>) -> Result<Self, ToolError> {
        let text = std::fs::read_to_string(path).map_err(ToolError::Io)?;
        let records = parse(&text)
            .map_err(|e| ToolError::Runtime(format!("proofs {}: {e}", path.display())))?;
        Ok(Self { records, trusted })
    }
}

/// The sidecar's JSON: `{"format": SIDECAR_FORMAT, "records": [{"revision",
/// "obligation", "checksum", "checker", "budget": {"unit", "count"},
/// "verdict"}]}`; hashes are spelled as in the export (`#` and 64 hex
/// digits), the checker `tool@version`. Unknown or missing fields refuse the
/// file.
pub fn parse(text: &str) -> Result<Vec<Record>, String> {
    let root: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let root = object(&root, "the sidecar", &["format", "records"])?;
    if root["format"].as_str() != Some(SIDECAR_FORMAT) {
        return Err(format!("format is not {SIDECAR_FORMAT:?}"));
    }
    let records = root["records"]
        .as_array()
        .ok_or("records is not an array")?;
    records
        .iter()
        .enumerate()
        .map(|(i, record)| {
            let at = format!("records[{i}]");
            let r = object(
                record,
                &at,
                &[
                    "revision",
                    "obligation",
                    "checksum",
                    "checker",
                    "budget",
                    "verdict",
                ],
            )?;
            let text = |field: &str| {
                r[field]
                    .as_str()
                    .ok_or_else(|| format!("{at}.{field} is not a string"))
            };
            let budget = object(&r["budget"], &format!("{at}.budget"), &["unit", "count"])?;
            let verdict = text("verdict")?;
            Ok(Record {
                revision: hash(text("revision")?).ok_or(format!("{at}.revision is not a hash"))?,
                obligation: text("obligation")?.to_owned(),
                checksum: hash(text("checksum")?).ok_or(format!("{at}.checksum is not a hash"))?,
                checker: Checker::parse(text("checker")?)
                    .ok_or(format!("{at}.checker is not tool@version"))?,
                budget: Budget {
                    unit: budget["unit"]
                        .as_str()
                        .ok_or(format!("{at}.budget.unit is not a string"))?
                        .to_owned(),
                    count: budget["count"]
                        .as_u64()
                        .ok_or(format!("{at}.budget.count is not an unsigned integer"))?,
                },
                verdict: Verdict::ALL
                    .into_iter()
                    .find(|v| v.as_str() == verdict)
                    .ok_or(format!("{at}.verdict {verdict:?} is not a verdict"))?,
            })
        })
        .collect()
}

fn object<'v>(
    value: &'v Value,
    at: &str,
    fields: &[&str],
) -> Result<&'v serde_json::Map<String, Value>, String> {
    let map = value.as_object().ok_or(format!("{at} is not an object"))?;
    if let Some(field) = map.keys().find(|k| !fields.contains(&k.as_str())) {
        return Err(format!("{at} has an unknown field {field:?}"));
    }
    if let Some(field) = fields.iter().find(|f| !map.contains_key(**f)) {
        return Err(format!("{at} lacks {field:?}"));
    }
    Ok(map)
}

/// `#` and 64 lowercase hex digits, the export's spelling.
fn hash(text: &str) -> Option<ContentHash> {
    let hex = text.strip_prefix('#')?;
    if hex.len() != 64 || hex.bytes().any(|b| !matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    let mut out = [0; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(ContentHash(out))
}

/// What one obligation's evidence came to before the build.
struct Line {
    id: String,
    definition: DefPath,
    /// The removal its accepted evidence licenses, or why its check stays.
    licensed: Result<(), String>,
}

/// The plan of one build and the facts its report reads.
pub struct Proofs {
    pub discharge: Discharge,
    lines: Vec<Line>,
    orphans: Vec<String>,
    missing: usize,
}

impl Proofs {
    /// Assess `evidence` against this compiler's export of `entry`'s closure.
    /// The native snapshot must hold the very TIR the export numbered.
    pub fn assess(
        engine: &mut QueryEngine,
        entry: &DefPath,
        validated: &ValidatedNativeInput,
        evidence: &Evidence,
    ) -> Result<Self, super::NativeError> {
        let error = |message: String| super::NativeError::new(StageName::Import, message);
        let input = engine
            .proof_export_input(entry)
            .map_err(|e| error(format!("proof export: {e:?}")))?;
        let export = hirsel_proof_export::build(&input);
        let assessment = sidecar::assess(&export, &evidence.records, &evidence.trusted)
            .map_err(|d| error(format!("proofs: two records share the key of {}", d.0)))?;
        let definitions = &validated.snapshot().definitions;
        let mut discharge = Discharge::default();
        let mut lines = vec![];
        let mut missing = 0;
        for assessed in &assessment.obligations {
            let id = assessed.obligation.id.to_string();
            let path = &assessed.section.path;
            let licensed = match &assessed.status {
                Err(sidecar::Keep::Missing) => {
                    missing += 1;
                    continue;
                }
                Err(keep) => Err(keep.to_string()),
                Ok(_) => forms(assessed.section, assessed.obligation),
            };
            if let Ok(sites) = &licensed {
                if definitions
                    .get(path)
                    .is_some_and(|d| **d != assessed.section.tir)
                {
                    return Err(error(format!(
                        "proofs: the native snapshot's TIR of {path} is not the exported TIR"
                    )));
                }
                discharge
                    .0
                    .entry(path.clone())
                    .or_default()
                    .extend(sites.iter().cloned());
            }
            lines.push(Line {
                id,
                definition: path.clone(),
                licensed: licensed.map(|_| ()),
            });
        }
        Ok(Self {
            discharge,
            lines,
            orphans: assessment
                .orphans
                .iter()
                .map(|r| r.obligation.clone())
                .collect(),
            missing,
        })
    }

    /// One line per record and per accepted obligation, then the counts.
    pub fn report(&self, ready: &ReadyProgram) -> Vec<String> {
        let mut out = vec![];
        let (mut removed, mut kept) = (0, 0);
        for line in &self.lines {
            let outcome = match &line.licensed {
                Ok(()) if ready.undischarged.contains(&line.definition) => Err(
                    "append fusion rewrote the definition before import, so its sites no longer number its TIR"
                        .to_string(),
                ),
                Ok(()) if !ready.discharged.0.contains_key(&line.definition) => {
                    Ok("removed (the definition has no native code)")
                }
                Ok(()) => Ok("removed"),
                Err(why) => Err(why.clone()),
            };
            match outcome {
                Ok(text) => {
                    removed += 1;
                    out.push(format!("proof {}: {text}", line.id));
                }
                Err(why) => {
                    kept += 1;
                    out.push(format!("proof {}: kept ({why})", line.id));
                }
            }
        }
        for id in &self.orphans {
            out.push(format!(
                "proof {id}: ignored (no obligation of this program has this id)"
            ));
        }
        out.push(format!(
            "proofs: {removed} removed, {kept} kept, {} without evidence",
            self.missing
        ));
        out
    }
}

/// The removal form of each site of an accepted obligation, or why it has
/// none. A `check` or `ensures` site is a TIR `Check`; a SEM-1 failure edge
/// is removable on an integer `Add` (wrapping is then exact). `requires` is
/// an assumption whose entry check every caller's evidence would have to
/// cover, and the remaining kinds and operations have no removal form yet.
fn forms(section: &DefinitionSection, obligation: &Obligation) -> Result<Vec<Discharged>, String> {
    use hirsel_tir::{InstructionKind, Rvalue};
    let kind = obligation.id.kind;
    let sem1 = obligation.id.rule.as_str() == "SEM-1";
    if !matches!(kind, ObligationKind::Check | ObligationKind::Ensures)
        && !(kind == ObligationKind::FailureEdge && sem1)
    {
        return Err(format!(
            "no removal form for a {} {} obligation",
            obligation.id.rule,
            kind.as_str()
        ));
    }
    obligation
        .sites
        .iter()
        .map(|site| {
            let body = &section.tir.bodies[site.body.0 as usize];
            let SitePosition::Instruction(i) = site.position else {
                return Err("no removal form for a terminator site".to_string());
            };
            let instruction = &body.blocks[site.block.0 as usize].instructions[i as usize].kind;
            let form = match instruction {
                InstructionKind::Check { rule, .. } if !sem1 => Form::Check(rule.to_string()),
                InstructionKind::Assign { value, .. } if sem1 => {
                    match &body.rvalues[value.0 as usize] {
                        Rvalue::Binary {
                            op: BinaryOp::Add, ..
                        } => Form::Overflow,
                        Rvalue::Binary { op, .. } => {
                            return Err(format!("no removal form for SEM-1 {op:?}"));
                        }
                        _ => return Err("no removal form for this SEM-1 site".to_string()),
                    }
                }
                _ => return Err("the site is not a check".to_string()),
            };
            Ok(Discharged {
                body: site.body.0,
                block: site.block.0,
                statement: i,
                form,
            })
        })
        .collect()
}
