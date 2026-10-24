//! PROG-1/LANG-4 expectations are handwritten from explicit package graphs.
//! Source provenance and bounded multi-file controls adapt rust-analyzer's
//! unresolved_module.rs:10-31,63-84 at 3131751eaf1c3b5da6e618acdc2148ad7db65bc7
//! (MIT OR Apache-2.0). No discovery, filename aliases or CreateFile fixes.
use crate::image_fixture::{Fixture, load};
use hirsel_driver::session::Session;
use hirsel_engine::{CheckCompletion, CheckOutput, LoweringPolicy, Verify, with_compiler_stack};
use serde_json::{Value, json};
use std::path::PathBuf;

const VALUE: &str = "pub fn value() -> u64 { 1u64 }\n";
const BAD: &str = "// λ before the byte span\npub fn main() -> u64 { odd_name.value() }\n";
const MARKER: &str = "help: selected source mappings:";

fn manifest(
    f: &Fixture,
    name: &str,
    version: &str,
    sources: &[(&str, &[&str])],
    dependencies: Vec<Value>,
) -> PathBuf {
    let path = f.0.join(format!("{name}-{version}.json"));
    std::fs::write(
        &path,
        json!({"schema": 1, "package": {"name": name, "version": version},
            "sources": sources.iter().map(|(path, module)| json!({"path": path, "module": module})).collect::<Vec<_>>(),
            "dependencies": dependencies, "entry": {"module": [], "name": "main"}}).to_string(),
    ).unwrap();
    path
}

fn dependency(path: PathBuf, name: &str, version: &str, expose: Value) -> Value {
    json!({"manifest": path, "package": {"name": name, "version": version}, "expose": expose})
}

fn library(f: &Fixture, name: &str, version: &str, module: &[&str], text: &str) -> Value {
    let directory = format!("{name}-{version}");
    std::fs::create_dir_all(f.0.join(&directory)).unwrap();
    let source = format!("{directory}/odd_name.hir");
    f.write(&source, text);
    let path = manifest(f, name, version, &[(&source, module)], vec![]);
    dependency(path, name, version, json!([]))
}

fn check(path: &std::path::Path) -> CheckOutput {
    let mut program = load(path, None);
    let output = program.check(LoweringPolicy::Comptime).unwrap();
    assert_eq!(output.completion, CheckCompletion::Complete);
    output
}

fn encoded(output: &CheckOutput) -> Vec<Value> {
    output
        .diagnostics
        .iter()
        .map(|d| d.to_json(&output.sources))
        .collect()
}

/// Every failing vector retains human/JSON parity and the original primary
/// lookup span, including bytes after non-ASCII text. Extra errors are failures.
fn guidance(output: &CheckOutput, primary: &str) -> Vec<String> {
    assert_eq!(output.diagnostics.len(), 1, "{:?}", output.diagnostics);
    let d = &output.diagnostics[0];
    assert_eq!(d.rule.as_str(), "LANG-4", "{d:?}");
    assert_eq!(output.sources.slice(d.primary), Some(primary));
    let at = output.sources.resolve(d.primary);
    let source = output.sources.file(at.file).unwrap().text();
    assert_eq!(at.start as usize, source.find(primary).unwrap());
    assert_eq!(
        at.end as usize,
        source.find(primary).unwrap() + primary.len()
    );
    let human = d.render_human(&output.sources);
    let json = d.to_json(&output.sources);
    assert_eq!(json["primary"]["start"], at.start);
    assert_eq!(json["primary"]["end"], at.end);
    for note in json["notes"].as_array().unwrap() {
        assert!(human.contains(note.as_str().unwrap()), "{human}\n{json}");
    }
    // This enrichment supplies explanations, never automatic edits.
    assert!(d.fix.is_none(), "{json}");
    eprintln!("{human}{json}");
    d.notes
        .iter()
        .filter(|n| n.starts_with(MARKER))
        .cloned()
        .collect()
}

fn expected(filename: &str, module: &str, repair: &str) -> String {
    format!(
        "help: selected source mappings: `{filename}` is registered as module `{module}` (use `{repair}`); filenames do not name modules"
    )
}

