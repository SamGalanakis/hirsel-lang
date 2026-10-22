//! Exact image admission remains authoritative for compact declaration rows.
use crate::image_fixture::*;
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_syntax::{DefId, DefPath, ItemPath};
#[path = "../support/canonical_debug.rs"]
mod canonical_debug;

#[test]
fn compact_image_admission_retains_private_views_unreachable_errors_and_positions() {
    let run = || {
        with_compiler_stack(|| {
            let f = Fixture::new();
            let lib = f.0.join("lib.json");
            let source = "pub opaque type Hidden: plain uses {} { value: u32 }\npub fn value()->u32{let h=Hidden(value:7u32);h.value}\n";
            f.write("lib.hir", source);
            let manifest = serde_json::json!({"schema":1,"package":{"name":"lib","version":"1"},"sources":[{"path":"lib.hir","module":["lib"]}],"dependencies":[]});
            std::fs::write(&lib, manifest.to_string()).unwrap();
            let image_path = f.0.join("lib.image");
            COMPILER.image_package(&lib, &image_path).unwrap();
            let stored = StdImage::open(&image_path).unwrap();
            let hidden = DefId::intern(&DefPath(ItemPath {
                package: package("lib"),
                modules: vec!["lib".into()],
                name: "Hidden".into(),
            }));
            let valid = "pub fn main()->u32{lib.value()}\n";
            let client = f.program("client", valid, &lib, "lib");
            let mut original = load(&client, &StdImages::of(vec![stored.clone()]));
            assert!(original.engine.imaged(&package("lib")));
            let before = original.engine.signature_snapshot();
            let old_public = before.signature(hidden).unwrap().unwrap().fingerprint;
            let old_private = before.representation(hidden).unwrap().unwrap().1;
            let mut transcript = String::new();
            for (library, root, adopted, rejected) in [
                (source.to_owned(), valid.to_owned(), true, false),
                (
                    source.to_owned(),
                    format!(
                        "\n\n{valid}fn unreachable()->u8{{lib.absent()}}\nfn also_bad()->u8{{true}}\n"
                    ),
                    true,
                    true,
                ),
                (
                    source.replace("u32", "u64").replace("7u32", "7u64"),
                    valid.replace("u32", "u64"),
                    false,
                    false,
                ),
                (
                    format!("// moved\n{source}"),
                    valid.to_owned(),
                    false,
                    false,
                ),
                (
                    format!("{source}fn unreachable()->u8{{true}}\n"),
                    valid.to_owned(),
                    false,
                    true,
                ),
            ] {
                f.write("lib.hir", &library);
                f.program("client", &root, &lib, "lib");
                for jobs in [1, 2, 8] {
                    let config = CompileConfig {
                        jobs: hirsel_intern::jobs::Jobs::new(jobs),
                        ..CompileConfig::for_memory_cap(None)
                    };
                    let mut cached = COMPILER
                        .clone()
                        .with_config(config.clone())
                        .load_program_source_with_images(
                            &client,
                            None,
                            &StdImages::of(vec![stored.clone()]),
                        )
                        .unwrap();
                    assert_eq!(cached.engine.imaged(&package("lib")), adopted);
                    let mut fresh = COMPILER
                        .clone()
                        .with_config(config)
                        .load_program_source_with_images(&client, None, &StdImages::none())
                        .unwrap();
                    let a = cached.check(LoweringPolicy::Comptime).unwrap();
                    let b = fresh.check(LoweringPolicy::Comptime).unwrap();
                    // Include diagnostic phase/order and resolved positions, not only rule IDs.
                    let actual = a
                        .diagnostics
                        .iter()
                        .map(|d| d.render_human(&a.sources))
                        .collect::<Vec<_>>();
                    let expected = b
                        .diagnostics
                        .iter()
                        .map(|d| d.render_human(&b.sources))
                        .collect::<Vec<_>>();
                    assert_eq!(a.completion, b.completion);
                    assert_eq!(actual, expected);
                    assert_eq!(!actual.is_empty(), rejected);
                    if !rejected {
                        for path in [
                            DefPath(ItemPath {
                                package: package("client"),
                                modules: vec![],
                                name: "main".into(),
                            }),
                            DefPath(ItemPath {
                                package: package("lib"),
                                modules: vec!["lib".into()],
                                name: "value".into(),
                            }),
                        ] {
                            let actual = cached.engine.lower_to_tir(&path).unwrap();
                            let expected = fresh.engine.lower_to_tir(&path).unwrap();
                            let mut actual_reads = vec![];
                            let mut expected_reads = vec![];
                            let actual_views = cached.engine.signature_snapshot();
                            let expected_views = fresh.engine.signature_snapshot();
                            assert!(
                                hirsel_validate::validate(
                                    &actual.value,
                                    cached.engine.types(),
                                    actual_views.validation(&path.0.package),
                                    &mut actual_reads
                                )
                                .violations
                                .is_empty()
                            );
                            assert!(
                                hirsel_validate::validate(
                                    &expected.value,
                                    fresh.engine.types(),
                                    expected_views.validation(&path.0.package),
                                    &mut expected_reads
                                )
                                .violations
                                .is_empty()
                            );
                            assert!(!actual_reads.is_empty());
                            assert_eq!(actual_reads, expected_reads);
                            let render =
                                |engine: &QueryEngine, definition: &hirsel_tir::Definition| {
                                    canonical_debug::placed(
                                        &engine.sources,
                                        canonical_debug::canonical_ids(
                                            engine.types(),
                                            || serde_json::to_vec(definition).unwrap(),
                                            format!("{definition:?}"),
                                        ),
                                    )
                                };
                            let actual_tir = render(&cached.engine, &actual.value);
                            assert_eq!(actual_tir, render(&fresh.engine, &expected.value));
                            transcript.push_str(
                                &format!("{actual_reads:?} {actual_tir}\n")
                                    .replace(&f.0.display().to_string(), "<fixture>"),
                            );
                        }
                    }
                    if library.contains("value: u64") {
                        let current = cached.engine.signature_snapshot();
                        assert_eq!(
                            current.signature(hidden).unwrap().unwrap().fingerprint,
                            old_public
                        );
                        assert_ne!(
                            current.representation(hidden).unwrap().unwrap().1,
                            old_private
                        );
                    }
                    transcript.push_str(
                        &format!("{adopted} {rejected} {actual:?}\n")
                            .replace(&f.0.display().to_string(), "<fixture>"),
                    );
                }
            }
            // The same source with changed manifest placement must reject the old image.
            f.write("lib.hir", source);
            let mut moved = manifest;
            moved["sources"][0]["module"] = serde_json::json!(["changed"]);
            std::fs::write(&lib, moved.to_string()).unwrap();
            let changed = load(&client, &StdImages::of(vec![stored]));
            assert!(!changed.engine.imaged(&package("lib")));
            transcript
        })
    };
    if let Some(outputs) = hirsel_intern::under_salts(
        "image_restore_17::compact_image_admission_retains_private_views_unreachable_errors_and_positions",
        &[1, 2, 0x5eed],
        run,
    ) {
        assert!(outputs.iter().all(|o| o == &outputs[0]));
        println!(
            "image-restore-proof: {:?}",
            hirsel_syntax::ContentHash::of_bytes(outputs[0].as_bytes())
        );
    }
}
