//! The proof seam: every accepted conformance program
//! exports, imports under V and re-exports byte-identically; corrupted and
//! forged exports are refused with a located error; digests, checksums and
//! obligation ids are stable exactly as PS-4..PS-6 promise.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_proof_export::{DefinitionSection, Detail, Export, ImportError, ObligationKind};
use std::path::{Path, PathBuf};

fn named(name: &str) -> DefPath {
    let mut path = entry_path(&default_context());
    path.0.name = name.into();
    path
}

fn engine(source: &str) -> QueryEngine {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine.sources.add("proof.hir", source.into()).unwrap();
    let result = check_progress(&mut engine, file, &default_context())
        .unwrap_or_else(|e| panic!("{source}\n{e:?}"));
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    engine
}

/// Export `source`, import it (which re-validates with V) and return both.
fn exported(source: &str) -> (String, Export) {
    let mut engine = engine(source);
    let text = engine
        .export_tir(&entry_path(&default_context()))
        .unwrap_or_else(|e| panic!("{e:?}"));
    let export = engine
        .import_tir(&text)
        .unwrap_or_else(|e| panic!("{e}\n{text}"));
    (text, export)
}

fn section<'a>(export: &'a Export, name: &str) -> &'a DefinitionSection {
    export
        .definition(&named(name))
        .unwrap_or_else(|| panic!("no section for {name}"))
}

fn import(text: &str) -> Result<Export, ImportError> {
    engine("pub fn main() -> Result[(), ()] { Ok(()) }\n").import_tir(text)
}

/// One line per obligation: id, first site, detail.
fn table(export: &Export, name: &str) -> String {
    let section = section(export, name);
    let mut text = String::new();
    for obligation in &section.obligations {
        let site = obligation
            .sites
            .first()
            .and_then(|s| section.locations.get(s.span.start as usize))
            .map_or("-".to_string(), |l| format!("{}:{}", l.line, l.column));
        let detail = match &obligation.detail {
            Detail::Site => String::new(),
            Detail::Call { callee, clause } => format!(
                " call={} clause={clause}",
                callee
                    .as_ref()
                    .map_or("indirect".into(), ToString::to_string)
            ),
            Detail::Clause { clause, .. } => format!(" clause={clause}"),
        };
        text.push_str(&format!(
            "{} @{} sites={}{detail}\n",
            obligation.id,
            site,
            obligation.sites.len()
        ));
    }
    text
}

fn expect_file(stored: &str, actual: &str, name: &str) {
    assert!(
        stored == actual,
        "tests/fixtures/proof_seam/{name} differs; actual:\n{actual}"
    );
}

const ADD: &str = "fn add(x: u64, y: u64) -> u64\n  requires x < 1000u64\n  ensures result >= y {\n  x + y\n}\npub fn main() -> Result[(), ()] {\n  let n = add(41u64, 1u64)\n  Ok(())\n}\n";

#[test]
fn a_contracted_program_round_trips_with_its_obligation_table() {
    with_compiler_stack(|| {
        let (text, export) = exported(ADD);
        assert_eq!(hirsel_proof_export::export(&export.to_input()), text);
        assert!(export.skipped.is_empty(), "{:?}", export.skipped);
        assert!(text.starts_with("(Export\n  (Header \"hirsel.tir.export.v9\" 9"));
        expect_file(
            include_str!("../fixtures/proof_seam/add.txt"),
            &(table(&export, "add") + &table(&export, "main")),
            "add.txt",
        );
    });
}

#[test]
fn two_exports_of_one_program_are_identical() {
    with_compiler_stack(|| {
        let (first, _) = exported(ADD);
        let (second, _) = exported(ADD);
        assert_eq!(first, second);
    });
}

#[test]
fn a_flipped_byte_in_a_definition_is_refused_at_that_definition() {
    with_compiler_stack(|| {
        let (text, _) = exported(ADD);
        assert_eq!(text.matches("(Integer 41)").count(), 1);
        let flipped = text.replacen("(Integer 41)", "(Integer 42)", 1);
        let error = import(&flipped).unwrap_err();
        assert_eq!(error.at, "standalone@0/main.tir_digest", "{error}");
    });
}

#[test]
fn a_flipped_delimiter_is_refused_at_its_line_and_column() {
    with_compiler_stack(|| {
        let (text, _) = exported(ADD);
        let offset = text.find("(DefinitionSection").unwrap();
        let mut flipped = text.clone().into_bytes();
        flipped[offset] = b'[';
        let flipped = String::from_utf8(flipped).unwrap();
        let line = text[..offset].matches('\n').count() + 1;
        let error = import(&flipped).unwrap_err();
        assert_eq!(error.at, format!("{line}:5"), "{error}");
        assert_eq!(error.message, "expected '('");
    });
}

