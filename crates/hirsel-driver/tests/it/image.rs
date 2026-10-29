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

// This constituent covers the selected std client closure and native import.
// The original all-definition equality test remains separate and unproved.

// Bounded per-definition coverage; the original complete identity above remains.
fn ordinary_std_definition_products_partition(partition: usize) {
    with_compiler_stack(|| {
        let f = Fixture::new();
        let products = f.0.join("std-products");
        let compiler = COMPILER.clone().with_config(CompileConfig {
            jobs: hirsel_intern::jobs::Jobs::new(1),
            ..CompileConfig::for_memory_cap(None)
        });
        compiler
            .precompute_package(&std_manifest(), &products)
            .unwrap();
        let shared = compiler.clone().with_config(CompileConfig {
            store: Arc::new(store::DirStore::new(products)),
            jobs: hirsel_intern::jobs::Jobs::new(1),
            ..CompileConfig::for_memory_cap(None)
        });
        // `aaa` places std after the program's own sources, exercising FileId rebasing.
        let client = f.program("aaa", STD_PROGRAM, &std_manifest(), "std");
        let mut imaged = shared.load_program_source(&client, None).unwrap();
        let definitions: Vec<_> = imaged
            .engine
            .indexed_definitions()
            .into_iter()
            .filter(|path| path.0.package == package("std"))
            .map(|path| DefId::intern(&path))
            .collect();
        assert!(definitions.len() > 100);
        let mut reference = compiler.load_program_source(&client, None).unwrap();
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
        for (index, path) in definitions.iter().copied().map(DefId::path).enumerate() {
            if index % 2 != partition {
                continue;
            }
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
        // The two disjoint partitions each prove >50 successful Lower
        // comparisons, so their explicit union exceeds the original100.
        assert!(lowered > 50, "partition={partition} lowered={lowered}");
        for path in definitions.iter().copied().map(DefId::path) {
            for phase in [QueryKind::Check, QueryKind::Modes] {
                assert_eq!(
                    imaged.engine.recompute_count(&path, phase),
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

fn library_products(f: &Fixture, source: &str) -> Result<PathBuf, ToolError> {
    f.library(source);
    products(f, "lib")
}

fn products(f: &Fixture, name: &str) -> Result<PathBuf, ToolError> {
    let root = f.0.join(format!("{name}.products"));
    COMPILER.precompute_package(&f.0.join(format!("{name}.json")), &root)?;
    Ok(root)
}

#[test]
fn ordinary_products_reject_changed_compiler_and_sources() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        let products = library_products(&f, LIBRARY).unwrap();
        let client = f.program("client", CLIENT, &f.0.join("lib.json"), "lib");
        let path = hirsel_syntax::DefPath(hirsel_syntax::ItemPath {
            package: package("lib"),
            modules: vec!["lib".into()],
            name: "value".into(),
        });
        let mut hit = load(&client, Some(&products));
        hit.check_for_execution().unwrap();
        assert_eq!(hit.engine.recompute_count(&path, QueryKind::Check), 0);
        let mut other = Compiler::new(FrontPhases::shared, [7; 32])
            .with_config(CompileConfig {
                store: Arc::new(
                    store::DirStore::new(f.0.join("other-reader"))
                        .with_read_only_root(products.clone()),
                ),
                ..CompileConfig::for_memory_cap(None)
            })
            .load_program_source(&client, None)
            .unwrap();
        other.check_for_execution().unwrap();
        assert!(other.engine.recompute_count(&path, QueryKind::Check) > 0);
        f.library(&LIBRARY.replace("x + 1", "x + 2"));
        let mut edited = load(&client, Some(&products));
        let mut fresh = load(&client, None);
        edited.check_for_execution().unwrap();
        fresh.check_for_execution().unwrap();
        assert!(edited.engine.recompute_count(&path, QueryKind::Check) > 0);
        assert_eq!(
            edited
                .engine
                .semantic_program_revision(&edited.entry.clone())
                .unwrap(),
            fresh
                .engine
                .semantic_program_revision(&fresh.entry.clone())
                .unwrap()
        );
    });
}

#[test]
fn ordinary_products_reject_changed_execution_limits() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        let products = library_products(&f, LIBRARY).unwrap();
        let client = f.program("client", CLIENT, &f.0.join("lib.json"), "lib");
        let path = hirsel_syntax::DefPath(hirsel_syntax::ItemPath {
            package: package("lib"),
            modules: vec!["lib".into()],
            name: "value".into(),
        });
        for (budget, depth) in [(999_999, 256), (999_999, 128)] {
            let mut p = COMPILER
                .clone()
                .with_comptime_limits(hirsel_engine::ComptimeLimits {
                    budget,
                    call_depth: depth,
                })
                .with_config(CompileConfig {
                    store: Arc::new(
                        store::DirStore::new(f.0.join(format!("limits-{depth}")))
                            .with_read_only_root(products.clone()),
                    ),
                    ..CompileConfig::for_memory_cap(None)
                })
                .load_program_source(&client, None)
                .unwrap();
            p.check_for_execution().unwrap();
            assert!(p.engine.recompute_count(&path, QueryKind::Check) > 0);
        }
    });
}

#[test]
fn editing_an_imaged_source_in_a_session_ends_its_image() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        let image = library_products(&f, LIBRARY).unwrap();
        let client = f.program("client", CLIENT, &f.0.join("lib.json"), "lib");
        let mut program = load(&client, Some(&image));

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

        // The edited body is checked and run from source.
        let warm = program.check(LoweringPolicy::Reachable).unwrap();
        f.library(&LIBRARY.replace("x + 1", "x + 2"));
        let mut cold = load(&client, None);
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
        let bytes = products(&f, "std").unwrap();
        let images = Some(bytes.as_path());
        let mut warm = load(&client, images);
        warm.check_for_execution().unwrap();

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

        let mut cold = load(&client, None);
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
        let image = library_products(&f, source).unwrap();
        let client = f.program("client", CLIENT, &f.0.join("lib.json"), "lib");
        let mut cached = load(&client, Some(&image));
        let mut fresh = load(&client, None);
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
        for path in cached
            .engine
            .indexed_definitions()
            .into_iter()
            .filter(|p| p.0.package == package("lib"))
        {
            for phase in [QueryKind::Check, QueryKind::Modes] {
                assert_eq!(
                    cached.engine.recompute_count(&path, phase),
                    0,
                    "{path}: {phase:?}"
                );
                assert!(
                    fresh.engine.recompute_count(&path, phase) > 0,
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
        let rejected = library_products(&f, "pub fn value(x: u64) -> u64 {\n  x + true\n}\n");
        assert!(rejected.is_err());
    });
}

/// Exercise the compiler's products, including source anchors, core rows and
/// query read sets, rather than only the image container's synthetic values.
#[test]
fn ordinary_product_identities_ignore_id_salts() {
    let run = || {
        with_compiler_stack(|| {
            let f = Fixture::new();
            let products = library_products(&f, LIBRARY).unwrap();
            let client = f.program("client", CLIENT, &f.0.join("lib.json"), "lib");
            let mut p = load(&client, Some(&products));
            format!(
                "{:?}",
                p.engine
                    .semantic_program_revision(&p.entry.clone())
                    .unwrap()
            )
        })
    };
    if let Some(runs) = hirsel_intern::under_salts(
        "image::ordinary_product_identities_ignore_id_salts",
        &[1, 2, 0x5eed],
        run,
    ) {
        assert!(runs.iter().all(|run| run == &runs[0]));
    }
}

/// Loading policy must preserve diagnostic selection and placement, including
/// rejection at a call into an imaged signature and its accepted control.
#[test]
fn image_diagnostics_equal_source_for_rejected_and_accepted_clients() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        let image = library_products(&f, LIBRARY).unwrap();
        let images = Some(image.as_path());
        for (source, rejected) in [
            (CLIENT.replace("lib.value(1)", "lib.value(true)"), true),
            (CLIENT.into(), false),
        ] {
            let client = f.program("client", &source, &f.0.join("lib.json"), "lib");
            let check = |images: Option<&std::path::Path>| {
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
            assert_eq!(check(images), check(None));
        }
    });
}

