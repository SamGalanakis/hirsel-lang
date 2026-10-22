//! Imaged packages (docs/building.md, "The std image") change only how long checking
//! takes: an image's products equal freshly checked ones, and an image is used only for
//! the exact compiler and sources it was checked under.
use crate::image_fixture::*;
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_syntax::DefId;
use std::{path::PathBuf, sync::Arc};

impl Fixture {
    fn library(&self, source: &str) -> PathBuf {
        self.library_named("lib", source)
    }
    fn library_named(&self, name: &str, source: &str) -> PathBuf {
        self.write("lib.hir", source);
        let manifest = serde_json::json!({
            "schema": 1,
            "package": {"name": name, "version": "1"},
            "sources": [{"path": "lib.hir", "module": ["lib"]}],
            "dependencies": [],
        });
        let path = self.0.join(format!("{name}.json"));
        std::fs::write(&path, manifest.to_string()).unwrap();
        path
    }
}
#[path = "../support/canonical_debug.rs"]
mod canonical_debug;
use canonical_debug::{canonical_ids, placed};

/// Every std definition in the image lowers to the same TIR and has the same revision
/// node as in a fresh check, in a program that places std's sources at other
/// FileIds than the program the image was written in.
#[test]
fn imaged_std_products_equal_fresh_ones() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        let image = std_image();
        // `aaa` places std after the program's own sources, exercising FileId rebasing.
        let client = f.program("aaa", STD_PROGRAM, &std_manifest(), "std");
        let mut imaged = load(&client, &StdImages::of(vec![image.clone()]));
        assert!(
            imaged.engine.imaged(&package("std")),
            "{:?}",
            imaged.image_notes
        );
        let mut reference = load(&client, &StdImages::none());
        let checked = |p: &mut LoadedProgram| {
            let output = p.check(hirsel_engine::LoweringPolicy::Reachable).unwrap();
            let rendered: Vec<_> = output
                .diagnostics
                .iter()
                .map(|d| d.render_human(&output.sources))
                .collect();
            (output.completion, rendered)
        };
        assert_eq!(checked(&mut imaged), checked(&mut reference));
        let entry = imaged.entry.clone();
        assert_eq!(
            imaged.engine.semantic_program_revision(&entry).unwrap(),
            reference.engine.semantic_program_revision(&entry).unwrap()
        );
        let mut lowered = 0;
        for path in image.definitions().into_iter().map(DefId::path) {
            assert_eq!(
                imaged.engine.semantic_program_revision(path).unwrap(),
                reference.engine.semantic_program_revision(path).unwrap(),
                "{path}"
            );
            let Ok(expected) = reference.engine.lower_to_tir(path) else {
                continue;
            };
            let actual = imaged.engine.lower_to_tir(path).unwrap();
            assert_eq!(
                imaged.engine.source_audit(path).unwrap(),
                reference.engine.source_audit(path).unwrap(),
                "{path}"
            );
            // Anchors are process-unique, so each is named by its placement;
            // TypeId's Debug omits provenance.
            // Each session interns its own rows: compare them by content.
            let (types, value) = (imaged.engine.types().clone(), &actual.value);
            let actual_text = canonical_ids(
                &types,
                || serde_json::to_vec(value.as_ref()).unwrap(),
                format!("{value:?}"),
            );
            let (types, value) = (reference.engine.types().clone(), &expected.value);
            let expected_text = canonical_ids(
                &types,
                || serde_json::to_vec(value.as_ref()).unwrap(),
                format!("{value:?}"),
            );
            assert_eq!(
                placed(&imaged.engine.sources, actual_text),
                placed(&reference.engine.sources, expected_text),
                "{path}"
            );
            assert_eq!(actual.dependencies, expected.dependencies, "{path}");
            lowered += 1;
        }
        assert!(lowered > 100, "{lowered}");
        let actual = imaged.engine.export_native_input().unwrap();
        let expected = reference.engine.export_native_input().unwrap();
        assert_eq!(actual.audits, expected.audits);
        assert_eq!(
            canonical_ids(
                &actual.types,
                || serde_json::to_vec(&actual.materialized.values().collect::<Vec<_>>()).unwrap(),
                format!("{:?}", actual.materialized)
            ),
            canonical_ids(
                &expected.types,
                || serde_json::to_vec(&expected.materialized.values().collect::<Vec<_>>()).unwrap(),
                format!("{:?}", expected.materialized)
            )
        );
        assert_eq!(
            canonical_ids(
                &actual.types,
                || serde_json::to_vec(&actual.constants.values().collect::<Vec<_>>()).unwrap(),
                format!("{:?}", actual.constants)
            ),
            canonical_ids(
                &expected.types,
                || serde_json::to_vec(&expected.constants.values().collect::<Vec<_>>()).unwrap(),
                format!("{:?}", expected.constants)
            )
        );
        imaged.engine.import_native_input(actual).unwrap();
        for path in image.definitions().into_iter().map(DefId::path) {
            for phase in [QueryKind::Check, QueryKind::Modes] {
                assert_eq!(
                    imaged.engine.recompute_count(path, phase),
                    0,
                    "{path}: {phase:?}"
                );
            }
        }
    });
}