#[test]
fn a_forged_definition_with_recomputed_digests_is_refused_by_v() {
    with_compiler_stack(|| {
        let (_, export) = exported(ADD);
        let mut input = export.to_input();
        let main = named("main");
        let mut forged = (*input.definitions[&main]).clone();
        let body = &mut forged.bodies[forged.entry.0 as usize];
        let returned = body
            .blocks
            .iter_mut()
            .find_map(|b| match &mut b.terminator {
                hirsel_tir::Terminator::Return { value, .. } => Some(value),
                _ => None,
            })
            .expect("a return");
        // A returned place outside the body's place arena.
        returned.place = hirsel_tir::PlaceId(u32::MAX);
        input.definitions.insert(main, std::sync::Arc::new(forged));
        // The forger recomputes every digest and checksum; only V can refuse.
        let text = hirsel_proof_export::export(&input);
        let error = import(&text).unwrap_err();
        assert_eq!(error.at, "standalone@0/main", "{error}");
        assert!(error.message.starts_with("V rejects"), "{error}");
    });
}

#[test]
fn a_version_mismatch_is_refused_at_the_header() {
    with_compiler_stack(|| {
        let (text, _) = exported(ADD);
        let other = text.replacen(
            "(Header \"hirsel.tir.export.v9\" 9",
            "(Header \"hirsel.tir.export.v9\" 10",
            1,
        );
        assert_eq!(import(&other).unwrap_err().at, "header.tir_version");
        let other = text.replacen("hirsel.tir.export.v9", "hirsel.tir.export.v0", 1);
        assert_eq!(import(&other).unwrap_err().at, "header.format");
        let engine = engine(ADD);
        let mut expected = engine.proof_environment();
        expected.compiler_version.push_str("-other");
        let error =
            hirsel_proof_export::import(&text, &expected, &hirsel_types::TypeInterner::new(None))
                .unwrap_err();
        assert_eq!(error.at, "header.environment", "{error}");
    });
}

#[test]
fn trailing_input_and_a_stale_export_digest_are_refused() {
    with_compiler_stack(|| {
        let (text, _) = exported(ADD);
        let error = import(&format!("{text}(Extra)")).unwrap_err();
        assert_eq!(error.message, "trailing input");
        // The program revision feeds no definition, signature or table: only
        // the export digest notices a canonical edit of it.
        let at = text.find("(some #").unwrap() + "(some #".len();
        let mut skipped = text.clone().into_bytes();
        skipped[at] = if skipped[at] == b'0' { b'1' } else { b'0' };
        let skipped = String::from_utf8(skipped).unwrap();
        assert_eq!(import(&skipped).unwrap_err().at, "export digest");
    });
}

/// Every signature use is consistent, every digest is recomputed: only the
/// fingerprint's re-derivation from the signature notices the forgery.
#[test]
fn a_fingerprint_forged_at_every_use_is_refused_by_re_derivation() {
    with_compiler_stack(|| {
        let (_, export) = exported(ADD);
        let add = named("add");
        let forged_hash = hirsel_syntax::ContentHash::of_bytes(b"forged");
        let mut forged = export.clone();
        for signature in &mut forged.signatures {
            if signature.path == add {
                signature.public.fingerprint = forged_hash;
            }
            for dependency in &mut signature.dependencies {
                if dependency.definition.path() == &add {
                    dependency.fingerprint = forged_hash;
                }
            }
        }
        for definition in &mut forged.definitions {
            if definition.path == add {
                definition.tir.signature_hash = forged_hash;
            }
        }
        let text = hirsel_proof_export::export(&forged.to_input());
        let error = import(&text).unwrap_err();
        assert_eq!(error.at, "standalone@0/add.public.fingerprint", "{error}");
    });
}

const SEALED: &str = include_str!("../fixtures/proof-export-sample-12/sealed.hir");

fn signature<'a>(export: &'a Export, name: &str) -> &'a hirsel_proof_export::SignatureSection {
    export
        .signatures
        .iter()
        .find(|s| s.path == named(name))
        .unwrap_or_else(|| panic!("no signature for {name}"))
}

/// A fact helper's fingerprint covers its owner alone, and an owner view keeps
/// its public fingerprint: both re-derive on import.
#[test]
fn a_fact_helper_and_an_owner_view_re_derive() {
    with_compiler_stack(|| {
        let (_, export) = exported(SEALED);
        let helper = signature(&export, "Percent$fact$0");
        let owner = signature(&export, "Percent");
        assert_eq!(
            helper.dependencies,
            [hirsel_types::SignatureDependency {
                definition: hirsel_syntax::DefId::intern(&owner.path),
                fingerprint: owner.public.fingerprint,
            }]
        );
        let meters = signature(&export, "Meters");
        let view = meters.owner_view.as_ref().expect("an owner view");
        assert_eq!(view.fingerprint, meters.public.fingerprint);
        assert_eq!(hirsel_types::encoding::sealed(view), meters.public);
    });
}