#[test]
fn manifest_module_guidance_48_exact_mapping_and_source_v_twin() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        f.write("main.hir", BAD);
        f.write("odd_name.hir", VALUE);
        let path = manifest(
            &f,
            "app",
            "1",
            &[("main.hir", &[]), ("odd_name.hir", &["api"])],
            vec![],
        );
        let mut program = load(&path, None);
        let first = program.check(LoweringPolicy::Comptime).unwrap();
        assert_eq!(
            guidance(&first, "odd_name.value"),
            [expected("odd_name.hir", "app@1/api", "app@1/api.value")]
        );
        // A second assembly must not accumulate help into semantic memos.
        assert_eq!(
            encoded(&first),
            encoded(&program.check(LoweringPolicy::Comptime).unwrap())
        );
        let names: Vec<_> = program
            .engine
            .indexed_definitions()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(names.contains(&"app@1/api/value".into()));
        assert!(!names.iter().any(|p| p.contains("odd_name")));
        for repair in ["api.value", "app@1/api.value"] {
            f.write("main.hir", &BAD.replace("odd_name.value", repair));
            let mut twin = load(&path, None);
            assert!(
                twin.check(LoweringPolicy::Reachable)
                    .unwrap()
                    .diagnostics
                    .is_empty()
            );
            let entry = twin.entry.clone();
            let text = twin.engine.export_tir(&entry).unwrap();
            let exported = twin.engine.import_tir(&text).unwrap();
            assert!(exported.skipped.is_empty(), "{:?}", exported.skipped);
            eprintln!("accepted source and V twin: {repair}");
        }
        std::fs::rename(f.0.join("odd_name.hir"), f.0.join("renamed.hir")).unwrap();
        manifest(
            &f,
            "app",
            "1",
            &[("main.hir", &[]), ("renamed.hir", &["api"])],
            vec![],
        );
        let mut renamed = load(&path, None);
        assert!(
            renamed
                .check(LoweringPolicy::Reachable)
                .unwrap()
                .diagnostics
                .is_empty()
        );
        let renamed_names: Vec<_> = renamed
            .engine
            .indexed_definitions()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(names, renamed_names);
    });
}

#[test]
fn manifest_module_guidance_48_renamed_nested_multifile_and_absent_controls() {
    with_compiler_stack(|| {
        for (filename, modules, spelling, member, want) in [
            (
                "odd_name.hir",
                vec!["api"],
                "odd_name.value",
                VALUE,
                Some(("app@1/api", "app@1/api.value")),
            ),
            ("renamed.hir", vec!["api"], "odd_name.value", VALUE, None),
            (
                "renamed.hir",
                vec!["api"],
                "renamed.value",
                VALUE,
                Some(("app@1/api", "app@1/api.value")),
            ),
            (
                "odd_name.hir",
                vec!["outer", "api"],
                "odd_name.value",
                VALUE,
                Some(("app@1/outer.api", "app@1/outer.api.value")),
            ),
            (
                "odd_name.hir",
                vec!["outer", "api"],
                "outer.odd_name.value",
                VALUE,
                Some(("app@1/outer.api", "app@1/outer.api.value")),
            ),
            (
                "odd_name.hir",
                vec![],
                "odd_name.value",
                VALUE,
                Some(("app@1/", "app@1/value")),
            ),
            ("odd_name.hir", vec!["api"], "odd_name.absent", VALUE, None),
            ("odd_name.hir", vec!["api"], "odd_nam.value", VALUE, None),
            // The module spans two source files; membership belongs to the
            // module, rather than being guessed from this file's body.
            (
                "odd_name.hir",
                vec!["api"],
                "odd_name.value",
                "pub fn other() {}\n",
                Some(("app@1/api", "app@1/api.value")),
            ),
        ] {
            let f = Fixture::new();
            f.write("main.hir", &BAD.replace("odd_name.value", spelling));
            f.write(filename, member);
            let mut sources = vec![("main.hir", &[][..]), (filename, modules.as_slice())];
            if member != VALUE {
                std::fs::create_dir_all(f.0.join("other")).unwrap();
                f.write("other/odd_name.hir", VALUE);
                sources.push(("other/odd_name.hir", modules.as_slice()));
            }
            let path = manifest(&f, "app", "1", &sources, vec![]);
            let notes = guidance(&check(&path), spelling);
            let wanted: Vec<_> = want
                .map(|(m, r)| expected(filename, m, r))
                .into_iter()
                .collect();
            assert_eq!(notes, wanted);
        }
        let f = Fixture::new();
        f.write("main.hir", BAD);
        f.write("odd_name.hir", VALUE); // physically present, never selected
        let path = manifest(&f, "app", "1", &[("main.hir", &[])], vec![]);
        assert!(guidance(&check(&path), "odd_name.value").is_empty());
    });
}

