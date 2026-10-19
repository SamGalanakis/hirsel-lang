use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_syntax::{ContentHash, ItemPath, PackageId};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "hirsel-checked-package-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn write(&self, name: &str, source: &str) {
        std::fs::write(self.0.join(name), source).unwrap();
    }
    fn package(&self, name: &str, dependencies: &[&str], root: bool) {
        let deps: Vec<_> = dependencies.iter().map(|d| serde_json::json!({"manifest":format!("{d}.json"),"package":{"name":d,"version":"1"},"expose":[]})).collect();
        let mut manifest = serde_json::json!({"schema":1,"package":{"name":name,"version":"1"},"sources":[{"path":format!("{name}.hir"),"module":[]}],"dependencies":deps});
        if root {
            manifest["entry"] = serde_json::json!({"module":[],"name":"main"});
        }
        self.write(&format!("{name}.json"), &manifest.to_string());
    }
    fn load(&self, root: &str, cache: &PackageCache) -> LoadedProgram {
        load_program_source_with_cache(&self.0.join(format!("{root}.json")), None, cache).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn definition(package: &str, name: &str) -> DefPath {
    DefPath(ItemPath {
        package: PackageId {
            name: package.into(),
            version: "1".into(),
        },
        modules: vec![],
        name: name.into(),
    })
}
fn rendered(output: &CheckOutput) -> (CheckCompletion, Vec<String>, Vec<String>) {
    (
        output.completion,
        output
            .diagnostics
            .iter()
            .map(|d| d.to_json(&output.sources).to_string())
            .collect(),
        output
            .diagnostics
            .iter()
            .map(|d| d.render_human(&output.sources))
            .collect(),
    )
}
fn fresh(f: &Fixture, root: &str) -> CheckOutput {
    f.load(root, &PackageCache::disabled())
        .check(hirsel_engine::LoweringPolicy::Reachable)
        .unwrap()
}

#[test]
fn rejected_clients_reuse_checked_dependencies_and_complete_lowering_later() {
    with_compiler_stack(|| {
        for library in [
            "pub fn value() -> u64 { 1 }",
            "pub fn value() -> u64 { comptime (1u64 / 0u64) }",
        ] {
            let f = Fixture::new();
            f.write("lib.hir", library);
            f.package("lib", &[], false);
            for root in ["aaa", "zzz"] {
                f.write(&format!("{root}.hir"), "pub fn main() { missing }\n");
                f.package(root, &["lib"], true);
            }
            let cache = PackageCache::new();
            let first = f
                .load("aaa", &cache)
                .check(hirsel_engine::LoweringPolicy::Reachable)
                .unwrap();
            assert!(!first.diagnostics.is_empty());
            assert_eq!(rendered(&first), rendered(&fresh(&f, "aaa")));
            let mut second = f.load("zzz", &cache);
            assert_eq!(
                rendered(
                    &second
                        .check(hirsel_engine::LoweringPolicy::Reachable)
                        .unwrap()
                ),
                rendered(&fresh(&f, "zzz"))
            );
            for kind in [
                QueryKind::Signature,
                QueryKind::Resolve,
                QueryKind::Check,
                QueryKind::Modes,
            ] {
                assert_eq!(
                    second
                        .engine
                        .recompute_count(&definition("lib", "value"), kind),
                    0,
                    "{kind:?}"
                );
            }
            // The failed client withheld lowering. Its successful twin must
            // still evaluate at compile time, exposing CT-1, but lowers an
            // unreachable dependency body only for that evaluation.
            f.write("zzz.hir", "pub fn main() {}\n");
            let mut accepted_client = f.load("zzz", &cache);
            let output = accepted_client
                .check(hirsel_engine::LoweringPolicy::Reachable)
                .unwrap();
            assert_eq!(rendered(&output), rendered(&fresh(&f, "zzz")));
            assert_eq!(output.diagnostics.is_empty(), !library.contains("comptime"));
            assert_eq!(
                accepted_client
                    .engine
                    .recompute_count(&definition("lib", "value"), QueryKind::Check),
                0
            );
            assert_eq!(
                accepted_client
                    .engine
                    .recompute_count(&definition("lib", "value"), QueryKind::Lower),
                usize::from(library.contains("comptime"))
            );
            // A client that reaches the body lowers and validates it.
            f.write("zzz.hir", "pub fn main() { let n = lib@1/value() }\n");
            let mut reaching_client = f.load("zzz", &cache);
            let output = reaching_client
                .check(hirsel_engine::LoweringPolicy::Reachable)
                .unwrap();
            assert_eq!(rendered(&output), rendered(&fresh(&f, "zzz")));
            assert_eq!(
                reaching_client
                    .engine
                    .recompute_count(&definition("lib", "value"), QueryKind::Lower),
                usize::from(!library.contains("comptime"))
            );
            // A later rejected client still withholds lowering. A retained
            // lowering diagnostic must not appear before a fresh check would
            // expose it, even after the successful client filled that query.
            assert_eq!(
                rendered(
                    &f.load("aaa", &cache)
                        .check(hirsel_engine::LoweringPolicy::Reachable)
                        .unwrap()
                ),
                rendered(&fresh(&f, "aaa"))
            );
        }
    });
}

#[test]
fn loop_source_operation_signatures_are_reused_across_engines() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        f.write(
            "lib.hir",
            "@loop_source(open, next)\n\
             pub opaque type View: copyable + inert + noalloc_drop uses {} { source: String }\n\
             pub opaque type Cursor: copyable + inert + noalloc_drop uses {} { source: String }\n\
             @noalloc fn open(v: View) -> Cursor in v uses {} { Cursor(source: copy v.source) }\n\
             @noalloc fn next(mut it: Cursor) -> Option[String] in it uses {} { None }\n",
        );
        f.package("lib", &[], false);
        f.write("app.hir", "pub fn main() {}\n");
        f.package("app", &["lib"], true);
        let cache = PackageCache::new();
        let path = definition("lib", "open");
        let mut first = f.load("app", &cache);
        let output = first
            .check(hirsel_engine::LoweringPolicy::Reachable)
            .unwrap();
        assert_eq!(output.completion, CheckCompletion::Complete);
        assert!(output.diagnostics.is_empty(), "{:?}", rendered(&output));
        first.engine.lower_to_tir(&path).unwrap();
        drop(first);
        let mut second = f.load("app", &cache);
        let output = second
            .check(hirsel_engine::LoweringPolicy::Reachable)
            .unwrap();
        assert_eq!(output.completion, CheckCompletion::Complete);
        assert!(output.diagnostics.is_empty(), "{:?}", rendered(&output));
        second.engine.lower_to_tir(&path).unwrap();
        for kind in [
            QueryKind::Signature,
            QueryKind::Resolve,
            QueryKind::Check,
            QueryKind::Modes,
            QueryKind::Lower,
        ] {
            assert_eq!(second.engine.recompute_count(&path, kind), 0, "{kind:?}");
        }
    });
}