const LIBRARY: &str = "pub fn value(x: u64) -> u64 {
  x + 1
}
";
const CLIENT: &str = "pub fn main() -> Result[(), ()] {
  failure.check(lib.value(1) == 2, Arithmetic())
  Ok(())
}
";

fn library_image(f: &Fixture, source: &str) -> Result<Arc<StdImage>, ToolError> {
    let manifest = f.library(source);
    let program = f.program("imager", CLIENT, &manifest, "lib");
    let bytes = image(f, &mut load(&program, &StdImages::none()), "lib")?;
    Ok(StdImage::from_bytes(bytes).unwrap())
}

/// `program`'s image of the dependency `name`, written to memory.
fn image(f: &Fixture, program: &mut LoadedProgram, name: &str) -> Result<Vec<u8>, ToolError> {
    let spill = std::fs::File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(f.0.join("image.records"))
        .unwrap();
    program.image(&package(name), spill)
}

#[test]
fn an_image_serves_only_the_compiler_and_sources_it_was_checked_under() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        let image = library_image(&f, LIBRARY).unwrap();
        let client = f.program("client", CLIENT, &f.0.join("lib.json"), "lib");
        let images = StdImages::of(vec![image.clone()]);
        let mut program = load(&client, &images);
        assert!(program.engine.imaged(&package("lib")));
        assert!(program.image_notes.is_empty());
        program.check_for_execution().unwrap();

        // Another compiler identity.
        let other = Compiler::new(FrontPhases::shared, [7; 32]);
        let program = other
            .load_program_source_with_images(&client, None, &images)
            .unwrap();
        assert!(!program.engine.imaged(&package("lib")));
        assert_eq!(program.image_notes.len(), 1, "{:?}", program.image_notes);

        // Other sources: a comment changes the package's checked input.
        f.library(&format!("{LIBRARY}// edited\n"));
        let mut program = load(&client, &images);
        assert!(!program.engine.imaged(&package("lib")));
        assert_eq!(program.image_notes.len(), 1, "{:?}", program.image_notes);
        program.check_for_execution().unwrap();
    });
}

#[test]
fn a_file_image_rebuilds_for_sources_compiler_and_limits_and_reuses_unchanged() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        let manifest = f.library(LIBRARY);
        let client = f.program("client", CLIENT, &manifest, "lib");
        let path = f.0.join("lib.image");
        COMPILER.image_package(&manifest, &path).unwrap();
        let images = StdImages::at(&path);
        let check = |compiler: Compiler| {
            let mut program = compiler
                .load_program_source_with_images(&client, None, &images)
                .unwrap();
            assert!(
                program.engine.imaged(&package("lib")),
                "{:?}",
                program.image_notes
            );
            assert!(
                program
                    .check(LoweringPolicy::Reachable)
                    .unwrap()
                    .diagnostics
                    .is_empty()
            );
            StdImage::open(&path).unwrap().key()
        };
        let original = std::fs::read(&path).unwrap();
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        let key = check(COMPILER);
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            modified
        );
        f.library(&format!("{LIBRARY}// edited\n"));
        let edited = check(COMPILER);
        assert_ne!(edited, key);
        let other = Compiler::new(FrontPhases::shared, [7; 32]);
        let compiler = check(other.clone());
        assert_ne!(compiler, edited);
        let steps = check(other.clone().with_comptime_limits(hirsel_interp::Limits {
            steps: Some(999_999),
            call_depth: 256,
        }));
        assert_ne!(steps, compiler);
        let depth = check(other.with_comptime_limits(hirsel_interp::Limits {
            steps: Some(999_999),
            call_depth: 128,
        }));
        assert_ne!(depth, steps);
    });
}

#[test]
fn editing_an_imaged_source_in_a_session_ends_its_image() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        let image = library_image(&f, LIBRARY).unwrap();
        let client = f.program("client", CLIENT, &f.0.join("lib.json"), "lib");
        let mut program = load(&client, &StdImages::of(vec![image]));
        assert!(program.engine.imaged(&package("lib")));
        let (file, context) = program
            .files
            .iter()
            .find(|(_, c)| c.package == package("lib"))
            .cloned()
            .unwrap();
        let path = program.engine.sources.file(file).unwrap().path.clone();
        let edited = LIBRARY.replace("x + 1", "x + 2");
        let file = program.engine.sources.add(&path, edited).unwrap();
        program.engine.parse_file(file, &context).unwrap();
        assert!(!program.engine.imaged(&package("lib")));
        // The edited body is checked and run from source.
        let warm = program.check(LoweringPolicy::Reachable).unwrap();
        f.library(&LIBRARY.replace("x + 1", "x + 2"));
        let mut cold = load(&client, &StdImages::none());
        let fresh = cold.check(LoweringPolicy::Reachable).unwrap();
        assert_eq!(warm.completion, fresh.completion);
        assert_eq!(
            warm.diagnostics
                .iter()
                .map(|d| d.render_human(&warm.sources))
                .collect::<Vec<_>>(),
            fresh
                .diagnostics
                .iter()
                .map(|d| d.render_human(&fresh.sources))
                .collect::<Vec<_>>()
        );
        let entry = program.entry.clone();
        assert_eq!(
            program.engine.semantic_program_revision(&entry).unwrap(),
            cold.engine.semantic_program_revision(&entry).unwrap()
        );
    });
}