/// Published summaries come from the image without retaining or rebuilding a
/// checked/mode body, and match a source compilation with another TypeInterner.
#[test]
fn imaged_summary_equals_source_without_body_recomputation() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        let image = library_products(&f, LIBRARY).unwrap();
        let library = f.0.join("lib.json");
        let client = f.program("zzz", CLIENT, &library, "lib");
        let mut cached = load(&client, Some(&image));
        let mut fresh = load(&client, None);
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

/// Only a shared local verifier cache reuses successful view checks. An image
/// carries ordinary data and cannot convey acceptance to a separate cache.
#[test]
fn ordinary_snapshot_reuses_only_local_signature_verdicts() {
    let run = || {
        with_compiler_stack(|| {
            use hirsel_intern::work::{self, Counter};
            let f = Fixture::new();
            let products = library_products(&f, LIBRARY).unwrap();
            let client = f.program("zzz", CLIENT, &f.0.join("lib.json"), "lib");
            let config = CompileConfig {
                store: Arc::new(
                    store::DirStore::new(f.0.join("shared")).with_read_only_root(products.clone()),
                ),
                ..CompileConfig::for_memory_cap(None)
            };
            let compiler = COMPILER.clone().with_config(config);
            let mut producer = compiler.load_program_source(&client, None).unwrap();
            producer.engine.signature_snapshot();
            let mut local = compiler.load_program_source(&client, None).unwrap();
            let before = work::read(Counter::ValidateSignatureRows);
            local.engine.signature_snapshot();
            assert_eq!(work::read(Counter::ValidateSignatureRows) - before, 0);
            let mut independent = load(&client, Some(&products));
            let before = work::read(Counter::ValidateSignatureRows);
            independent.engine.signature_snapshot();
            assert!(
                work::read(Counter::ValidateSignatureRows) - before > 1,
                "ordinary bytes cannot convey verifier authority"
            );
            "local".to_owned()
        })
    };
    if let Some(outputs) = hirsel_intern::under_salts(
        "image::ordinary_snapshot_reuses_only_local_signature_verdicts",
        &[1],
        run,
    ) {
        assert_eq!(outputs, ["local"]);
    }
}