#[test]
fn lazy_lowerings_publish_from_engines_without_losing_other_clients_bodies() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        f.write(
            "lib.hir",
            "pub fn first() -> u64 { 1 }\npub fn second() -> u64 { 2 }\n",
        );
        f.package("lib", &[], false);
        for root in ["aaa", "zzz"] {
            f.write(&format!("{root}.hir"), "pub fn main() {}\n");
            f.package(root, &["lib"], true);
        }
        let cache = PackageCache::new();
        // Both clients start before either body is lowered, with different
        // FileIds. Publication must accumulate their concurrent results.
        let a = definition("lib", "first");
        let b = definition("lib", "second");
        let barrier = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            for (root, path) in [("aaa", &a), ("zzz", &b)] {
                let (f, cache, barrier) = (&f, &cache, &barrier);
                scope.spawn(move || {
                    with_compiler_stack(|| {
                        let mut program = f.load(root, cache);
                        assert!(
                            program
                                .check(hirsel_engine::LoweringPolicy::Reachable)
                                .unwrap()
                                .diagnostics
                                .is_empty()
                        );
                        let mut engine = program.engine;
                        assert_eq!(engine.recompute_count(path, QueryKind::Lower), 0);
                        barrier.wait();
                        engine.lower_to_tir(path).unwrap();
                    })
                });
            }
        });
        let mut warm = f.load("zzz", &cache);
        assert!(
            warm.check(hirsel_engine::LoweringPolicy::Reachable)
                .unwrap()
                .diagnostics
                .is_empty()
        );
        for path in [a, b] {
            warm.engine.lower_to_tir(&path).unwrap();
            for kind in [
                QueryKind::Signature,
                QueryKind::Resolve,
                QueryKind::Check,
                QueryKind::Modes,
                QueryKind::Lower,
            ] {
                assert_eq!(
                    warm.engine.recompute_count(&path, kind),
                    0,
                    "{path:?}: {kind:?}"
                );
            }
        }
    });
}