/// O2's std-touching case uses a small std package so it exercises std's
/// module-head policy and image retirement without rechecking the full library.
#[test]
fn editing_an_imaged_std_module_is_warm_equals_cold() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        let manifest = f.library_named("std", LIBRARY);
        let client = f.program("client", CLIENT, &manifest, "std");
        let bytes = image(&f, &mut load(&client, &StdImages::none()), "std").unwrap();
        let images = StdImages::of(vec![StdImage::from_bytes(bytes).unwrap()]);
        let mut warm = load(&client, &images);
        warm.check_for_execution().unwrap();
        assert!(warm.engine.imaged(&package("std")));
        let (file, context) = warm
            .files
            .iter()
            .find(|(_, context)| context.package == package("std"))
            .cloned()
            .unwrap();
        let path = warm.engine.sources.file(file).unwrap().path.clone();
        let edited = LIBRARY.replace("x + 1", "x + 2");
        f.library_named("std", &edited);
        let file = warm.engine.sources.add(path, edited).unwrap();
        warm.engine.parse_file(file, &context).unwrap();
        assert!(!warm.engine.imaged(&package("std")));
        let mut cold = load(&client, &StdImages::none());
        let checked = |program: &mut LoadedProgram| {
            let output = program.check(LoweringPolicy::Reachable).unwrap();
            let diagnostics: Vec<_> = output
                .diagnostics
                .iter()
                .map(|d| d.render_human(&output.sources))
                .collect();
            (output.completion, diagnostics)
        };
        assert_eq!(checked(&mut warm), checked(&mut cold));
        let entry = warm.entry.clone();
        assert_eq!(
            warm.engine.semantic_program_revision(&entry).unwrap(),
            cold.engine.semantic_program_revision(&entry).unwrap()
        );
    });
}

/// The image must retain the source evidence and compile-time substitutions, not
/// just a completion bit. Fresh import is the accepted control for both forgeries.
#[test]
fn imaged_native_products_equal_fresh_and_reject_forged_audits() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        let source = "const BASE: u64 = comptime 1\n@noalloc\npub fn value(x: u64) -> u64 {\n  x + comptime BASE\n}\n";
        let image = library_image(&f, source).unwrap();
        let client = f.program("client", CLIENT, &f.0.join("lib.json"), "lib");
        let mut cached = load(&client, &StdImages::of(vec![image.clone()]));
        let mut fresh = load(&client, &StdImages::none());
        cached.check_for_execution().unwrap();
        fresh.check_for_execution().unwrap();
        let actual = cached.engine.export_native_input().unwrap();
        let expected = fresh.engine.export_native_input().unwrap();
        assert_eq!(actual.audits, expected.audits);
        assert_eq!(
            canonical_ids(
                &actual.types,
                || serde_json::to_vec(&actual.materialized.values().collect::<Vec<_>>()).unwrap(),
                format!("{:?}", actual.materialized)
            ),
            canonical_ids(
                &expected.types,
                || serde_json::to_vec(&expected.materialized.values().collect::<Vec<_>>()).unwrap(),
                format!("{:?}", expected.materialized)
            )
        );
        assert_eq!(
            canonical_ids(
                &actual.types,
                || serde_json::to_vec(&actual.constants.values().collect::<Vec<_>>()).unwrap(),
                format!("{:?}", actual.constants)
            ),
            canonical_ids(
                &expected.types,
                || serde_json::to_vec(&expected.constants.values().collect::<Vec<_>>()).unwrap(),
                format!("{:?}", expected.constants)
            )
        );
        for path in image.definitions().into_iter().map(DefId::path) {
            for phase in [QueryKind::Check, QueryKind::Modes] {
                assert_eq!(
                    cached.engine.recompute_count(path, phase),
                    0,
                    "{path}: {phase:?}"
                );
                assert!(
                    fresh.engine.recompute_count(path, phase) > 0,
                    "{path}: {phase:?}"
                );
            }
        }
        let path = actual
            .audits
            .keys()
            .find(|path| path.0.package == package("lib") && path.0.name.as_str() == "value")
            .unwrap()
            .clone();
        assert!(actual.audits[&path].noalloc);
        cached.engine.import_native_input(actual.clone()).unwrap();
        let mut forged = actual.clone();
        forged.audits.get_mut(&path).unwrap().noalloc = false;
        assert!(cached.engine.import_native_input(forged).is_err());
        let mut missing = actual;
        missing.audits.remove(&path);
        assert!(cached.engine.import_native_input(missing).is_err());

        let (file, context) = cached
            .files
            .iter()
            .find(|(_, context)| context.package == package("lib"))
            .cloned()
            .unwrap();
        let path = cached.engine.sources.file(file).unwrap().path.clone();
        let file = cached
            .engine
            .sources
            .add(&path, source.replace("comptime 1", "comptime 2"))
            .unwrap();
        cached.engine.parse_file(file, &context).unwrap();
        assert!(!cached.engine.imaged(&package("lib")));
        cached.check_for_execution().unwrap();
        let edited = cached.engine.export_native_input().unwrap();
        assert_ne!(
            format!("{:?}", edited.constants),
            format!("{:?}", expected.constants)
        );
        assert_ne!(
            format!("{:?}", edited.materialized),
            format!("{:?}", expected.materialized)
        );
    });
}

