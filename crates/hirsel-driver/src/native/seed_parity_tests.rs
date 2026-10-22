//! Cross-open and fresh-compilation vectors from Go cache_test.go:22–60,
//! 99–128 and Cranelift incremental_cache.rs:39–95, with a read-only donor.
use super::tests::{count, options};
use super::*;
use crate::native::{Backend, stages};
use hirsel_intern::timings::{self, Count};
use hirsel_lir::objects::{DirObjects, ReadOnlyObjects, SeededObjects};
use std::sync::Arc;

const NEXT_STREAM: &str = "pub fn main()->Result[(),()]{let f=std@1/http.next_stream;Ok(())}";

fn consumer(path: &std::path::Path) -> ValidatedNativeInput {
    let mut loaded = crate::COMPILER
        .with_owned_engines()
        .load_program(path)
        .unwrap();
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
        let seed = scratch.0.join("seed");
        // Produce legacy fragments without Ready-call sidecars in a distinct
        // program. Its extra tuple shifts layout numbering before the HTTP
        // resource closure, like the larger std seed producer.
        {
            let donor = scratch.0.join("donor.hir");
            std::fs::write(&donor,
                "fn aaa()->(bool,u32){(true,0u32)}\npub fn main()->Result[(),()]{let _=aaa();let f=std@1/http.next_stream;Ok(())}").unwrap();
            let input = consumer(&donor);
            let mut opts = options(Arc::new(DirObjects::new(seed.clone())), 1);
            let objects = opts.objects.as_mut().unwrap();
            objects.backend = backend;
            objects.debug = true;
            stages::build_full(&input, &opts).unwrap();
        }
        {
            let mut opts = options(
                Arc::new(DirObjects::new(scratch.0.join("source-cache"))),
                jobs,
            );
            opts.objects.as_mut().unwrap().backend = backend;
            opts.objects.as_mut().unwrap().debug = true;
            let cold = prepare_in(&consumer(&source), &opts, Mode::Disabled).unwrap();
            let expected = cold.ready.unit_objects.as_ref().unwrap().objects.clone();
            let summaries = cold.ready.summaries.clone();
            drop(cold);
            // Warm source imports without a seed must agree too.
            let warm = prepare_in(&consumer(&source), &opts, Mode::Reuse).unwrap();
            assert_eq!(summaries, warm.ready.summaries);
            assert_eq!(
                &expected,
                &warm.ready.unit_objects.as_ref().unwrap().objects
            );
            drop(warm);
            let writable = Arc::new(DirObjects::new(scratch.0.join("writable")));
            opts.objects.as_mut().unwrap().store = Arc::new(SeededObjects {
                writable,
                seed: Arc::new(ReadOnlyObjects(seed.clone())),
            });
            let ordinary = prepare_in(&consumer(&source), &opts, Mode::Disabled).unwrap();
            assert!(ordinary.ready.unit_objects.as_ref().unwrap().hits > 0);
            assert_eq!(
                &expected,
                &ordinary.ready.unit_objects.as_ref().unwrap().objects
            );
            drop(ordinary);
            let populated = prepare_in(&consumer(&source), &opts, Mode::Reuse).unwrap();
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
            let hit = prepare_in(&consumer(&source), &opts, Mode::Reuse).unwrap();
            assert!(hit.stats.hit, "{:?}", hit.stats);
            assert_eq!(
                before,
                [Count::ValueDecodes, Count::ValueRebinds, Count::LateKeys].map(count)
            );
            assert_eq!(&expected, &hit.ready.unit_objects.as_ref().unwrap().objects);
            drop(hit);
            let verified = prepare_in(&consumer(&source), &opts, Mode::VerifyReuse).unwrap();
            assert_eq!(
                &expected,
                &verified.ready.unit_objects.as_ref().unwrap().objects
            );
            assert_eq!(summaries, verified.ready.summaries);
            format!("{records:?}")
        }
    })
}

fn salted(test: &str, backend: Backend, jobs: usize) {
    let mut outputs = Vec::new();
    // Sequential fresh processes keep the HTTP closure under the normal cap.
    for salt in [1, 17, 65537] {
        match hirsel_intern::under_salts(test, &[salt], || next_stream(backend, jobs)) {
            Some(output) => outputs.extend(output),
            None => return,
        }
    }
    assert!(outputs.windows(2).all(|w| w[0] == w[1]));
}

#[test]
fn installed_next_stream_cranelift_jobs1() {
    salted(
        "native::plan::seed_parity_tests::installed_next_stream_cranelift_jobs1",
        Backend::Cranelift,
        1,
    );
}

#[test]
fn installed_next_stream_cranelift_jobs2() {
    salted(
        "native::plan::seed_parity_tests::installed_next_stream_cranelift_jobs2",
        Backend::Cranelift,
        2,
    );
}

#[test]
fn installed_next_stream_cranelift_jobs8() {
    salted(
        "native::plan::seed_parity_tests::installed_next_stream_cranelift_jobs8",
        Backend::Cranelift,
        8,
    );
}

#[test]
fn installed_next_stream_llvm_jobs1() {
    salted(
        "native::plan::seed_parity_tests::installed_next_stream_llvm_jobs1",
        Backend::Llvm,
        1,
    );
}

#[test]
fn installed_next_stream_llvm_jobs2() {
    salted(
        "native::plan::seed_parity_tests::installed_next_stream_llvm_jobs2",
        Backend::Llvm,
        2,
    );
}

#[test]
fn installed_next_stream_llvm_jobs8() {
    salted(
        "native::plan::seed_parity_tests::installed_next_stream_llvm_jobs8",
        Backend::Llvm,
        8,
    );
}
