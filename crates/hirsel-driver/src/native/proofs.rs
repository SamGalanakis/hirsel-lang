//! `--proofs`: PS-12 sidecar evidence licenses PS-13 check removal in native
//! builds. The obligation table is this compiler's own export of the entry's
//! closure; `hirsel_proof_export::sidecar` decides which records are
//! accepted, this module maps each accepted obligation's TIR sites to their
//! removal form, and the build's `validate_discharge` confirms what import
//! removed. The interpreter never reads a sidecar.
use super::{ReadyProgram, StageName};
use hirsel_engine::{NativeRecordSource, QueryEngine, ToolError, ValidatedNativeInput};
use hirsel_lir::discharge::{Discharge, Discharged, Form};
use hirsel_proof_export::sidecar::{self, Budget, Checker, Record, SIDECAR_FORMAT, Verdict};
use hirsel_proof_export::{DefinitionSection, Obligation, ObligationKind, SitePosition};
use hirsel_syntax::{BinaryOp, ContentHash, DefPath, UnaryOp};
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
        Self::assess_export(&export, validated, evidence)
    }

    /// Assess the exact export supplied by an admitted source producer. The
    /// ordinary sidecar checker and exact native-TIR equality remain authority.
    pub fn assess_export(
        export: &hirsel_proof_export::Export,
        validated: &ValidatedNativeInput,
        evidence: &Evidence,
    ) -> Result<Self, super::NativeError> {
        Self::assess_selected(export, evidence, |path, definition| {
            if validated
                .snapshot()
                .definitions
                .get(path)
                .is_some_and(|selected| **selected != *definition)
            {
                return Err(super::NativeError::new(
                    StageName::Import,
                    format!("proofs: the native snapshot's TIR of {path} is not the exported TIR"),
                ));
            }
            Ok(())
        })
    }

    /// Assess an independently selected complete PS9 export against settled
    /// original Lower products. This miss operation needs no NativeInput,
    /// native Value, or mutable compiler. It does not establish the export's
    /// source closure or authorize reuse of another export's assessment.
    pub fn assess_original(
        export: &hirsel_proof_export::Export,
        original: &hirsel_engine::NativeOriginalReader,
        evidence: &Evidence,
    ) -> Result<Self, super::NativeError> {
        if !std::sync::Arc::ptr_eq(&export.types, original.record_context().record_types()) {
            return Err(super::NativeError::new(
                StageName::Import,
                "proofs: exported and original Lower type universes differ",
            ));
        }
        let mut checked = std::collections::BTreeSet::new();
        Self::assess_selected(export, evidence, |path, definition| {
            if checked.contains(path) {
                return Ok(());
            }
            let selected = original.read(path).ok_or_else(|| {
                super::NativeError::new(
                    StageName::Import,
                    format!("proofs: missing independently selected original Lower {path}"),
                )
            })?;
            if **selected.definition() != *definition {
                return Err(super::NativeError::new(
                    StageName::Import,
                    format!("proofs: the original Lower TIR of {path} is not the exported TIR"),
                ));
            }
            checked.insert(path.clone());
            Ok(())
        })
    }

    /// Assess the complete PS9 export selected by this exact completed native
    /// source root. The source producer supplies both the package export and
    /// original Lower products; no NativeInput or native Value is reconstructed.
    pub fn assess_native(
        export: &hirsel_engine::FrontProofExport,
        native: &std::sync::Arc<hirsel_engine::FrontNativeQueries>,
        completed: hirsel_engine::runtime::ProductRef,
        evidence: &Evidence,
        store: std::sync::Arc<dyn hirsel_engine::store::ResultStore>,
    ) -> Result<Self, super::NativeError> {
        let error = |message| super::NativeError::new(StageName::Import, message);
        let input = export
            .input_for_native(native, completed.clone())
            .map_err(|e| error(format!("proofs: native source export unavailable: {e:?}")))?;
        let source = native
            .outcome(completed)
            .ok_or_else(|| error("proofs: native source root has not completed".to_string()))?;
        let original = source
            .products
            .original_context
            .reader(source.products.originals.iter().map(|p| (**p).clone()))
            .map_err(|e| {
                error(format!(
                    "proofs: original Lower selection unavailable: {e:?}"
                ))
            })?;
        Self::assess_original(&hirsel_proof_export::build(&input), &original, evidence)
    }

    fn assess_selected(
        export: &hirsel_proof_export::Export,
        evidence: &Evidence,
        mut selected: impl FnMut(&DefPath, &hirsel_tir::Definition) -> Result<(), super::NativeError>,
    ) -> Result<Self, super::NativeError> {
        let error = |message: String| super::NativeError::new(StageName::Import, message);
        let assessment = sidecar::assess(export, &evidence.records, &evidence.trusted)
            .map_err(|d| error(format!("proofs: two records share the key of {}", d.0)))?;
        let mut discharge = Discharge::default();
        let mut lines = vec![];
        let mut missing = 0;
        let mut projections = std::collections::BTreeMap::new();
        for assessed in &assessment.obligations {
            let id = assessed.obligation.id.to_string();
            let path = &assessed.section.path;
            let licensed = match &assessed.status {
                Err(sidecar::Keep::Missing) => {
                    missing += 1;
                    continue;
                }
                Err(keep) => Err(keep.to_string()),
                Ok(_) => {
                    let places = projections.entry(path.clone()).or_insert_with(|| {
                        hirsel_proof_export::projected_places(&export.types, &assessed.section.tir)
                    });
                    forms(
                        &export.types,
                        assessed.section,
                        assessed.obligation,
                        places.get(&assessed.obligation.id).copied(),
                    )
                }
            };
            if let Ok(sites) = &licensed {
                selected(path, &assessed.section.tir)?;
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
        self.report_discharged(&ready.discharged)
    }

    /// Report against settled native discharge facts without a Ready body or
    /// a mutable compiler controller at executable readiness.
    pub fn report_discharged(&self, discharged: &Discharge) -> Vec<String> {
        let mut out = vec![];
        let (mut removed, mut kept) = (0, 0);
        for line in &self.lines {
            let outcome = match &line.licensed {
                Ok(()) if !discharged.0.contains_key(&line.definition) => {
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
/// is removable on checked integer arithmetic (wrapping is then exact). `requires` is
/// an assumption whose entry check every caller's evidence would have to
/// cover, and the remaining kinds and operations have no removal form yet.
fn forms(
    types: &hirsel_types::TypeInterner,
    section: &DefinitionSection,
    obligation: &Obligation,
    projection: Option<(hirsel_tir::BodyId, hirsel_tir::PlaceId)>,
) -> Result<Vec<Discharged>, String> {
    use hirsel_tir::{InstructionKind, Rvalue};
    let kind = obligation.id.kind;
    let sem1 = obligation.id.rule.as_str() == "SEM-1";
    if kind == ObligationKind::FailureEdge
        && let Some((body_id, place)) = projection
    {
        let body = &section.tir.bodies[body_id.0 as usize];
        return Ok(obligation
            .sites
            .iter()
            .map(|site| Discharged {
                body: site.body.0,
                block: site.block.0,
                statement: match site.position {
                    SitePosition::Instruction(i) => i,
                    SitePosition::Terminator => {
                        body.blocks[site.block.0 as usize].instructions.len() as u32
                    }
                },
                form: Form::Projection {
                    place: place.0,
                    rule: obligation.id.rule.to_string(),
                },
            })
            .collect());
    }
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
                            op:
                                BinaryOp::Add
                                | BinaryOp::Sub
                                | BinaryOp::Mul
                                | BinaryOp::Div
                                | BinaryOp::Rem
                                | BinaryOp::Shl
                                | BinaryOp::Shr,
                            ..
                        } => Form::Integer,
                        Rvalue::Unary {
                            op: UnaryOp::Neg,
                            value,
                        } => {
                            let ty = body.places[value.place.0 as usize].ty;
                            if matches!(
                                types.get(ty).map(|t| &t.kind),
                                Some(hirsel_types::TypeKind::Primitive(
                                    hirsel_types::Primitive::I8
                                        | hirsel_types::Primitive::I16
                                        | hirsel_types::Primitive::I32
                                        | hirsel_types::Primitive::I64
                                        | hirsel_types::Primitive::I128
                                ))
                            ) {
                                Form::Negation
                            } else {
                                Form::Integer
                            }
                        }
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