#[test]
fn lazy_publication_ends_on_source_environment_manifest_and_limit_edits() {
    with_compiler_stack(|| {
        for edit in ["source", "environment", "manifest", "limits"] {
            let f = Fixture::new();
            f.write("lib.hir", "pub fn value() -> u64 { 1 }\n");
            f.package("lib", &[], false);
            f.write("app.hir", "pub fn main() {}\n");
            f.package("app", &["lib"], true);
            let cache = PackageCache::new();
            let mut program = f.load("app", &cache);
            assert!(
                program
                    .check(hirsel_engine::LoweringPolicy::Reachable)
                    .unwrap()
                    .diagnostics
                    .is_empty()
            );
            let mut engine = program.engine;
            let path = definition("lib", "value");
            match edit {
                "source" => {
                    let file = engine
                        .sources
                        .add(f.0.join("lib.hir"), "pub fn value() -> u64 { 99 }\n".into())
                        .unwrap();
                    engine
                        .parse_file(
                            file,
                            &FileContext {
                                package: path.0.package.clone(),
                                modules: vec![],
                            },
                        )
                        .unwrap();
                }
                "environment" => engine.environment.compiler_version.push_str("-edited"),
                "manifest" => {
                    engine
                        .adapters
                        .compiler_state
                        .as_ref()
                        .unwrap()
                        .borrow_mut()
                        .manifest_revision = ContentHash::of_bytes(b"edited")
                }
                "limits" => {
                    engine.comptime_limits.steps =
                        engine.comptime_limits.steps.map(|steps| steps + 1)
                }
                _ => unreachable!(),
            }
            engine.lower_to_tir(&path).unwrap();
            let mut pristine = f.load("app", &cache);
            assert!(
                pristine
                    .check(hirsel_engine::LoweringPolicy::Reachable)
                    .unwrap()
                    .diagnostics
                    .is_empty()
            );
            pristine.engine.lower_to_tir(&path).unwrap();
            assert_eq!(
                pristine.engine.recompute_count(&path, QueryKind::Lower),
                1,
                "{edit}"
            );
        }
    });
}