/// An image is written only for a package that checks clean; its accepted twin is
/// `an_image_serves_only_the_compiler_and_sources_it_was_checked_under`.
#[test]
fn a_package_with_diagnostics_is_never_imaged() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        let rejected = library_image(&f, "pub fn value(x: u64) -> u64 {\n  x + true\n}\n");
        assert!(rejected.is_err());
    });
}

/// Exercise the compiler's products, including source anchors, core rows and
/// query read sets, rather than only the image container's synthetic values.
#[test]
fn compiler_image_bytes_ignore_id_salts() {
    let bytes = || {
        with_compiler_stack(|| {
            let f = Fixture::new();
            let manifest = f.library(LIBRARY);
            let client = f.program("imager", CLIENT, &manifest, "lib");
            let mut program = load(&client, &StdImages::none());
            let bytes = image(&f, &mut program, "lib").unwrap();
            format!(
                "{} {:?}",
                bytes.len(),
                hirsel_syntax::ContentHash::of_bytes(&bytes)
            )
        })
    };
    let Some(runs) = hirsel_intern::under_salts(
        "image::compiler_image_bytes_ignore_id_salts",
        &[1, 2, 0x5eed],
        bytes,
    ) else {
        return;
    };
    assert!(runs.iter().all(|run| run == &runs[0]), "{runs:#?}");
    assert_eq!(runs[0], bytes());
}

/// Loading policy must preserve diagnostic selection and placement, including
/// rejection at a call into an imaged signature and its accepted control.
#[test]
fn image_diagnostics_equal_source_for_rejected_and_accepted_clients() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        let image = library_image(&f, LIBRARY).unwrap();
        let images = StdImages::of(vec![image]);
        for (source, rejected) in [
            (CLIENT.replace("lib.value(1)", "lib.value(true)"), true),
            (CLIENT.into(), false),
        ] {
            let client = f.program("client", &source, &f.0.join("lib.json"), "lib");
            let check = |images: &StdImages| {
                let mut program = load(&client, images);
                let output = program.check(LoweringPolicy::Comptime).unwrap();
                let diagnostics: Vec<_> = output
                    .diagnostics
                    .iter()
                    .map(|d| d.render_human(&output.sources))
                    .collect();
                assert_eq!(!diagnostics.is_empty(), rejected);
                (output.completion, diagnostics)
            };
            assert_eq!(check(&images), check(&StdImages::none()));
        }
    });
}

#[test]
fn imaged_generic_comptime_materializes_selected_client_sizes() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        let source = "pub fn value[n:size](xs:Inline[u8,n])->u64{let _=xs;comptime (n*n)}\n";
        let client = "pub fn main()->Result[(),()]{let a:Inline[u8,2]=[1,2];let b:Inline[u8,3]=[1,2,3];failure.check(lib.value(a)==4u64,Arithmetic());failure.check(lib.value(b)==9u64,Arithmetic());Ok(())}\n";
        let library = f.library(source);
        let imager = f.program("imager", client, &library, "lib");
        let bytes = image(&f, &mut load(&imager, &StdImages::none()), "lib").unwrap();
        let image = StdImage::from_bytes(bytes).unwrap();
        let client = f.program("client", client, &library, "lib");
        let mut cached = load(&client, &StdImages::of(vec![image]));
        let mut fresh = load(&client, &StdImages::none());
        assert!(cached.engine.imaged(&package("lib")));
        cached.check_for_execution().unwrap();
        fresh.check_for_execution().unwrap();
        let actual = cached.engine.export_native_input().unwrap();
        let expected = fresh.engine.export_native_input().unwrap();
        let selected = |input: &NativeInput| {
            input
                .constants
                .iter()
                .filter(|(path, _)| path.0.name.starts_with("$constant:$comptime:value:"))
                .map(|(path, value)| (path.clone(), value.root.kind.clone()))
                .collect::<std::collections::BTreeMap<_, _>>()
        };
        let values = selected(&actual);
        assert_eq!(values, selected(&expected));
        assert_eq!(
            values.values().cloned().collect::<Vec<_>>(),
            vec![
                hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Integer(4)),
                hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Integer(9)),
            ]
        );
        cached.engine.import_native_input(actual).unwrap();
    });
}

