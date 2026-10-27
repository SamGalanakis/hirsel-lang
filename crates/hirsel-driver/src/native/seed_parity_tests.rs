//! Cross-open and fresh-compilation vectors from Go cache_test.go:22–60,
//! 99–128 and Cranelift incremental_cache.rs:39–95, with an ordinary local donor.
use super::tests::{count, options};
use super::*;
use crate::native::{Backend, stages};
use hirsel_engine::DefinitionQueries;
use hirsel_intern::timings::{self, Count};
use hirsel_lir::objects::{DirObjects, NoObjects, ObjectStore, UnitKey};
use std::sync::Arc;

const NEXT_STREAM: &str = "pub fn main()->Result[(),()]{let f=std@1/http.next_stream;Ok(())}";

fn consumer(path: &std::path::Path) -> ValidatedNativeInput {
    checked_consumer(&crate::COMPILER.with_owned_engines(), path)
}

fn checked_consumer(
    compiler: &hirsel_engine::Compiler,
    path: &std::path::Path,
) -> ValidatedNativeInput {
    let mut loaded = compiler.load_program(path).unwrap();
    loaded.check_for_execution().unwrap();
    loaded
        .engine
        .native_input_for(&[loaded.entry.clone()])
        .unwrap()
}

fn next_stream(backend: Backend, jobs: usize) -> String {
    hirsel_engine::with_compiler_stack(|| {
        timings::enable();
        let scratch = crate::native::executable::Scratch::new().unwrap();
        let source = scratch.0.join("next-stream.hir");
        std::fs::write(&source, NEXT_STREAM).unwrap();
        let shared = scratch.0.join("shared");
        let mut compiler = crate::COMPILER.with_owned_engines();
        if let Some(products) = std::env::var_os("HIRSEL_SEED_SOURCE_PRODUCTS") {
            compiler = compiler.with_config(hirsel_engine::CompileConfig {
                store: Arc::new(hirsel_engine::store::DirStore::new(products.into())),
                ..Default::default()
            });
        }
        // Produce ordinary fragments in a distinct program. Its extra tuple
        // shifts layout numbering before the HTTP resource closure.
        {
            let donor = scratch.0.join("donor.hir");
            std::fs::write(&donor,
                "fn aaa()->(bool,u32){(true,0u32)}\npub fn main()->Result[(),()]{let _=aaa();let f=std@1/http.next_stream;Ok(())}").unwrap();
            let input = checked_consumer(&compiler, &donor);
            let mut opts = options(Arc::new(DirObjects::new(shared.clone())), 1);
            let objects = opts.objects.as_mut().unwrap();
            objects.backend = backend;
            objects.debug = true;
            stages::build_full(&input, &opts).unwrap();
        }
        {
            // Source checking is independent of the object-store policy under
            // test. Keep a fresh verified source snapshot for each salt, then
            // compare every cold/warm/donor/verified native consumer against it.
            let input = checked_consumer(&compiler, &source);
            let mut opts = options(
                Arc::new(DirObjects::new(scratch.0.join("source-cache"))),
                jobs,
            );
            opts.objects.as_mut().unwrap().backend = backend;
            opts.objects.as_mut().unwrap().debug = true;
            let cold = prepare_in(&input, &opts, Mode::Disabled).unwrap();
            let expected = cold.ready.unit_objects.as_ref().unwrap().objects.clone();
            let summaries = cold.ready.summaries.clone();
            drop(cold);
            // Warm source imports in an independent cache must agree too.
            let warm = prepare_in(&input, &opts, Mode::Reuse).unwrap();
            assert_eq!(summaries, warm.ready.summaries);
            assert_eq!(
                &expected,
                &warm.ready.unit_objects.as_ref().unwrap().objects
            );
            drop(warm);
            opts.objects.as_mut().unwrap().store = Arc::new(DirObjects::new(shared.clone()));
            let ordinary = prepare_in(&input, &opts, Mode::Disabled).unwrap();
            assert!(ordinary.ready.unit_objects.as_ref().unwrap().hits > 0);
            assert_eq!(
                &expected,
                &ordinary.ready.unit_objects.as_ref().unwrap().objects
            );
            drop(ordinary);
            let populated = prepare_in(&input, &opts, Mode::Reuse).unwrap();
            assert!(!populated.stats.hit);
            assert_eq!(
                &expected,
                &populated.ready.unit_objects.as_ref().unwrap().objects
            );
            let records = populated
                .ready
                .production
                .as_ref()
                .unwrap()
                .records
                .iter()
                .map(|r| (r.key, r.digest))
                .collect::<Vec<_>>();
            let channel = populated
                .ready
                .summaries
                .iter()
                .find(|s| s.definition == hirsel_syntax::DefPath::core("channel.open"))
                .expect("the HTTP transfer resource channel must execute");
            assert_ne!(channel.instance, ContentHash([0; 32]));
            drop(populated);
            let before = [Count::ValueDecodes, Count::ValueRebinds, Count::LateKeys].map(count);
            let hit = prepare_in(&input, &opts, Mode::Reuse).unwrap();
            assert!(hit.stats.hit, "{:?}", hit.stats);
            assert_eq!(
                before,
                [Count::ValueDecodes, Count::ValueRebinds, Count::LateKeys].map(count)
            );
            assert_eq!(&expected, &hit.ready.unit_objects.as_ref().unwrap().objects);
            drop(hit);
            let verified = prepare_in(&input, &opts, Mode::VerifyReuse).unwrap();
            assert_eq!(
                &expected,
                &verified.ready.unit_objects.as_ref().unwrap().objects
            );
            assert_eq!(summaries, verified.ready.summaries);
            format!("{records:?}")
        }
    })
}