#[test]
fn package_bodies_are_reused_across_engines_threads_and_client_edits() {
    let f = Fixture::new();
    f.write("lib.hir", "pub fn value() -> u64 { 1 }\n");
    f.package("lib", &[], false);
    f.write("aaa.hir", "pub fn main() { let n = lib@1/value() }\n");
    f.package("aaa", &["lib"], true);
    f.write("zzz.hir", "pub fn main() { let m = lib@1/value() }\n");
    f.package("zzz", &["lib"], true);
    let cache = PackageCache::new();
    with_compiler_stack(|| {
        let mut cold = f.load("aaa", &cache);
        assert!(
            cold.check(hirsel_engine::LoweringPolicy::Reachable)
                .unwrap()
                .diagnostics
                .is_empty()
        );
        assert_eq!(
            cold.engine
                .recompute_count(&definition("lib", "value"), QueryKind::Lower),
            1
        );
    });
    // A new compiler thread has no Rc state from the first engine. The root sorts
    // after lib instead of before it, changing every retained package FileId.
    with_compiler_stack(|| {
        let mut warm = f.load("zzz", &cache);
        assert_eq!(
            rendered(
                &warm
                    .check(hirsel_engine::LoweringPolicy::Reachable)
                    .unwrap()
            ),
            rendered(&fresh(&f, "zzz"))
        );
        for kind in [
            QueryKind::Signature,
            QueryKind::Resolve,
            QueryKind::Check,
            QueryKind::Modes,
            QueryKind::Lower,
        ] {
            assert_eq!(
                warm.engine
                    .recompute_count(&definition("lib", "value"), kind),
                0,
                "{kind:?}"
            );
        }
        let mut path = definition("lib", "value");
        let body = warm.engine.lower_to_tir(&path).unwrap().value;
        assert!(body.types.get(body.bodies[0].result).is_some());
        path.0.name = "main".into();
        path.0.package.name = "zzz".into();
        assert!(warm.engine.item_signature(&path).is_ok());
    });
}

#[test]
fn rejected_package_diagnostics_rebase_identically_and_accepted_twin_reuses() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        f.write("lib.hir", "pub fn unused() -> u64 { true }\n");
        f.package("lib", &[], false);
        for root in ["aaa", "zzz"] {
            f.write(&format!("{root}.hir"), "pub fn main() {}\n");
            f.package(root, &["lib"], true);
        }
        let cache = PackageCache::new();
        let before = f
            .load("aaa", &cache)
            .check(hirsel_engine::LoweringPolicy::Reachable)
            .unwrap();
        assert!(!before.diagnostics.is_empty());
        let mut warm = f.load("zzz", &cache);
        assert_eq!(
            rendered(
                &warm
                    .check(hirsel_engine::LoweringPolicy::Reachable)
                    .unwrap()
            ),
            rendered(&fresh(&f, "zzz"))
        );
        assert_eq!(
            warm.engine
                .recompute_count(&definition("lib", "unused"), QueryKind::Check),
            0
        );
        // An edit selects the accepted twin, rather than reviving a rejection by path.
        f.write("lib.hir", "pub fn unused() -> u64 { 1 }\n");
        let accepted = f
            .load("zzz", &cache)
            .check(hirsel_engine::LoweringPolicy::Reachable)
            .unwrap();
        assert!(accepted.diagnostics.is_empty());
        assert_eq!(rendered(&accepted), rendered(&fresh(&f, "zzz")));
        assert_eq!(
            f.load("aaa", &cache)
                .check(hirsel_engine::LoweringPolicy::Reachable)
                .unwrap()
                .diagnostics
                .len(),
            0
        );
    });
}