/// Published summaries come from the image without retaining or rebuilding a
/// checked/mode body, and match a source compilation with another TypeInterner.
#[test]
fn imaged_summary_equals_source_without_body_recomputation() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        let image = library_image(&f, LIBRARY).unwrap();
        let library = f.0.join("lib.json");
        let client = f.program("zzz", CLIENT, &library, "lib");
        let mut cached = load(&client, &StdImages::of(vec![image]));
        let mut fresh = load(&client, &StdImages::none());
        let path = hirsel_syntax::DefPath(hirsel_syntax::ItemPath {
            package: package("lib"),
            modules: vec!["lib".into()],
            name: "value".into(),
        });
        let id = DefId::intern(&path);
        let actual = cached.engine.definition_summary(id).unwrap();
        let expected = fresh.engine.definition_summary(id).unwrap();
        let canonical = |engine: &QueryEngine, summary: &PublishedSummary| {
            placed(
                &engine.sources,
                canonical_ids(
                    engine.types(),
                    || serde_json::to_vec(summary).unwrap(),
                    format!("{summary:?}"),
                ),
            )
        };
        assert_eq!(
            canonical(&cached.engine, &actual),
            canonical(&fresh.engine, &expected)
        );
        assert!(Arc::ptr_eq(
            &actual,
            &cached.engine.definition_summary(id).unwrap()
        ));
        for phase in [
            QueryKind::Resolve,
            QueryKind::Check,
            QueryKind::Modes,
            QueryKind::Summary,
        ] {
            assert_eq!(cached.engine.recompute_count(&path, phase), 0, "{phase:?}");
        }
        assert_eq!(fresh.engine.recompute_count(&path, QueryKind::Modes), 1);
    });
}

/// Only the new program signature needs a V declaration check: core and image
/// views carry successful receipts checked against independently derived digests.
#[test]
fn imaged_snapshot_reuses_signature_verdicts() {
    let run = || {
        with_compiler_stack(|| {
            use hirsel_intern::work::{self, Counter};
            let f = Fixture::new();
            let image = library_image(&f, LIBRARY).unwrap();
            let library = f.0.join("lib.json");
            let client = f.program("zzz", CLIENT, &library, "lib");
            let mut cached = load(&client, &StdImages::of(vec![image]));
            assert!(cached.engine.imaged(&package("lib")));
            let before = work::read(Counter::ValidateSignatureRows);
            let snapshot = cached.engine.signature_snapshot();
            assert_eq!(
                work::read(Counter::ValidateSignatureRows) - before,
                1,
                "only the new program signature should be checked"
            );
            assert!(
                snapshot
                    .validation(&package("lib"))
                    .universe_errors()
                    .1
                    .is_empty()
            );
            "seeded".to_owned()
        })
    };
    if let Some(outputs) = hirsel_intern::under_salts(
        "image::imaged_snapshot_reuses_signature_verdicts",
        &[1],
        run,
    ) {
        assert_eq!(outputs, ["seeded"]);
    }
}

/// Asking directly for syntax must materialize its image source. A content-key
/// read alone is syntax-free, so explicit parse/resolve demands own this step.
#[test]
fn explicit_image_syntax_requests_materialize_the_source() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        let image = library_image(&f, LIBRARY).unwrap();
        let library = f.0.join("lib.json");
        let client = f.program("client", CLIENT, &library, "lib");
        let path = hirsel_syntax::DefPath(hirsel_syntax::ItemPath {
            package: package("lib"),
            modules: vec!["lib".into()],
            name: "value".into(),
        });
        for resolve in [false, true] {
            let mut cached = load(&client, &StdImages::of(vec![image.clone()]));
            assert!(cached.engine.imaged(&package("lib")));
            if resolve {
                cached.engine.resolve_definition(&path).unwrap();
            } else {
                let syntax = cached.engine.parse_definition(&path).unwrap();
                assert_eq!(syntax.value.item.definition, path);
            }
            // A syntax demand does not retire its still-current image.
            assert!(cached.engine.imaged(&package("lib")));
        }
    });
}