/// An owner view is the public signature's entity: one whose public part
/// differs, or that adds nothing, is refused even with the public fingerprint.
#[test]
fn an_owner_view_that_does_not_seal_to_its_public_signature_is_refused() {
    with_compiler_stack(|| {
        let (_, export) = exported(SEALED);
        let refused = |edit: &dyn Fn(&mut hirsel_types::Signature, &hirsel_types::Signature)| {
            let mut forged = export.clone();
            let meters = forged
                .signatures
                .iter_mut()
                .find(|s| s.path == named("Meters"))
                .unwrap();
            let public = meters.public.clone();
            edit(meters.owner_view.as_mut().unwrap(), &public);
            import(&hirsel_proof_export::export(&forged.to_input())).unwrap_err()
        };
        let error = refused(&|view, _| {
            view.metadata.visibility = hirsel_syntax::Visibility::Public;
        });
        assert_eq!(error.at, "standalone@0/Meters.owner_view", "{error}");
        let error = refused(&|view, public| *view = public.clone());
        assert_eq!(error.at, "standalone@0/Meters.owner_view", "{error}");
        let error = refused(&|view, _| {
            let hirsel_types::SignatureKind::Type(declaration) = &mut view.kind else {
                panic!("expected a type");
            };
            declaration.captures = Some(Default::default());
        });
        assert_eq!(error.at, "standalone@0/Meters.owner_view", "{error}");
    });
}

#[test]
fn a_carrier_owner_view_hides_field_ordinals_but_keeps_its_public_marker() {
    with_compiler_stack(|| {
        let (_, mut export) = exported(
            "@captures(inner)\npub opaque type Pipe: () uses {} {\n  tag: u64,\n  inner: Iter[u64],\n}\n",
        );
        let pipe = signature(&export, "Pipe");
        let view = pipe.owner_view.as_ref().expect("an owner view");
        let captures = |signature: &hirsel_types::Signature| {
            let hirsel_types::SignatureKind::Type(declaration) = &signature.kind else {
                panic!("expected a type");
            };
            declaration.captures.clone()
        };
        assert_eq!(captures(view).unwrap().as_ref(), &[1]);
        assert!(captures(&pipe.public).unwrap().is_empty());
        assert_eq!(hirsel_types::encoding::sealed(view), pipe.public);
        // Sealing hides the selection, not whether this is a carrier at all.
        let pipe = export
            .signatures
            .iter_mut()
            .find(|s| s.path == named("Pipe"))
            .unwrap();
        let hirsel_types::SignatureKind::Type(declaration) =
            &mut pipe.owner_view.as_mut().unwrap().kind
        else {
            panic!("expected a type");
        };
        declaration.captures = None;
        let error = import(&hirsel_proof_export::export(&export.to_input())).unwrap_err();
        assert_eq!(error.at, "standalone@0/Pipe.owner_view", "{error}");
    });
}

const CHECKED: &str = "@serializable(check = valid)\npub opaque type Month: plain + inert + noalloc_drop uses {} {\n  number: u8,\n}\n@noalloc\nfn valid(value: Month) -> bool uses {} {\n  value.number >= 1u8 && value.number <= 12u8\n}\nfn month(number: u8) -> Month {\n  Month(number: number)\n}\npub fn main() -> Result[(), ()] {\n  let m = month(1u8)\n  Ok(())\n}\n";

/// A checked serializable opt-in publishes its representation graph (SER-1), so
/// its owner view keeps the graph in its own rows: the view imports, but one whose
/// public part or check differs from the public signature's is refused.
#[test]
fn a_checked_opt_in_owner_view_seals_to_its_published_graph() {
    with_compiler_stack(|| {
        let (_, export) = exported(CHECKED);
        let month = signature(&export, "Month");
        assert!(month.owner_view.is_some(), "an owner view");
        let refused = |edit: &dyn Fn(&mut hirsel_types::Signature)| {
            let mut forged = export.clone();
            let month = forged
                .signatures
                .iter_mut()
                .find(|s| s.path == named("Month"))
                .unwrap();
            edit(month.owner_view.as_mut().unwrap());
            import(&hirsel_proof_export::export(&forged.to_input())).unwrap_err()
        };
        let error = refused(&|view| {
            view.metadata.visibility = hirsel_syntax::Visibility::Public;
        });
        assert_eq!(error.at, "standalone@0/Month.owner_view", "{error}");
        let error = refused(&|view| {
            if let hirsel_types::SignatureKind::Type(t) = &mut view.kind {
                t.serializable_check.as_mut().unwrap().function = named("month");
            }
        });
        assert_eq!(error.at, "standalone@0/Month.owner_view", "{error}");
    });
}

const CALLER: &str = "fn caller(a: u64) -> u64 {\n  let _ = callee(a)\n  a + 1u64\n}\npub fn main() -> Result[(), ()] {\n  let n = caller(1u64)\n  Ok(())\n}\n";

fn with_callee(callee: &str) -> Export {
    exported(&format!("{callee}{CALLER}")).1
}