#[test]
fn dependency_body_and_manifest_edits_invalidate_the_complete_closure() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        f.write("leaf.hir", "pub fn value() -> u64 { 1 }\n");
        f.package("leaf", &[], false);
        f.write(
            "lib.hir",
            "pub fn value() -> u64 { comptime leaf@1/value() }\n",
        );
        f.package("lib", &["leaf"], false);
        f.write("app.hir", "pub fn main() { let n = lib@1/value() }\n");
        f.package("app", &["lib"], true);
        let cache = PackageCache::new();
        assert!(
            f.load("app", &cache)
                .check(hirsel_engine::LoweringPolicy::Reachable)
                .unwrap()
                .diagnostics
                .is_empty()
        );
        let mut warm = f.load("app", &cache);
        assert!(
            warm.check(hirsel_engine::LoweringPolicy::Reachable)
                .unwrap()
                .diagnostics
                .is_empty()
        );
        assert_eq!(
            warm.engine
                .recompute_count(&definition("lib", "value"), QueryKind::Lower),
            0
        );
        f.write("leaf.hir", "pub fn value() -> u64 { 2 }\n");
        let mut edited = f.load("app", &cache);
        assert_eq!(
            rendered(
                &edited
                    .check(hirsel_engine::LoweringPolicy::Reachable)
                    .unwrap()
            ),
            rendered(&fresh(&f, "app"))
        );
        assert_eq!(
            edited
                .engine
                .recompute_count(&definition("lib", "value"), QueryKind::Lower),
            1
        );
        let code = edited
            .engine
            .lower_to_tir(&definition("lib", "value"))
            .unwrap()
            .value;
        assert!(
            code.bodies
                .iter()
                .flat_map(|b| &b.rvalues)
                .any(|r| matches!(
                    r,
                    hirsel_tir::Rvalue::Constant(hirsel_tir::Constant::Integer(2))
                ))
        );
        let old = std::fs::read_to_string(f.0.join("leaf.json")).unwrap();
        f.write("leaf.json", &(old + "\n"));
        let mut manifest = f.load("app", &cache);
        assert!(
            manifest
                .check(hirsel_engine::LoweringPolicy::Reachable)
                .unwrap()
                .diagnostics
                .is_empty()
        );
        assert_eq!(
            manifest
                .engine
                .recompute_count(&definition("lib", "value"), QueryKind::Lower),
            1
        );
    });
}

#[test]
fn missing_source_is_retried_after_a_package_has_been_cached() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        f.write("lib.hir", "pub fn value() -> u64 { 1 }\n");
        f.package("lib", &[], false);
        f.write("app.hir", "pub fn main() {}\n");
        f.package("app", &["lib"], true);
        let cache = PackageCache::new();
        assert!(
            f.load("app", &cache)
                .check(hirsel_engine::LoweringPolicy::Reachable)
                .unwrap()
                .diagnostics
                .is_empty()
        );
        std::fs::remove_file(f.0.join("lib.hir")).unwrap();
        assert!(matches!(
            load_program_source_with_cache(&f.0.join("app.json"), None, &cache),
            Err(ToolError::Io(_))
        ));
        f.write("lib.hir", "pub fn value() -> u64 { 1 }\n");
        assert!(
            f.load("app", &cache)
                .check(hirsel_engine::LoweringPolicy::Reachable)
                .unwrap()
                .diagnostics
                .is_empty()
        );
    });
}

#[test]
fn cache_identity_survives_relocation_and_adopted_edits_preserve_old_outputs() {
    with_compiler_stack(|| {
        let first = Fixture::new();
        let second = Fixture::new();
        for f in [&first, &second] {
            f.write("lib.hir", "pub fn value() -> u64 { 1 }\n");
            f.package("lib", &[], false);
            f.write("app.hir", "pub fn main() { let n = lib@1/value() }\n");
            f.package("app", &["lib"], true);
        }
        let cache = PackageCache::new();
        assert!(
            first
                .load("app", &cache)
                .check(hirsel_engine::LoweringPolicy::Reachable)
                .unwrap()
                .diagnostics
                .is_empty()
        );
        let mut adopted = second.load("app", &cache);
        assert!(
            adopted
                .check(hirsel_engine::LoweringPolicy::Reachable)
                .unwrap()
                .diagnostics
                .is_empty()
        );
        let path = definition("lib", "value");
        assert_eq!(adopted.engine.recompute_count(&path, QueryKind::Lower), 0);
        let old = adopted.engine.lower_to_tir(&path).unwrap().value;
        let old_dump = format!("{old:?}");
        let context = FileContext {
            package: path.0.package.clone(),
            modules: vec![],
        };
        let file = adopted
            .engine
            .sources
            .add(
                second.0.join("lib.hir"),
                "pub fn value() -> u64 { true }\n".into(),
            )
            .unwrap();
        adopted.engine.parse_file(file, &context).unwrap();
        assert!(matches!(
            adopted.engine.lower_to_tir(&path),
            Err(QueryError::Diagnostics(_))
        ));
        assert_eq!(format!("{old:?}"), old_dump);
        let mut pristine = first.load("app", &cache);
        assert!(
            pristine
                .check(hirsel_engine::LoweringPolicy::Reachable)
                .unwrap()
                .diagnostics
                .is_empty()
        );
        assert_eq!(pristine.engine.recompute_count(&path, QueryKind::Lower), 0);
        assert!(std::sync::Arc::ptr_eq(
            &old,
            &pristine.engine.lower_to_tir(&path).unwrap().value
        ));
    });
}