/// V accepted the writer's exact TIR. Its receipt avoids body V while all
/// contextual reads hold; a changed FirstHeap selection requires revalidation.
#[test]
fn imaged_lowering_reuses_validated_evidence() {
    let run = || {
        with_compiler_stack(|| {
            use hirsel_intern::timings::{self, Count};
            let f = Fixture::new();
            let image = library_image(&f, LIBRARY).unwrap();
            let library = f.0.join("lib.json");
            let path = hirsel_syntax::DefPath(hirsel_syntax::ItemPath {
                package: package("lib"),
                modules: vec!["lib".into()],
                name: "value".into(),
            });
            // Both roots use the same source. Only `aaa` sorts before core's
            // heap-role declaration, changing V's FirstHeap read.
            for (name, expected_runs) in [("zzz", 0), ("aaa", 1)] {
                let client = f.program(name, CLIENT, &library, "lib");
                let mut program = load(&client, &StdImages::of(vec![image.clone()]));
                assert!(program.engine.imaged(&package("lib")));
                program.engine.signature_snapshot();
                timings::enable();
                let count = || {
                    timings::report()
                        .counts
                        .into_iter()
                        .find(|(c, _)| *c == Count::VRuns)
                        .unwrap()
                        .1
                };
                let before = count();
                let first = program.engine.lower_to_tir(&path).unwrap();
                assert_eq!(
                    count() - before,
                    expected_runs,
                    "{name}: contextual V reads"
                );
                let second = program.engine.lower_to_tir(&path).unwrap();
                assert!(Arc::ptr_eq(&first.value, &second.value));
                assert_eq!(
                    count() - before,
                    expected_runs,
                    "repeated demand must reuse its verdict"
                );
                assert_eq!(program.engine.recompute_count(&path, QueryKind::Lower), 0);
            }
            "reused".to_owned()
        })
    };
    // Process isolation keeps timing counters independent of other harness tests.
    if let Some(outputs) = hirsel_intern::under_salts(
        "image::imaged_lowering_reuses_validated_evidence",
        &[1],
        run,
    ) {
        assert_eq!(outputs, ["reused"]);
    }
}

/// An image's lowered substitutions preserve their dependency requests when
/// used beneath a client's constant chain, just like a cold source lowering.
#[test]
fn imaged_helper_lowering_preserves_dependency_depth() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        let source = "pub const LEAF: u64 = 1u64\npub fn helper()->u64{comptime LEAF}\n";
        let library = f.library(source);
        let main = "pub fn main()->Result[(),()]{Ok(())}\n";
        let imager = f.program("imager", main, &library, "lib");
        let bytes = image(&f, &mut load(&imager, &StdImages::none()), "lib").unwrap();
        let stored = StdImage::from_bytes(bytes).unwrap();
        let mut client = main.to_owned();
        // The compiler dependency budget is min(call_depth,32). Ordinary
        // interpreter calls remain shallow in this fixture.
        for n in 0..31 {
            client.push_str(&format!("const C{n}:u64=comptime C{}\n", n + 1));
        }
        client.push_str("const C31:u64=comptime lib.helper()\n");
        let manifest = f.program("client", &client, &library, "lib");
        let root = hirsel_syntax::DefPath(hirsel_syntax::ItemPath {
            package: package("client"),
            modules: vec![],
            name: "C0".into(),
        });
        let mut results = vec![];
        for images in [StdImages::none(), StdImages::of(vec![stored])] {
            let mut program = load(&manifest, &images);
            let result = program.engine.materialize_constant(&root);
            assert!(
                matches!(result, Err(QueryError::Operational(_))),
                "{result:?}"
            );
            let limit = program.engine.last_comptime_limit.as_ref().unwrap();
            assert_eq!(limit.kind, hirsel_tir::ToolingLimitKind::CallDepth);
            assert_eq!(limit.limit, 32);
            let site = program.engine.sources.resolve(limit.site.unwrap());
            let file = program.engine.sources.file(site.file).unwrap();
            assert_eq!(site.start, source.find("comptime LEAF").unwrap() as u32 + 9);
            results.push((file.path.clone(), site.start, site.end));
        }
        assert_eq!(results[0], results[1]);
    });
}

/// Worker acceptance may depend on the imager's root signatures. Such optional
/// constants/lowerings must be omitted rather than exported as a closure proof.
#[test]
fn frozen_s1_products_do_not_export_root_validation_universe_receipts() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        let library =
            f.library("pub opaque type Hidden: plain uses {} { value: u32 }\npub fn helper()->u32{let h=Hidden(value:7u32);h.value}\npub const VALUE:u32=comptime helper()\n");
        let main = "pub fn main()->Result[(),()]{Ok(())}\n";
        let source = format!("{main}const ROOT:u32=comptime lib.helper()\n");
        let manifest = f.program("imager", &source, &library, "lib");
        let mut program = load(&manifest, &StdImages::none());
        let path = DefPath(hirsel_syntax::ItemPath {
            package: package("lib"),
            modules: vec!["lib".into()],
            name: "VALUE".into(),
        });
        let root = DefPath(hirsel_syntax::ItemPath {
            package: package("imager"),
            modules: vec![],
            name: "ROOT".into(),
        });
        for jobs in [1, 2, 8] {
            let values = program.engine.materialize_values(
                vec![path.clone(), root.clone(), path.clone()],
                hirsel_intern::jobs::Jobs::new(jobs),
            );
            assert!(values.iter().all(Result::is_ok), "{values:?}");
        }
        program.engine.lower_to_tir(&root).unwrap();
        let reads = program.engine.memo_reads(&root, QueryKind::Lower).unwrap();
        for origin in ["lib", "imager"] {
            assert!(
                reads
                    .get(&memo::ReadKey::ValidationUniverse(memo::PackageKey::of(
                        &package(origin)
                    )))
                    .is_some(),
                "missing {origin} V input in {reads:?}"
            );
        }
        let bytes = image(&f, &mut program, "lib").unwrap();
        let stored = StdImage::from_bytes(bytes).unwrap();
        let client = f.program("client", main, &library, "lib");
        let mut imaged = load(&client, &StdImages::of(vec![stored]));
        assert!(imaged.engine.imaged(&package("lib")));
        assert_eq!(
            imaged.engine.materialize_constant(&path).unwrap(),
            hirsel_tir::Constant::Integer(7)
        );
    });
}