fn checksums(section: &DefinitionSection) -> Vec<(String, hirsel_syntax::ContentHash)> {
    section
        .obligations
        .iter()
        .map(|o| (o.id.to_string(), o.checksum))
        .collect()
}

#[test]
fn a_callee_body_edit_leaves_the_caller_digest_and_checksums_unchanged() {
    with_compiler_stack(|| {
        let base = with_callee("fn callee(x: u64) -> u64 {\n  x\n}\n");
        // More expressions shift every later file-arena id of the caller.
        let body = with_callee("fn callee(x: u64) -> u64 {\n  let y = x * 2u64\n  y / 2u64\n}\n");
        let (old, new) = (section(&base, "caller"), section(&body, "caller"));
        assert_eq!(old.tir_digest, new.tir_digest);
        assert!(!checksums(old).is_empty());
        assert_eq!(checksums(old), checksums(new));
        assert_ne!(
            section(&base, "callee").tir_digest,
            section(&body, "callee").tir_digest
        );
    });
}

#[test]
fn a_callee_signature_edit_changes_the_caller_checksums() {
    with_compiler_stack(|| {
        let base = with_callee("fn callee(x: u64) -> u64 {\n  x\n}\n");
        let old = section(&base, "caller");
        // A result type the caller's TIR holds: its digest changes too.
        let result = with_callee("fn callee(x: u64) -> u32 {\n  7u32\n}\n");
        let new = section(&result, "caller");
        assert_ne!(old.tir_digest, new.tir_digest);
        for ((id, before), (_, after)) in checksums(old).iter().zip(checksums(new)) {
            assert_ne!(*before, after, "{id}");
        }
        // A contract: the referenced fingerprint moves, and the call gains a
        // requires-at-call obligation.
        let contract = with_callee("fn callee(x: u64) -> u64\n  requires x < 100u64 {\n  x\n}\n");
        let new = section(&contract, "caller");
        let new_checksums = checksums(new);
        for (id, before) in checksums(old) {
            let after = new_checksums.iter().find(|(i, _)| *i == id).expect(&id);
            assert_ne!(before, after.1, "{id}");
        }
        assert!(
            new.obligations
                .iter()
                .any(|o| o.id.kind == ObligationKind::RequiresAtCall)
        );
    });
}

const PICK: &str = "fn pick(xs: List[u64], i: u64, a: u64, b: u64) -> u64 {\n  let s = a + b\n  let t = xs[i]\n  s * t\n}\npub fn main() -> Result[(), ()] {\n  let n = pick([1u64, 2u64], 1u64, 3u64, 4u64)\n  Ok(())\n}\n";

fn ids(export: &Export, name: &str) -> Vec<String> {
    section(export, name)
        .obligations
        .iter()
        .map(|o| o.id.to_string())
        .collect()
}

#[test]
fn an_unrelated_definition_leaves_obligation_ids_unchanged() {
    with_compiler_stack(|| {
        let (_, base) = exported(PICK);
        let (_, added) = exported(&format!(
            "fn unrelated(x: u64) -> u64 {{\n  x - 1u64\n}}\n{PICK}"
        ));
        assert_eq!(ids(&base, "pick"), ids(&added, "pick"));
        assert_eq!(
            section(&base, "pick").tir_digest,
            section(&added, "pick").tir_digest
        );
        expect_file(
            include_str!("../fixtures/proof_seam/pick.txt"),
            &table(&base, "pick"),
            "pick.txt",
        );
    });
}

#[test]
fn an_earlier_check_shifts_only_its_own_rule_and_kind() {
    with_compiler_stack(|| {
        let (_, base) = exported(PICK);
        let earlier = PICK.replacen(
            "  let s = a + b\n",
            "  let z = a + 1u64\n  let s = a + b\n",
            1,
        );
        let (_, edited) = exported(&earlier);
        let table = table(&edited, "pick");
        expect_file(
            include_str!("../fixtures/proof_seam/pick-earlier-check.txt"),
            &table,
            "pick-earlier-check.txt",
        );
        let shifted = |ids: Vec<String>| -> (Vec<String>, Vec<String>) {
            ids.into_iter()
                .partition(|id| id.contains("#SEM-1/failure-edge/"))
        };
        let (old_sem, old_rest) = shifted(ids(&base, "pick"));
        let (new_sem, new_rest) = shifted(ids(&edited, "pick"));
        assert_eq!(old_rest, new_rest);
        assert_eq!(new_sem.len(), old_sem.len() + 1);
    });
}