#[test]
fn engines_share_one_dependency_image_and_never_see_each_others_edits() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        f.write("lib.hir", "pub fn value() -> u64 { 1 }\n");
        f.package("lib", &[], false);
        f.write("app.hir", "pub fn main() { let n = lib@1/value() }\n");
        f.package("app", &["lib"], true);
        let cache = PackageCache::new();
        let path = definition("lib", "value");
        let syntax =
            |program: &mut LoadedProgram| program.engine.parse_definition(&path).unwrap().value.0;
        let mut first = f.load("app", &cache);
        assert!(
            first
                .check(hirsel_engine::LoweringPolicy::Reachable)
                .unwrap()
                .diagnostics
                .is_empty()
        );
        let mut second = f.load("app", &cache);
        let mut third = f.load("app", &cache);
        // The dependency was parsed once; both later engines hold its syntax.
        assert!(std::sync::Arc::ptr_eq(
            &syntax(&mut first),
            &syntax(&mut second)
        ));
        assert!(std::sync::Arc::ptr_eq(
            &syntax(&mut second),
            &syntax(&mut third)
        ));
        // An in-engine edit of the dependency stays in that engine.
        let context = FileContext {
            package: path.0.package.clone(),
            modules: vec![],
        };
        let file = second
            .engine
            .sources
            .add(
                f.0.join("lib.hir"),
                "pub fn value() -> u64 { true }\n".into(),
            )
            .unwrap();
        second.engine.parse_file(file, &context).unwrap();
        assert!(matches!(
            second.engine.lower_to_tir(&path),
            Err(QueryError::Diagnostics(_))
        ));
        assert!(
            third
                .check(hirsel_engine::LoweringPolicy::Reachable)
                .unwrap()
                .diagnostics
                .is_empty()
        );
        assert_eq!(third.engine.recompute_count(&path, QueryKind::Signature), 0);
        assert!(third.engine.lower_to_tir(&path).is_ok());
        let mut fourth = f.load("app", &cache);
        assert!(
            fourth
                .check(hirsel_engine::LoweringPolicy::Reachable)
                .unwrap()
                .diagnostics
                .is_empty()
        );
        assert!(std::sync::Arc::ptr_eq(
            &syntax(&mut third),
            &syntax(&mut fourth)
        ));
        // A changed dependency file selects a new image and is checked afresh.
        f.write("lib.hir", "pub fn value() -> u64 { false }\n");
        let mut changed = f.load("app", &cache);
        assert!(!std::sync::Arc::ptr_eq(
            &syntax(&mut fourth),
            &syntax(&mut changed)
        ));
        assert_eq!(
            rendered(
                &changed
                    .check(hirsel_engine::LoweringPolicy::Reachable)
                    .unwrap()
            ),
            rendered(&fresh(&f, "app"))
        );
        assert!(
            !changed
                .check(hirsel_engine::LoweringPolicy::Reachable)
                .unwrap()
                .diagnostics
                .is_empty()
        );
    });
}