#[test]
fn image_releases_unrecorded_files_after_parallel_constant_publication() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        let library = f.library("pub fn first()->u32{1u32}\n");
        std::fs::write(
            f.0.join("later.hir"),
            "pub const VALUE:u32=comptime 42u32\n",
        )
        .unwrap();
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&library).unwrap()).unwrap();
        manifest["sources"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"path":"later.hir","module":["later"]}));
        std::fs::write(&library, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let main = "pub fn main()->Result[(),()]{Ok(())}\n";
        let imager = f.program("imager", main, &library, "lib");
        let path = DefPath(hirsel_syntax::ItemPath {
            package: package("lib"),
            modules: vec!["later".into()],
            name: "VALUE".into(),
        });
        let mut producer = load(&imager, &StdImages::none());
        assert!(
            producer
                .engine
                .materialize_values(vec![path.clone()], hirsel_intern::jobs::Jobs::ONE)[0]
                .is_ok()
        );
        producer.engine.lower_to_tir(&path).unwrap();
        assert!(
            producer
                .engine
                .memo_reads(&path, QueryKind::Lower)
                .unwrap()
                .iter()
                .any(|(read, _)| matches!(read, memo::ReadKey::ValidationUniverse(_)))
        );
        let bytes = image(&f, &mut producer, "lib").unwrap();
        let stored = StdImage::from_bytes(bytes).unwrap();
        let client = f.program("client", main, &library, "lib");
        let mut program = load(&client, &StdImages::of(vec![stored]));
        let path = DefPath(hirsel_syntax::ItemPath {
            package: package("lib"),
            modules: vec!["later".into()],
            name: "VALUE".into(),
        });
        assert_eq!(
            program.engine.materialize_constant(&path).unwrap(),
            hirsel_tir::Constant::Integer(42)
        );
    });
}

#[test]
fn frozen_lowering_frontier_replays_imaged_helper_output_and_receipts() {
    let Some(outputs) = hirsel_intern::under_salts(
        "image::frozen_lowering_frontier_replays_imaged_helper_output_and_receipts",
        &[1, 2, 3],
        imaged_frontier_replay_budget_matrix,
    ) else {
        return;
    };
    assert!(outputs.iter().all(|output| output == &outputs[0]));
}