#[test]
fn a_runtime_failure_maps_to_its_obligation() {
    with_compiler_stack(|| {
        for tracked in [false, true] {
            let source = "fn add(x: u64, y: u64) -> u64 {\n  x + y\n}\npub fn main() -> Result[(), ()] {\n  let n = add(18446744073709551615u64, 1u64)\n  Ok(())\n}\n";
            let source = format!("{}{source}", if tracked { "@track_caller\n" } else { "" });
            let mut engine = engine(&source);
            let entry = entry_path(&default_context());
            let text = engine.export_tir(&entry).unwrap();
            let export = engine.import_tir(&text).unwrap();
            let config = RunConfiguration {
                entry,
                hash_seed: 0,
                inputs: vec![],
            };
            let run = run_observed_snapshot(&mut engine, &config, SimulationConfig::default())
                .unwrap_or_else(|e| panic!("{e:?}"));
            let observation = run.observation.expect("an observation");
            let failure = observation.failures.first().expect("a failure");
            let site = failure.operation.as_ref().unwrap_or(&failure.stable_site);
            assert_eq!(
                failure.stable_site.definition.0.name,
                if tracked { "main" } else { "add" }
            );
            let ids: Vec<String> = export
                .failure_obligations(&site.definition, &failure.rule, site.line, site.column)
                .into_iter()
                .map(|id| id.to_string())
                .collect();
            assert_eq!(ids, ["standalone@0/add#SEM-1/failure-edge/0"]);
        }
    });
}

// --- every accepted conformance program -----------------------------------

fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../conformance")
}

fn accepted(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            accepted(&path, out);
        } else if path.extension().is_some_and(|e| e == "hir") {
            let source = std::fs::read_to_string(&path).unwrap();
            if source.lines().take(8).any(|line| {
                ["accept", "run", "fail"]
                    .iter()
                    .any(|e| line.starts_with(&format!("// expect: {e}")))
            }) {
                out.push(path);
            }
        }
    }
}

/// The signature fingerprints an import re-derived, by derivation.
#[derive(Debug, Default)]
struct Kinds {
    declared: usize,
    fact_helpers: usize,
    owner_views: usize,
    registry_rows: usize,
}
impl Kinds {
    fn of(export: &Export) -> Self {
        let operations: std::collections::BTreeSet<_> = export
            .definitions
            .iter()
            .flat_map(|d| &d.operations)
            .collect();
        let mut kinds = Self::default();
        for signature in &export.signatures {
            if signature.path.0.name.contains("$fact$") {
                kinds.fact_helpers += 1;
            } else if operations.contains(&signature.path) {
                kinds.registry_rows += 1;
            } else {
                kinds.declared += 1;
            }
            kinds.owner_views += usize::from(signature.owner_view.is_some());
        }
        kinds
    }
    fn require_all(&self) -> Result<(), String> {
        for (kind, count) in [
            ("declared signatures", self.declared),
            ("fact helpers", self.fact_helpers),
            ("owner views", self.owner_views),
            ("registry rows", self.registry_rows),
        ] {
            if count == 0 {
                return Err(format!("re-derives no {kind}"));
            }
        }
        Ok(())
    }
    fn add(&mut self, other: &Self) {
        self.declared += other.declared;
        self.fact_helpers += other.fact_helpers;
        self.owner_views += other.owner_views;
        self.registry_rows += other.registry_rows;
    }
}

/// Export, import (re-deriving every fingerprint, then V), re-export
/// byte-identically, and export again from a fresh session with identical
/// bytes. `None` when the program is not currently accepted (the conformance
/// ratchet owns that verdict).
fn round_trip(path: &Path) -> Result<Option<Kinds>, String> {
    let export_once = || -> Result<Option<(String, QueryEngine)>, String> {
        let mut program = load_program(path).map_err(|e| format!("load: {e}"))?;
        let checked = program
            .check(hirsel_engine::LoweringPolicy::Reachable)
            .map_err(|e| format!("check: {e:?}"))?;
        if checked
            .diagnostics
            .iter()
            .any(|d| d.severity == hirsel_diag::Severity::Error)
            || checked.completion != CheckCompletion::Complete
        {
            return Ok(None);
        }
        let entry = program.entry.clone();
        let text = program
            .engine
            .export_tir(&entry)
            .map_err(|e| format!("export: {e:?}"))?;
        Ok(Some((text, program.engine)))
    };
    let Some((text, engine)) = export_once()? else {
        return Ok(None);
    };
    // Import into a fresh interner, with no types warmed by checking/exporting.
    let export = hirsel_proof_export::import(
        &text,
        &engine.proof_environment(),
        &hirsel_types::TypeInterner::new(None),
    )
    .map_err(|e| format!("fresh import: {e}"))?;
    if hirsel_proof_export::export(&export.to_input()) != text {
        return Err("re-export differs".into());
    }
    let Some((again, _)) = export_once()? else {
        return Err("second session rejects".into());
    };
    if again != text {
        return Err("a second export differs".into());
    }
    Ok(Some(Kinds::of(&export)))
}

// Each maintained fixture must independently cover all four derivations.
// These are explicit paths, not discoveries: deletion must remain a failure.
fn required_fixtures() -> Vec<PathBuf> {
    ["tests/fixtures/proof-export-sample-12/sealed.hir"]
        .iter()
        .map(|path| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path))
        .collect()
}