#[test]
fn compile_time_limit_edits_retry_cached_package_results() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        f.write(
            "lib.hir",
            "pub fn value() -> u64 { comptime (1u64 + 2u64) }\n",
        );
        f.package("lib", &[], false);
        f.write("app.hir", "pub fn main() {}\n");
        f.package("app", &["lib"], true);
        let cache = PackageCache::new();
        assert!(
            f.load("app", &cache)
                .check(hirsel_engine::LoweringPolicy::Reachable)
                .unwrap()
                .diagnostics
                .is_empty()
        );
        for steps in [Some(0), None, Some(u64::MAX)] {
            let mut warm = f.load("app", &cache);
            warm.engine.comptime_limits.steps = steps;
            let cached = warm
                .check(hirsel_engine::LoweringPolicy::Reachable)
                .unwrap();
            let mut cold = f.load("app", &PackageCache::disabled());
            cold.engine.comptime_limits.steps = steps;
            assert_eq!(
                rendered(&cached),
                rendered(
                    &cold
                        .check(hirsel_engine::LoweringPolicy::Reachable)
                        .unwrap()
                )
            );
            if steps == Some(0) {
                assert_eq!(cached.diagnostics[0].rule.as_str(), "CT-1");
            } else {
                assert!(cached.diagnostics.is_empty());
            }
        }
        assert!(
            f.load("app", &cache)
                .check(hirsel_engine::LoweringPolicy::Reachable)
                .unwrap()
                .diagnostics
                .is_empty()
        );
    });
}

#[test]
#[ignore = "full gate"]
fn full_gate_cold_and_warm_corpus_verdicts_and_diagnostics_are_identical() {
    fn files(dir: &Path, result: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files(&path, result);
            } else if path.extension().is_some_and(|e| e == "hir") {
                result.push(path);
            }
        }
    }
    let mut corpus = vec![];
    files(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../conformance"),
        &mut corpus,
    );
    corpus.sort();
    let jobs = hirsel_guard::jobs();
    // Worker `w` checks programs w, w + jobs, ... in order, each warm through a cache of
    // its own, so a failing program replays from the same HIRSEL_JOBS.
    let corpus = &corpus;
    let timings: Vec<[std::time::Duration; 2]> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..jobs)
            .map(|worker| {
                scope.spawn(move || {
                    with_compiler_stack(|| {
                        let cache = PackageCache::new();
                        let mut timings = [std::time::Duration::ZERO; 2];
                        for path in corpus.iter().skip(worker).step_by(jobs) {
                            let mut verdicts = vec![];
                            for (i, cache) in
                                [&PackageCache::disabled(), &cache].into_iter().enumerate()
                            {
                                let start = std::time::Instant::now();
                                let verdict =
                                    match load_program_source_with_cache(path, None, cache) {
                                        Ok(mut p) => match p
                                            .check(hirsel_engine::LoweringPolicy::Reachable)
                                        {
                                            Ok(o) => format!("{:?}", rendered(&o)),
                                            Err(e) => format!("{e:?}"),
                                        },
                                        Err(ToolError::Query {
                                            error: QueryError::Diagnostics(ds),
                                            sources,
                                            ..
                                        }) => format!(
                                            "{:?}",
                                            ds.iter()
                                                .map(|d| d.to_json(&sources).to_string())
                                                .collect::<Vec<_>>()
                                        ),
                                        Err(e) => format!("{e}"),
                                    };
                                timings[i] += start.elapsed();
                                verdicts.push(verdict);
                            }
                            assert_eq!(verdicts[0], verdicts[1], "{}", path.display());
                        }
                        timings
                    })
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|worker| {
                worker
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            })
            .collect()
    });
    let total = |i: usize| timings.iter().map(|t| t[i]).sum::<std::time::Duration>();
    eprintln!(
        "{} corpus programs on {jobs} threads: cold {:?}, warm {:?} in all; zero verdict/diagnostic differences",
        corpus.len(),
        total(0),
        total(1)
    );
}

