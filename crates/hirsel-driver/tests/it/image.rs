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
        self.write("lib.hir", source);
        let manifest = serde_json::json!({
            "schema": 1,
            "package": {"name": "lib", "version": "1"},
            "sources": [{"path": "lib.hir", "module": ["lib"]}],
            "dependencies": [],
        });
        let path = self.0.join("lib.json");
        std::fs::write(&path, manifest.to_string()).unwrap();
        path
    }
}
/// `text`, a Debug rendering of a value, with each `TypeId(n)` that `serialize`
/// writes named by that type's canonical table instead, so two sessions'
/// interners compare.
fn canonical_ids(
    t: &hirsel_types::TypeInterner,
    serialize: impl FnOnce() -> Vec<u8>,
    mut text: String,
) -> String {
    let (_, ids) = hirsel_types::with_table_collection(serialize);
    for id in ids {
        let (table, _) = hirsel_types::encode_table(t, &[id]);
        let content = hirsel_syntax::ContentHash::of_bytes(format!("{table:?}").as_bytes());
        // The closing parenthesis keeps TypeId(1) from matching TypeId(12).
        text = text.replace(&format!("{id:?}"), &format!("Type{content:?}"));
    }
    text
}
/// `Anchor(n)` in a Debug rendering, replaced by that anchor's file path and base.
fn placed(sources: &hirsel_span::SourceMap, text: String) -> String {
    let mut out = String::new();
    let mut rest = text.as_str();
    while let Some(at) = rest.find("Anchor(") {
        out.push_str(&rest[..at]);
        rest = &rest[at + "Anchor(".len()..];
        let end = rest.find(')').unwrap();
        let anchor = hirsel_span::Anchor(rest[..end].parse().unwrap());
        let place = sources
            .anchor(anchor)
            .map(|(file, base)| (sources.file(file).unwrap().path.clone(), base));
        out.push_str(&format!("Anchor({place:?})"));
        rest = &rest[end + 1..];
    }
    out + rest
}

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
        let compiler = check(other);
        assert_ne!(compiler, edited);
        let steps = check(other.with_comptime_limits(hirsel_interp::Limits {
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