#[test]
fn manifest_module_guidance_48_packages_aliases_precedence_and_versions() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        f.write("main.hir", BAD);
        let left = library(&f, "left", "1", &["api"], VALUE);
        let right = library(&f, "right", "2", &["api"], VALUE);
        let path = manifest(
            &f,
            "app",
            "1",
            &[("main.hir", &[])],
            vec![right.clone(), left.clone()],
        );
        let output = check(&path);
        let notes = guidance(&output, "odd_name.value");
        assert_eq!(
            notes,
            [
                "help: selected source mappings: `odd_name.hir` is registered as module `left@1/api` (use `left@1/api.value`), `odd_name.hir` is registered as module `right@2/api` (use `right@2/api.value`); filenames do not name modules"
            ]
        );
        manifest(
            &f,
            "app",
            "1",
            &[("main.hir", &[])],
            vec![left.clone(), right.clone()],
        );
        assert_eq!(notes, guidance(&check(&path), "odd_name.value"));
        for repair in ["left@1/api.value", "right@2/api.value"] {
            f.write("main.hir", &BAD.replace("odd_name.value", repair));
            assert!(check(&path).diagnostics.is_empty());
        }
        // An actually ambiguous declared head keeps its original diagnostic.
        f.write("main.hir", &BAD.replace("odd_name.value", "api.value"));
        let ambiguous = check(&path);
        assert!(
            ambiguous.diagnostics[0]
                .message
                .starts_with("ambiguous module head")
        );
        assert!(guidance(&ambiguous, "api.value").is_empty());
        // A versioned erroneous path restricts the mapping to that package.
        f.write(
            "main.hir",
            &BAD.replace("odd_name.value", "right@2/odd_name.value"),
        );
        assert_eq!(
            guidance(&check(&path), "right@2/odd_name.value"),
            [expected("odd_name.hir", "right@2/api", "right@2/api.value")]
        );
        // Explicit expose aliases remain legal, but create no filename alias.
        let mut aliased = left.clone();
        aliased["expose"] = json!([{"head": "friendly", "module": ["api"]}]);
        manifest(&f, "app", "1", &[("main.hir", &[])], vec![aliased, right]);
        f.write("main.hir", &BAD.replace("odd_name.value", "friendly.value"));
        assert!(check(&path).diagnostics.is_empty());
        f.write("main.hir", BAD);
        assert_eq!(notes, guidance(&check(&path), "odd_name.value"));
        // Same-package api wins over both dependency api heads. Its source
        // has a different stem, so the filename mistake still has two repairs.
        f.write("own.hir", "fn value() -> u64 { 2u64 }\n");
        manifest(
            &f,
            "app",
            "1",
            &[("main.hir", &[]), ("own.hir", &["api"])],
            vec![left],
        );
        f.write("main.hir", &BAD.replace("odd_name.value", "api.value"));
        assert!(check(&path).diagnostics.is_empty());
        f.write("main.hir", BAD);
        assert_eq!(
            guidance(&check(&path), "odd_name.value"),
            [expected("odd_name.hir", "left@1/api", "left@1/api.value")]
        );
    });
}

#[test]
fn manifest_module_guidance_48_visibility_and_bounded_candidates() {
    with_compiler_stack(|| {
        for (modules, text, spelling) in [
            (
                vec!["api"],
                "fn value() -> u64 { 1u64 }\n",
                "odd_name.value",
            ),
            (vec!["internal", "api"], VALUE, "odd_name.value"),
            (vec!["api", "internal", "nested"], VALUE, "odd_name.value"),
            (vec!["api"], VALUE, "odd_name.absent"),
            (
                vec!["api"],
                "pub opaque type Secret: data uses {} { value: u64 }\n",
                "odd_name.value",
            ),
            (
                vec!["api"],
                "pub opaque type Secret: data uses {} { value: u64 }\n",
                "odd_name.Secret",
            ),
        ] {
            let f = Fixture::new();
            let source = if spelling.ends_with("Secret") {
                "pub fn main(x: odd_name.Secret) {}".to_string()
            } else {
                BAD.replace("odd_name.value", spelling)
            };
            f.write("main.hir", &source);
            let dep = library(&f, "dep", "1", &modules, text);
            let path = manifest(&f, "app", "1", &[("main.hir", &[])], vec![dep]);
            let notes = guidance(&check(&path), spelling);
            assert!(notes.is_empty(), "{notes:?}");
            if spelling.ends_with("Secret") {
                f.write("main.hir", "pub fn main(x: dep@1/api.Secret) {}");
                assert!(check(&path).diagnostics.is_empty());
                f.write(
                    "main.hir",
                    "pub fn main() { let _ = odd_name.Secret(value: 1u64) }",
                );
                assert!(guidance(&check(&path), "odd_name.Secret").is_empty());
                f.write(
                    "main.hir",
                    "pub fn main() { let _ = dep@1/api.Secret(value: 1u64) }",
                );
                let denied = check(&path);
                assert!(
                    denied
                        .diagnostics
                        .iter()
                        .any(|d| d.rule.as_str() == "PROG-2")
                );
                assert!(
                    denied
                        .diagnostics
                        .iter()
                        .all(|d| d.notes.iter().all(|n| !n.starts_with(MARKER)))
                );
            }
        }
        let f = Fixture::new();
        f.write("main.hir", BAD);
        let dependencies = (0..6)
            .rev()
            .map(|i| library(&f, &format!("dep_{i}"), "1", &["api"], VALUE))
            .collect();
        let path = manifest(&f, "app", "1", &[("main.hir", &[])], dependencies);
        let notes = guidance(&check(&path), "odd_name.value");
        assert_eq!(notes.len(), 1);
        for i in 0..4 {
            assert!(notes[0].contains(&format!("dep_{i}@1/api.value")));
        }
        for i in 4..6 {
            assert!(!notes[0].contains(&format!("dep_{i}@1")));
        }
        assert!(notes[0].contains("and other visible candidates"));
    });
}