fn salted(test: &str, backend: Backend, jobs: usize, salts: &[u64]) {
    let source = crate::native::executable::Scratch::new().unwrap();
    let products = source.0.join("source-products");
    let mut outputs = Vec::new();
    // Source products are ordinary checked query results. Native object stores
    // and every compiler engine remain fresh in each independently salted child.
    for &salt in salts {
        let output = if backend == Backend::Cranelift {
            hirsel_intern::under_salts(test, &[salt], || next_stream(backend, jobs))
        } else {
            hirsel_intern::under_salts_with_env(
                test,
                &[salt],
                &[("HIRSEL_SEED_SOURCE_PRODUCTS", products.as_os_str())],
                || next_stream(backend, jobs),
            )
        };
        match output {
            Some(output) => outputs.extend(output),
            None => return,
        }
    }
    assert!(outputs.windows(2).all(|w| w[0] == w[1]));
}

// Compare both remaining salts with the same reference. Requiring both
// equalities still requires all three original outputs to be identical.
macro_rules! stream_salts {
    ($name:ident, $backend:expr, $salts:expr) => {
        #[test]
        fn $name() {
            salted(
                concat!("native::plan::seed_parity_tests::", stringify!($name)),
                $backend,
                8,
                $salts,
            );
        }
    };
}
stream_salts!(
    ordinary_next_stream_cranelift_jobs8,
    Backend::Cranelift,
    &[1, 17, 65537]
);
stream_salts!(
    ordinary_next_stream_llvm_jobs8_salts1_17,
    Backend::Llvm,
    &[1, 17]
);
stream_salts!(
    ordinary_next_stream_llvm_jobs8_salts1_65537,
    Backend::Llvm,
    &[1, 65537]
);