fn required_round_trip(path: &Path) -> Result<Kinds, String> {
    (|| {
        let kinds = round_trip(path)?.ok_or("required fixture is no longer accepted")?;
        kinds.require_all()?;
        Ok(kinds)
    })()
    .map_err(|error: String| format!("{}: {error}", path.display()))
}

fn round_trip_all(select: impl Fn(&Path) -> bool, required: &[PathBuf]) -> Kinds {
    let mut programs = vec![];
    accepted(&corpus(), &mut programs);
    programs.retain(|p| select(p));
    programs.extend_from_slice(required);
    programs.sort();
    programs.dedup();
    let jobs = hirsel_guard::jobs();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results = std::sync::Mutex::new(vec![]);
    std::thread::scope(|scope| {
        for _ in 0..jobs {
            scope.spawn(|| {
                with_compiler_stack(|| {
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(path) = programs.get(i) else { break };
                        let result = std::panic::catch_unwind(|| {
                            if required.contains(path) {
                                required_round_trip(path).map(Some)
                            } else {
                                round_trip(path)
                            }
                        })
                        .unwrap_or_else(|_| Err("panicked".into()));
                        results.lock().unwrap().push((path.clone(), result));
                    }
                })
            });
        }
    });
    let results = results.into_inner().unwrap();
    let failures: Vec<String> = results
        .iter()
        .filter_map(|(path, r)| r.as_ref().err().map(|e| format!("{}: {e}", path.display())))
        .collect();
    let mut kinds = Kinds::default();
    let mut exported = 0;
    for (_, result) in &results {
        if let Ok(Some(program)) = result {
            exported += 1;
            kinds.add(program);
        }
    }
    eprintln!(
        "proof export: {exported} of {} selected accepted programs round-trip, re-deriving {} declared, {} fact-helper, {} owner-view and {} registry-row fingerprints",
        programs.len(),
        kinds.declared,
        kinds.fact_helpers,
        kinds.owner_views,
        kinds.registry_rows,
    );
    assert!(
        failures.is_empty(),
        "{} failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert!(exported > 0);
    kinds
}

/// The per-push sample: about one program in sixty-four, chosen by a seeded hash
/// of its corpus path, so adding a program never reshuffles the others.
fn sampled(path: &Path) -> bool {
    const SEED: &[u8] = b"hirsel.proof-export.sample.v1";
    let relative = path.strip_prefix(corpus()).unwrap_or(path);
    let hash = hirsel_syntax::ContentHash::of_parts(
        "hirsel.test.sample",
        [SEED, relative.to_string_lossy().as_bytes()],
    );
    hash.0[0] % 64 == 0
}

/// The stable seeded 1/64 discovery plus mandatory category fixtures. Corpus
/// acceptance may drift; required fixtures must check and cover every derivation.
#[test]
fn a_sample_of_accepted_programs_round_trips() {
    let kinds = round_trip_all(sampled, &required_fixtures());
    kinds.require_all().expect("sample coverage");
}

#[test]
fn a_required_fixture_round_trips_with_a_fresh_interner() {
    with_compiler_stack(|| {
        for path in required_fixtures() {
            let kinds = required_round_trip(&path).unwrap_or_else(|error| panic!("{error}"));
            eprintln!("required proof fixture {}: {kinds:?}", path.display());
        }
    });
}

#[test]
fn a_deleted_or_rejected_required_fixture_is_refused_with_its_path() {
    with_compiler_stack(|| {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("required.hir");
        std::fs::write(&path, SEALED).unwrap();
        std::fs::remove_file(&path).unwrap();
        let error = required_round_trip(&path).unwrap_err();
        assert!(
            error.starts_with(&format!("{}: load:", path.display())),
            "{error}"
        );
        std::fs::write(&path, "pub fn main() -> Result[(), ()] { missing() }\n").unwrap();
        let error = required_round_trip(&path).unwrap_err();
        assert_eq!(
            error,
            format!("{}: required fixture is no longer accepted", path.display())
        );
    });
}

#[test]
fn a_required_fixture_that_omits_a_category_is_refused() {
    with_compiler_stack(|| {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("no-fact.hir");
        // Still an accepted distinct type and opaque owner, but no fact helper.
        std::fs::write(&path, SEALED.replace(" where v <= 100", "")).unwrap();
        let error = required_round_trip(&path).unwrap_err();
        assert_eq!(
            error,
            format!("{}: re-derives no fact helpers", path.display())
        );
    });
}