/// A model-based run over edits shared through one cache: a leaf package, a library
/// that reads it at compile time and at run time, and two roots. Each step edits one
/// file to one of its variants (accepted, rejected, comptime-failing, or changing the
/// interface the next package reads), checks a root on an engine loaded through the
/// shared images, or checks again an engine loaded earlier, after newer images were
/// published. A loaded engine's verdict and diagnostics equal a fresh uncached engine's
/// over the same files, and an engine checked again keeps them.
#[test]
fn engines_sharing_images_agree_with_fresh_engines_across_random_edits() {
    const LEAF: &[&str] = &[
        "pub fn value() -> u64 { 1 }\n",
        "pub fn value() -> u64 { 2 }\n",
        "pub fn value() -> u64 { true }\n",
        "pub fn value() -> u64 { 1 }\npub fn extra() -> bool { false }\n",
    ];
    const LIB: &[&str] = &[
        "pub fn value() -> u64 { comptime leaf@1/value() }\n",
        "pub fn value() -> u64 { leaf@1/value() }\n",
        "pub fn value() -> u64 { 7 }\n",
        "pub fn value() -> u64 { comptime (1u64 / 0u64) }\n",
        "pub fn value() -> bool { leaf@1/extra() }\n",
    ];
    const ROOT: &[&str] = &[
        "pub fn main() { let n = lib@1/value() }\n",
        "pub fn main() {}\n",
        "pub fn main() { missing }\n",
        "pub fn main() { let b: bool = lib@1/value() }\n",
    ];
    let files: [(&str, &[&str]); 4] = [("leaf", LEAF), ("lib", LIB), ("aaa", ROOT), ("zzz", ROOT)];
    for seed in 1u64..=5 {
        // splitmix64: a fixed sequence per seed, so a failure replays.
        let mut state = seed;
        let mut next = move |n: usize| {
            state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            ((z ^ (z >> 31)) % n as u64) as usize
        };
        with_compiler_stack(|| {
            let f = Fixture::new();
            for (name, variants) in files {
                f.write(&format!("{name}.hir"), variants[0]);
            }
            f.package("leaf", &[], false);
            f.package("lib", &["leaf"], false);
            f.package("aaa", &["lib"], true);
            f.package("zzz", &["lib"], true);
            let cache = PackageCache::new();
            let mut live: Vec<(LoadedProgram, (CheckCompletion, Vec<String>, Vec<String>))> =
                Vec::new();
            let mut trace = Vec::new();
            for _ in 0..60 {
                match next(10) {
                    0..=4 => {
                        let (name, variants) = files[next(files.len())];
                        let variant = next(variants.len());
                        trace.push(format!("edit {name} {variant}"));
                        f.write(&format!("{name}.hir"), variants[variant]);
                    }
                    5..=7 => {
                        let root = ["aaa", "zzz"][next(2)];
                        trace.push(format!("check {root}"));
                        let mut warm = f.load(root, &cache);
                        let verdict = rendered(
                            &warm
                                .check(hirsel_engine::LoweringPolicy::Reachable)
                                .unwrap(),
                        );
                        assert_eq!(
                            verdict,
                            rendered(&fresh(&f, root)),
                            "seed {seed}: {trace:?}"
                        );
                        if live.len() == 4 {
                            live.remove(0);
                        }
                        live.push((warm, verdict));
                    }
                    _ if !live.is_empty() => {
                        let which = next(live.len());
                        trace.push(format!("recheck {which}"));
                        let (program, verdict) = &mut live[which];
                        assert_eq!(
                            &rendered(
                                &program
                                    .check(hirsel_engine::LoweringPolicy::Reachable)
                                    .unwrap()
                            ),
                            verdict,
                            "seed {seed}: {trace:?}"
                        );
                    }
                    _ => {}
                }
            }
        });
    }
}