// Ordinary local publication replaces the former dedicated seed builder. Each
// consumer opens a fresh compiler and cache handle; neither V nor import/Ready
// validation is supplied by the fragment provider.
fn ordinary_provider(backend: Backend) {
    hirsel_engine::with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::write(root.join("library.hir"), "type Payload { n:u64 }\npub fn aaa()->(bool,u32) { (true,0u32) }\npub fn answer(x:u64)->u64 { let p=Payload(n:x); let get=fn()->u64 { p.n }; get()+1u64 }\n").unwrap();
        let library = root.join("library.json");
        std::fs::write(
            &library,
            serde_json::json!({
                "schema":1,"package":{"name":"ordinary","version":"1"},
                "sources":[{"path":"library.hir","module":[]}],"dependencies":[]
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(root.join("main.hir"), "pub fn main()->Result[(),()] { failure.check(lib.answer(3u64)==4u64, Arithmetic()); Ok(()) }\n").unwrap();
        let application = root.join("application.json");
        std::fs::write(&application, serde_json::json!({
            "schema":1,"package":{"name":"consumer","version":"1"},
            "sources":[{"path":"main.hir","module":[]}],
            "dependencies":[{"manifest":"library.json","package":{"name":"ordinary","version":"1"},"expose":[{"head":"lib","module":[]}]}],
            "entry":{"module":[],"name":"main"}
        }).to_string()).unwrap();
        let cache = root.join("objects");
        let mut opts = options(Arc::new(DirObjects::new(cache.clone())), 1);
        opts.objects.as_mut().unwrap().backend = backend;
        let donor = {
            let mut package = crate::COMPILER
                .with_owned_engines()
                .load_package_declarations(&library)
                .unwrap();
            let roots = ["aaa", "answer"].map(|name| {
                DefPath(hirsel_syntax::ItemPath {
                    package: package.package.clone(),
                    modules: vec![],
                    name: name.into(),
                })
            });
            package.engine.native_input_for(&roots).unwrap()
        };
        let published = stages::build_full(&donor, &opts).unwrap();
        assert!(!published.unit_objects.as_ref().unwrap().objects.is_empty());
        drop(published);
        drop(donor);
        opts.objects.as_mut().unwrap().store = Arc::new(DirObjects::new(cache.clone()));
        let first = prepare_in(&consumer(&application), &opts, Mode::Disabled).unwrap();
        assert!(
            first.ready.unit_objects.as_ref().unwrap().hits > 0,
            "{backend:?}: no ordinary donor hit"
        );
        let expected = first.ready.unit_objects.as_ref().unwrap().objects.clone();
        let summaries = first.ready.summaries.clone();
        drop(first);
        // Full reference computation imports, checks strict C and Ready, and
        // independently compares every cached fragment with fresh emission.
        let verified = prepare_in(&consumer(&application), &opts, Mode::VerifyReuse).unwrap();
        assert_eq!(
            expected,
            verified.ready.unit_objects.as_ref().unwrap().objects
        );
        assert_eq!(summaries, verified.ready.summaries);
        drop(verified);
        let warm = prepare_in(&consumer(&application), &opts, Mode::Reuse).unwrap();
        assert!(warm.stats.hit, "{backend:?}: {:?}", warm.stats);
        assert_eq!(expected, warm.ready.unit_objects.as_ref().unwrap().objects);
        drop(warm);
        struct Imported(DirObjects);
        impl ObjectStore for Imported {
            fn get(&self, key: &UnitKey) -> Option<Arc<[u8]>> {
                self.0.get(key)
            }
            fn put(&self, _: &UnitKey, _: &[u8]) {}
        }
        let imported = Arc::new(Imported(DirObjects::new(cache)));
        assert!(!imported.production_enabled());
        opts.objects.as_mut().unwrap().store = imported;
        let imported = prepare_in(&consumer(&application), &opts, Mode::Reuse).unwrap();
        assert!(
            !imported.stats.hit,
            "copied bytes gained local producer authority"
        );
        assert_eq!(imported.stats.fallback, Some("store-disabled"));
        assert_eq!(
            expected,
            imported.ready.unit_objects.as_ref().unwrap().objects
        );
        drop(imported);
        opts.objects.as_mut().unwrap().store = Arc::new(NoObjects);
        let fresh = stages::build_full(&consumer(&application), &opts).unwrap();
        assert_eq!(expected, fresh.unit_objects.as_ref().unwrap().objects);
    });
}

#[test]
fn ordinary_provider_cranelift() {
    ordinary_provider(Backend::Cranelift);
}
#[test]
fn ordinary_provider_llvm() {
    ordinary_provider(Backend::Llvm);
}

#[test]
fn http_reject_lowering_has_valid_projections() {
    hirsel_engine::with_compiler_stack(|| {
        let manifest =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stdlib/std@1/hirsel.json");
        let mut package = crate::COMPILER
            .load_package_declarations(&manifest)
            .unwrap();
        let root = hirsel_syntax::DefPath(hirsel_syntax::ItemPath {
            package: package.package.clone(),
            modules: vec!["http".into()],
            name: "http_server_reject".into(),
        });
        // This private root is absent from ordinary program reachability.
        // Keep its independent V projection regression covered.
        package.engine.lower_to_tir(&root).unwrap();
    });
}

// A contract helper's closure is addressed and called in the same singleton
// object. An untyped LLVM address declaration conflicts with the bound call.
#[test]
fn ordinary_store_delete_has_one_consistent_entry_abi() {
    hirsel_engine::with_compiler_stack(|| {
        let manifest =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stdlib/std@1/hirsel.json");
        let mut package = crate::COMPILER
            .load_package_declarations(&manifest)
            .unwrap();
        let root = hirsel_syntax::DefPath(hirsel_syntax::ItemPath {
            package: package.package.clone(),
            modules: vec!["store".into()],
            name: "tx_delete".into(),
        });
        let input = package.engine.native_input_for(&[root]).unwrap();
        for backend in [Backend::Cranelift, Backend::Llvm] {
            for opt in [
                hirsel_lir::emission::OptLevel::None,
                hirsel_lir::emission::OptLevel::Default,
                hirsel_lir::emission::OptLevel::Speed,
            ] {
                let options =
                    super::super::Options::new(hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu());
                let ready = stages::build_full(&input, &options)
                    .unwrap_or_else(|e| panic!("{backend:?} {opt:?}: {e}"));
                let sources = super::super::profile_sources(input.snapshot());
                super::super::emit_backend_objects(
                    &ready,
                    backend,
                    opt,
                    super::super::Partition::Singleton,
                    false,
                    (opt == hirsel_lir::emission::OptLevel::None).then_some(&sources),
                )
                .unwrap_or_else(|e| panic!("{backend:?} {opt:?}: {e}"));
            }
        }
    });
}