#[test]
fn required_helper_and_owner_fingerprints_cannot_be_forged() {
    with_compiler_stack(|| {
        let (_, export) = exported(SEALED);
        let hash = hirsel_syntax::ContentHash::of_bytes(b"forged required fingerprint");
        for name in ["Percent$fact$0", "Meters"] {
            let path = named(name);
            let mut forged = export.clone();
            // Keep every use consistent and recompute all transport digests.
            for signature in &mut forged.signatures {
                if signature.path == path {
                    signature.public.fingerprint = hash;
                    if let Some(view) = &mut signature.owner_view {
                        view.fingerprint = hash;
                    }
                }
                for dependency in &mut signature.dependencies {
                    if dependency.definition.path() == &path {
                        dependency.fingerprint = hash;
                    }
                }
            }
            for definition in &mut forged.definitions {
                if definition.path == path {
                    definition.tir.signature_hash = hash;
                }
            }
            let error = import(&hirsel_proof_export::export(&forged.to_input())).unwrap_err();
            assert_eq!(error.at, format!("{path}.public.fingerprint"), "{error}");
        }
        let mut forged = export.clone();
        let meters = forged
            .signatures
            .iter_mut()
            .find(|s| s.path == named("Meters"))
            .unwrap();
        meters.owner_view.as_mut().unwrap().fingerprint = hash;
        let error = import(&hirsel_proof_export::export(&forged.to_input())).unwrap_err();
        assert_eq!(error.at, "standalone@0/Meters.owner_view", "{error}");
    });
}

#[test]
#[ignore = "full gate"]
fn full_gate_every_accepted_program_round_trips() {
    round_trip_all(|_| true, &[]);
}