fn imaged_frontier_replay_budget_matrix() -> String {
    with_compiler_stack(|| {
        let mut transcript = String::new();
        let f = Fixture::new();
        let image = library_image(
            &f,
            "const LEAF:u64=1u64\nconst CHAIN:u64=comptime LEAF\npub fn value(x:u64)->u64{x+comptime CHAIN}\npub const BASE:u64=comptime value(1u64)\n",
        )
        .unwrap();
        let client = f.program("worker_image", CLIENT, &f.0.join("lib.json"), "lib");
        let path = hirsel_syntax::DefPath(hirsel_syntax::ItemPath {
            package: package("lib"),
            modules: vec!["lib".into()],
            name: "value".into(),
        });
        for (budget, jobs, fallback) in [0, 1, 1 << 20].into_iter().flat_map(|budget| {
            [1, 2, 8]
                .into_iter()
                .flat_map(move |jobs| [false, true].map(move |fallback| (budget, jobs, fallback)))
        }) {
            let mut cached = load(&client, &StdImages::of(vec![image.clone()]));
            cached.engine = cached.engine.with_config(CompileConfig {
                hold_bytes: budget,
                ..CompileConfig::for_memory_cap(None)
            });
            let mut fresh = load(&client, &StdImages::none());
            assert!(cached.engine.imaged(&package("lib")));
            let expected = fresh.engine.lower_to_tir(&path).unwrap();
            let expected_depth = fresh
                .engine
                .lower_definition_outcomes(vec![path.clone()], hirsel_intern::jobs::Jobs::new(jobs))
                .remove(0)
                .depth
                .unwrap();
            assert!(
                expected_depth.height > 0,
                "fixture must exercise real transparent requests"
            );
            // A missing worker adapter forces the scalar path; image lowering
            // still succeeds before consulting that adapter.
            let lowering = fallback.then(|| cached.engine.adapters.lowering.take().unwrap());
            let outcomes = cached.engine.lower_definition_outcomes(
                vec![path.clone(), path.clone()],
                hirsel_intern::jobs::Jobs::new(jobs),
            );
            if fallback {
                cached.engine.adapters.lowering = lowering;
            }
            for outcome in outcomes {
                assert_eq!(outcome.definition, path);
                assert_eq!(outcome.phase, Some(QueryKind::Lower));
                assert!(outcome.limit.is_none());
                assert_eq!(
                    placed(
                        &cached.engine.sources,
                        format!("{:?}", outcome.depth.as_ref().unwrap())
                    ),
                    placed(&fresh.engine.sources, format!("{expected_depth:?}"))
                );
                let depth = outcome.depth.as_ref().unwrap();
                for stack in 0..=depth.limit() {
                    let coordinator = memo::S1Coordinator::<u32>::new();
                    let mut actual_worker = coordinator.worker();
                    for frame in 0..stack {
                        assert_eq!(
                            actual_worker.enter(frame, hirsel_span::Span::default()),
                            memo::Entered::Run
                        );
                    }
                    assert_eq!(
                        placed(
                            &cached.engine.sources,
                            format!("{:?}", actual_worker.replay_requests(depth))
                        ),
                        placed(
                            &fresh.engine.sources,
                            format!(
                                "{:?}",
                                expected_depth.first[stack as usize].map_or(Ok(()), Err)
                            )
                        ),
                        "jobs={jobs} fallback={fallback} stack={stack}"
                    );
                }
                let actual = outcome.result.unwrap();
                assert_eq!(actual.dependencies, expected.dependencies);
                assert_eq!(actual.diagnostics, expected.diagnostics);
                let actual_text = canonical_ids(
                    cached.engine.types(),
                    || serde_json::to_vec(actual.value.as_ref()).unwrap(),
                    format!("{:?}", actual.value),
                );
                transcript.push_str(
                    &placed(&cached.engine.sources, actual_text.clone())
                        .replace(&f.0.display().to_string(), "<fixture>"),
                );
                let expected_text = canonical_ids(
                    fresh.engine.types(),
                    || serde_json::to_vec(expected.value.as_ref()).unwrap(),
                    format!("{:?}", expected.value),
                );
                assert_eq!(
                    placed(&cached.engine.sources, actual_text),
                    placed(&fresh.engine.sources, expected_text)
                );
                assert!(!outcome.reads.is_empty());
                assert!(outcome.reads.iter().any(|(key, _)| *key
                    == memo::ReadKey::ExecutedBody(hirsel_syntax::DefId::intern(&path))));
                assert!(
                    outcome
                        .reads
                        .iter()
                        .any(|(key, _)| matches!(key, memo::ReadKey::ValidationUniverse(_)))
                );
            }
            for kind in [
                QueryKind::Resolve,
                QueryKind::Check,
                QueryKind::Modes,
                QueryKind::Lower,
            ] {
                assert_eq!(
                    cached.engine.recompute_count(&path, kind),
                    0,
                    "jobs={jobs} {kind:?}"
                );
            }
        }
        transcript
    })
}

const STD_PROGRAM: &str = "pub type AppError = Grant(GrantError) | Output(OutputError)

pub fn main(rt: Io) -> Result[(), AppError] {
  let output = io.stdout(rt) ? Grant
  out.print(output, string.trim(\" hello \")) ? Output
  out.print(output, u64.to_string(42u64)) ? Output
  Ok(())
}
";

#[test]
fn dictionary_literals_and_copied_fields_survive_package_images() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        let library = "pub type Op { run: fn(u64) -> u64 uses {} }\n\
            pub const OP: Op = Op(run: fn(value:u64) uses {} { value + 1 })\n\
            pub const COPY: Op = Op(run: (OP).run)\n";
        let client_source = "pub fn main() -> Result[(), ()] { let op = lib.COPY; failure.check(op.run(8) == 9, Arithmetic()); Ok(()) }\n";
        let manifest = f.library(library);
        let client = f.program("client", client_source, &manifest, "lib");
        let mut fresh = load(&client, &StdImages::none());
        let bytes = image(&f, &mut fresh, "lib").unwrap();
        let images = StdImages::of(vec![StdImage::from_bytes(bytes).unwrap()]);
        let mut imaged = load(&client, &images);
        assert!(imaged.engine.imaged(&package("lib")));
        fresh.check_for_execution().unwrap();
        imaged.check_for_execution().unwrap();
        assert_eq!(
            fresh
                .engine
                .semantic_program_revision(&fresh.entry)
                .unwrap(),
            imaged
                .engine
                .semantic_program_revision(&imaged.entry)
                .unwrap()
        );
        for program in [&mut fresh, &mut imaged] {
            program.engine.closure_assertions = true;
            let execution = run_observed_snapshot(
                &mut program.engine,
                &RunConfiguration {
                    entry: program.entry.clone(),
                    hash_seed: 0,
                    inputs: vec![],
                },
                SimulationConfig::default(),
            )
            .unwrap();
            assert!(
                matches!(
                    execution.outcome,
                    Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
                ),
                "{:?}",
                execution.outcome
            );
            let exported = program.engine.export_tir(&program.entry).unwrap();
            program.engine.import_tir(&exported).unwrap();
            let input = program.engine.export_native_input().unwrap();
            program.engine.import_native_input(input).unwrap();
        }
    });
}