#[test]
fn manifest_module_guidance_48_zonetable_selected_std_mapping() {
    with_compiler_stack(|| {
        // The original incident uses these actual selected std sources, not
        // a special case for a known std filename in the helper.
        let std = crate::image_fixture::std_manifest();
        let metadata: Value = serde_json::from_slice(&std::fs::read(&std).unwrap()).unwrap();
        assert!(
            metadata["sources"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s["path"] == "time_format.hir" && s["module"] == json!(["time"]))
        );
        let f = Fixture::new();
        f.write("main.hir", "pub fn main(x: time_format.FormatError) {}\n");
        let path = manifest(
            &f,
            "app",
            "1",
            &[("main.hir", &[])],
            vec![dependency(std, "std", "1", json!([]))],
        );
        assert_eq!(
            guidance(&check(&path), "time_format.FormatError"),
            [expected(
                "time_format.hir",
                "std@1/time",
                "std@1/time.FormatError"
            )]
        );
        for member in ["broken_down", "format"] {
            let spelling = format!("time_format.{member}");
            f.write(
                "main.hir",
                &format!("pub fn main() {{ let _ = {spelling} }}\n"),
            );
            assert_eq!(
                guidance(&check(&path), &spelling),
                [expected(
                    "time_format.hir",
                    "std@1/time",
                    &format!("std@1/time.{member}")
                )]
            );
        }
        f.write("main.hir", "pub fn main(x: std@1/time.FormatError) {}\n");
        assert!(check(&path).diagnostics.is_empty());
    });
}

#[test]
fn manifest_module_guidance_48_resident_manifest_remap_and_file_rename() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        f.write("main.hir", BAD);
        f.write("odd_name.hir", VALUE);
        let path = manifest(
            &f,
            "app",
            "1",
            &[("main.hir", &[]), ("odd_name.hir", &["api"])],
            vec![],
        );
        let mut session = Session::open(&path, Verify::On);
        let first = session.check().unwrap();
        assert_eq!(
            guidance(&first, "odd_name.value"),
            [expected("odd_name.hir", "app@1/api", "app@1/api.value")]
        );
        manifest(
            &f,
            "app",
            "1",
            &[("main.hir", &[]), ("odd_name.hir", &["new_api"])],
            vec![],
        );
        let remapped = session.check().unwrap();
        assert_eq!(encoded(&remapped), encoded(&check(&path)));
        assert_eq!(
            guidance(&remapped, "odd_name.value"),
            [expected(
                "odd_name.hir",
                "app@1/new_api",
                "app@1/new_api.value"
            )]
        );
        assert_eq!(
            first.diagnostics[0].message,
            remapped.diagnostics[0].message
        );
        assert_eq!(
            first.sources.resolve(first.diagnostics[0].primary).start,
            remapped
                .sources
                .resolve(remapped.diagnostics[0].primary)
                .start
        );
        std::fs::rename(f.0.join("odd_name.hir"), f.0.join("renamed.hir")).unwrap();
        manifest(
            &f,
            "app",
            "1",
            &[("main.hir", &[]), ("renamed.hir", &["new_api"])],
            vec![],
        );
        let renamed = session.check().unwrap();
        assert_eq!(encoded(&renamed), encoded(&check(&path)));
        assert!(guidance(&renamed, "odd_name.value").is_empty());
        assert_eq!(first.diagnostics[0].message, renamed.diagnostics[0].message);
        assert_eq!(session.stats().loads, 3);
        f.write("main.hir", &BAD.replace("odd_name.value", "new_api.value"));
        assert!(session.check().unwrap().diagnostics.is_empty());
        assert!(check(&path).diagnostics.is_empty());
        // In-place source snapshots use the same output seam without reload.
        f.write("main.hir", &BAD.replace("odd_name.value", "renamed.value"));
        let edited = session.check().unwrap();
        assert_eq!(encoded(&edited), encoded(&check(&path)));
        assert_eq!(
            guidance(&edited, "renamed.value"),
            [expected(
                "renamed.hir",
                "app@1/new_api",
                "app@1/new_api.value"
            )]
        );
        assert_eq!(session.stats().loads, 3);
    });
}