/// Asking directly for syntax must materialize its image source. A content-key
/// read alone is syntax-free, so explicit parse/resolve demands own this step.
#[test]
fn explicit_image_syntax_requests_materialize_the_source() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        let image = library_products(&f, LIBRARY).unwrap();
        let library = f.0.join("lib.json");
        let client = f.program("client", CLIENT, &library, "lib");
        let path = hirsel_syntax::DefPath(hirsel_syntax::ItemPath {
            package: package("lib"),
            modules: vec!["lib".into()],
            name: "value".into(),
        });
        for resolve in [false, true] {
            let mut cached = load(&client, Some(&image));

            if resolve {
                cached.engine.resolve_definition(&path).unwrap();
            } else {
                let syntax = cached.engine.parse_definition(&path).unwrap();
                assert_eq!(syntax.value.item.definition, path);
            }
            // A syntax demand does not retire its still-current image.
        }
    });
}

/// Imported producer receipts are not V authority (soundness repair 9d683676).
/// Each consumer independently validates once, then reuses its local evidence,
/// whether or not its FirstHeap selection matches the producer's.
#[test]
fn imaged_lowering_reuses_validated_evidence() {
    let run = || {
        with_compiler_stack(|| {
            use hirsel_intern::timings::{self, Count};
            let f = Fixture::new();
            let image = library_products(&f, LIBRARY).unwrap();
            let library = f.0.join("lib.json");
            let path = hirsel_syntax::DefPath(hirsel_syntax::ItemPath {
                package: package("lib"),
                modules: vec!["lib".into()],
                name: "value".into(),
            });
            // Both roots use the same source. Only `aaa` sorts before core's
            // heap-role declaration, changing V's FirstHeap read.
            for name in ["zzz", "aaa"] {
                let client = f.program(name, CLIENT, &library, "lib");
                let mut program = load(&client, Some(&image));

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
                    1,
                    "{name}: imported TIR needs independent V"
                );
                let second = program.engine.lower_to_tir(&path).unwrap();
                assert!(Arc::ptr_eq(&first.value, &second.value));
                assert_eq!(
                    count() - before,
                    1,
                    "repeated demand must reuse its locally established verdict"
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
        let _imager = f.program("imager", main, &library, "lib");
        let bytes = products(&f, "lib").unwrap();
        let stored = bytes;
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
        for images in [None, Some(stored.as_path())] {
            let mut program = load(&manifest, images);
            let result = program.engine.materialize_constant(&root);
            assert!(
                matches!(result, Err(QueryError::Operational(_))),
                "products={} phase={:?} result={result:?}",
                images.is_some(),
                program.engine.last_phase
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
        let mut program = load(&manifest, None);
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
        let bytes = products(&f, "lib").unwrap();
        let stored = bytes;
        let client = f.program("client", main, &library, "lib");
        let mut imaged = load(&client, Some(&stored));

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
        let _imager = f.program("imager", main, &library, "lib");
        let path = DefPath(hirsel_syntax::ItemPath {
            package: package("lib"),
            modules: vec!["later".into()],
            name: "VALUE".into(),
        });
        let mut producer = load(&_imager, None);
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
        let bytes = products(&f, "lib").unwrap();
        let stored = bytes;
        let client = f.program("client", main, &library, "lib");
        let mut program = load(&client, Some(&stored));
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
        let image = library_products(
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
            // This positive receipt-reuse matrix shares the producer's local
            // verifier namespace. Separate read-only roots carry no authority.
            let mut cached = COMPILER
                .clone()
                .with_config(CompileConfig {
                    store: Arc::new(store::DirStore::new(image.clone())),
                    jobs: hirsel_intern::jobs::Jobs::new(jobs),
                    ..CompileConfig::for_memory_cap(None)
                })
                .load_program_source(&client, None)
                .unwrap();
            let product_store = cached.engine.product_store().unwrap();
            cached.engine = cached.engine.with_config(CompileConfig {
                store: product_store,
                hold_bytes: budget,
                ..CompileConfig::for_memory_cap(None)
            });
            let mut fresh = load(&client, None);

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
                assert_eq!(
                    outcome.phase,
                    Some(QueryKind::Lower),
                    "budget={budget} jobs={jobs} fallback={fallback} result={:?}",
                    outcome.result
                );
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
        let mut fresh = load(&client, None);
        let bytes = products(&f, "lib").unwrap();
        let images = Some(bytes.as_path());
        let mut imaged = load(&client, images);

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

/// Scalar private admission remains memory-only proof metadata. It cannot
/// become a semantic dependency on the imager's complete V universe.
#[test]
fn scalar_private_receipts_stay_outside_package_dependency_reads() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        library_products(
            &f,
            "pub const EARLY:u32=comptime 7u32\npub fn value(x:u64)->u64{let _=EARLY;x+1u64}\n",
        )
        .unwrap();
    });
}