/// Every erased-carrier -ok fixture must import, including declarations with
/// no executable body. Do not silently skip a fixture that stops compiling.
#[test]
fn every_erased_carrier_ok_program_round_trips() {
    with_compiler_stack(|| {
        let mut programs = vec![];
        accepted(&corpus(), &mut programs);
        programs.retain(|path| {
            let name = path.file_name().unwrap().to_str().unwrap();
            name.contains("erased-carrier") && name.ends_with("-ok.hir")
        });
        assert!(programs.len() >= 5, "missing erased-carrier fixtures");
        let mut failures = vec![];
        for path in &programs {
            match round_trip(path) {
                Ok(Some(_)) => {}
                Ok(None) => failures.push(format!("{}: no longer accepted", path.display())),
                Err(error) => failures.push(format!("{}: {error}", path.display())),
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
        eprintln!(
            "proof export: all {} erased-carrier -ok programs round-trip",
            programs.len()
        );
    });
}

#[test]
fn a_recursive_type_group_round_trips_with_its_group_keys() {
    with_compiler_stack(|| {
        let source = "type Tree = Leaf | Node(box Entry)\n\
                      type Entry { left: Tree, key: u64, right: Tree }\n\
                      fn size(t: Tree) -> u64 {\n  match t {\n    Leaf() => 0u64,\n    \
                      Node(e) => size(e.left) + 1u64 + size(e.right),\n  }\n}\n\
                      pub fn main() -> Result[(), ()] {\n  let n = size(Leaf())\n  Ok(())\n}\n";
        let (text, export) = exported(source);
        assert_eq!(hirsel_proof_export::export(&export.to_input()), text);
        for name in ["Tree", "Entry"] {
            let section = export
                .signatures
                .iter()
                .find(|s| s.public.definition == hirsel_syntax::DefId::intern(&named(name)))
                .unwrap_or_else(|| panic!("no signature for {name}"));
            assert!(section.public.metadata.recursive_group.is_some(), "{name}");
        }
    });
}

/// PS-17: annotations at every site kind, beside a blanked twin whose
/// annotations are spaces of the same length.
pub(crate) const ANNOTATED: &str = "@analysis(acme.units: result = m/s)\n\
     fn speed(@analysis(unit: m) d: u64, @analysis(unit: s) t: u64) -> u64\n  requires t > 0u64 {\n  d / t\n}\n\
     @analysis(taint)\ntype Point {\n  @analysis(unit: m) x: u64,\n  y: u64,\n}\n\
     type Shape = Dot(@analysis(unit: [m, (s)]) u64) | Empty\n\
     distinct type Meters(@analysis(unit: m) v: u64)\n\
     pub fn main() -> Result[(), ()] {\n  let n = speed(10u64, 2u64)\n  Ok(())\n}\n";

pub(crate) fn blanked(source: &str) -> String {
    let mut out = source.to_string();
    while let Some(start) = out.find("@analysis(") {
        let mut depth = 0;
        let end = start
            + out[start..]
                .char_indices()
                .find(|&(_, c)| {
                    depth += match c {
                        '(' | '[' => 1,
                        ')' | ']' => -1,
                        _ => 0,
                    };
                    c == ')' && depth == 0
                })
                .unwrap()
                .0;
        out.replace_range(start..=end, &" ".repeat(end + 1 - start));
    }
    out
}

#[test]
fn annotations_round_trip_at_every_site_and_move_no_other_digest() {
    with_compiler_stack(|| {
        use hirsel_proof_export::Anchor;
        let (text, export) = exported(ANNOTATED);
        assert_eq!(hirsel_proof_export::export(&export.to_input()), text);
        let written: Vec<_> = export
            .annotations
            .iter()
            .map(|a| {
                format!(
                    "{} {:?} {} [{}] {}:{}",
                    a.path.0.name,
                    a.anchor,
                    a.name,
                    a.payload.join(" "),
                    a.location.line,
                    a.location.column
                )
            })
            .collect();
        assert_eq!(
            written,
            [
                "Meters Field(0) unit [m] 12:22",
                "Point Item taint [] 6:1",
                "Point Field(0) unit [m] 8:3",
                "Shape Payload { variant: 0, field: 0 } unit [[ m , ( s ) ]] 11:18",
                "speed Item acme.units [result = m / s] 1:1",
                "speed Parameter(0) unit [m] 2:10",
                "speed Parameter(1) unit [s] 2:37",
            ]
        );
        assert!(export.annotations.iter().any(|a| a.anchor == Anchor::Item));
        let (plain_text, plain) = exported(&blanked(ANNOTATED));
        assert!(plain.annotations.is_empty());
        assert_ne!(plain.annotation_digest, export.annotation_digest);
        assert_eq!(plain.header, export.header, "the program revision moved");
        let fingerprints = |e: &Export| {
            e.signatures
                .iter()
                .map(|s| (s.path.clone(), s.public.fingerprint))
                .collect::<Vec<_>>()
        };
        assert_eq!(fingerprints(&plain), fingerprints(&export));
        for (a, b) in plain.definitions.iter().zip(&export.definitions) {
            assert_eq!(a.path, b.path);
            assert_eq!(a.tir_digest, b.tir_digest, "{}", a.path);
            assert_eq!(a.locations, b.locations, "{}", a.path);
            assert_eq!(a.obligations, b.obligations, "{}", a.path);
        }
        // Without its annotations, the export is the blanked program's, byte for byte.
        let mut input = export.to_input();
        input.annotations.clear();
        assert_eq!(hirsel_proof_export::export(&input), plain_text);
    });
}

#[test]
fn a_forged_or_unsorted_annotation_is_refused() {
    with_compiler_stack(|| {
        use hirsel_proof_export::Anchor;
        let (_, export) = exported(ANNOTATED);
        // A payload edit without the digest.
        let text = hirsel_proof_export::export(&export.to_input());
        let edited = text.replacen("\"acme.units\"", "\"acme.unit\"", 1);
        let error = import(&edited).unwrap_err();
        assert_eq!(error.at, "annotation_digest", "{error}");
        // Recomputed digests leave only the section's own checks.
        let refused = |edit: &dyn Fn(&mut Vec<hirsel_proof_export::Annotation>)| {
            let mut input = export.to_input();
            edit(&mut input.annotations);
            import(&hirsel_proof_export::export(&input)).unwrap_err()
        };
        let error = refused(&|a| a.reverse());
        assert_eq!(error.message, "annotations are not sorted by definition");
        let error = refused(&|a| a[5].anchor = Anchor::Parameter(2));
        assert!(error.message.contains("is not a site of"), "{error}");
        let error = refused(&|a| a[1].anchor = Anchor::Parameter(0));
        assert!(error.message.contains("is not a site of"), "{error}");
        let error = refused(&|a| a[0].name = "9units".into());
        assert!(error.message.contains("dotted analysis name"), "{error}");
        let error = refused(&|a| a[0].path = named("nothing"));
        assert!(error.message.contains("no exported signature"), "{error}");
    });
}

#[test]
fn scoped_contract_and_classification_round_trip_and_cannot_be_stripped() {
    with_compiler_stack(|| {
        let source = "fn invoke(f:fn()->u64 uses {})->u64 uses {f} {f()}\npub fn main()->Result[(),u64] {let n=3u64;Error(invoke(fn()->u64 uses {} {n}))}";
        let (text, export) = exported(source);
        assert_eq!(hirsel_proof_export::export(&export.to_input()), text);
        let mut input = export.to_input();
        let path = named("invoke");
        let mut forged = (*input.definitions[&path]).clone();
        let body = &mut forged.bodies[forged.entry.0 as usize];
        assert_eq!(body.scoped_parameters.len(), 1);
        body.scoped_parameters.clear();
        input.definitions.insert(path, std::sync::Arc::new(forged));
        let error = import(&hirsel_proof_export::export(&input)).unwrap_err();
        assert!(error.message.starts_with("V rejects"), "{error}");
    });
}

#[test]
fn unsupported_generic_borrow_keeps_export_import_acceptance() {
    with_compiler_stack(|| {
        let source = "fn discard[T](x:T) {}\nfn invoke(f:fn()->u64 uses {})->u64 uses {f} {discard(f);f()}\npub fn main()->Result[(),u64] {let n=3u64;Error(invoke(fn()->u64 uses {} {n}))}";
        let (text, export) = exported(source);
        assert_eq!(hirsel_proof_export::export(&export.to_input()), text);
    });
}
