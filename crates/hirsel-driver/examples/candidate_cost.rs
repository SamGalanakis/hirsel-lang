//! Bounded library host for serial candidate cost comparisons. The Python
//! client sends eight open-loop lines; this is measurement transport only.
hirsel_guard::install!();
use hirsel_driver::candidate::*;
use hirsel_driver::session::{FileChange, Session};
use hirsel_engine::{CheckCompletion, CompileConfig, LoweringPolicy};
use sha2::{Digest, Sha256};
use std::{
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};
fn ns(t: Instant) -> u64 {
    t.elapsed().as_nanos() as u64
}
fn emit(v: serde_json::Value) {
    println!("{v}");
    io::stdout().flush().unwrap();
}
fn selected_file(program: &hirsel_engine::LoadedProgram, path: &Path) -> hirsel_span::FileId {
    let files: Vec<_> = program
        .files
        .iter()
        .filter_map(|(id, _)| {
            (program.engine.sources.file(*id).unwrap().path == path).then_some(*id)
        })
        .collect();
    assert_eq!(files.len(), 1, "selected source must be unique");
    files[0]
}
fn fresh_receipt_file(sources: &hirsel_span::SourceMap, path: &Path) -> hirsel_span::FileId {
    // Fresh candidate engines cannot hold historical snapshots for this path.
    // Assert that invariant rather than choose a text matching the request.
    let files: std::collections::BTreeSet<_> = sources
        .anchors()
        .filter_map(|(_, id, _)| (sources.file(id)?.path == path).then_some(id))
        .collect();
    assert_eq!(
        files.len(),
        1,
        "fresh receipt has multiple source snapshots"
    );
    *files.first().unwrap()
}
fn checked_source_hash(sources: &hirsel_span::SourceMap, file: hirsel_span::FileId) -> String {
    Sha256::digest(sources.file(file).unwrap().text().as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn save_products(
    spec: &serde_json::Value,
    index: usize,
    products: &hirsel_engine::CheckedProducts,
) {
    if let Some(directory) = spec["artifacts"].as_str() {
        let directory = Path::new(directory);
        std::fs::create_dir_all(directory).unwrap();
        if let Some(canonical) = &products.canonical {
            std::fs::write(
                directory.join(format!("{index}.canonical")),
                canonical.as_bytes(),
            )
            .unwrap();
        }
        std::fs::write(
            directory.join(format!("{index}.reads")),
            format!("{:#?}", products.reads),
        )
        .unwrap();
    }
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let mode = &args[1];
    let spec: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&args[2]).unwrap()).unwrap();
    let entry = Path::new(spec["entry"].as_str().unwrap());
    let source = Path::new(spec["source"].as_str().unwrap());
    let original = std::fs::read(source).unwrap();
    let variants: Vec<String> = spec["variants"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().into())
        .collect();
    let started = Instant::now();
    let mut config = CompileConfig::for_memory_cap(Some(4 << 30));
    config.jobs = hirsel_intern::jobs::Jobs::new(1);
    if mode == "resident" {
        config.retention = hirsel_engine::memo::Retention::Resident;
    }
    if mode == "disk" {
        config.store = Arc::new(
            hirsel_engine::store::DirStore::new(PathBuf::from(spec["store"].as_str().unwrap()))
                .with_cap(256 << 20),
        );
    } else if mode != "cold" && mode != "cold-once" {
        config.store = Arc::new(hirsel_engine::store::MemStore::default());
    }
    let compiler = hirsel_driver::COMPILER
        .clone()
        .with_config(config.clone())
        .with_owned_engines();
    let workspace = CandidateWorkspace::new(
        config.clone(),
        hirsel_engine::ComptimeLimits::default(),
        Limits::default(),
    )
    .unwrap();
    if mode == "cold-once" {
        let index: usize = args[3].parse().unwrap();
        let tree = Path::new(spec["trees"][index].as_str().unwrap());
        let row = hirsel_engine::with_compiler_stack(|| {
            let load = Instant::now();
            let mut p = compiler.load_program_source(tree, None).unwrap();
            let load_ns = ns(load);
            let check = Instant::now();
            let output = p.check(LoweringPolicy::Comptime).unwrap();
            let check_ns = ns(check);
            assert_eq!(output.completion, CheckCompletion::Complete);
            assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
            let adopted = p.engine.store_adoptions();
            let recomputed = p.engine.memo_stats().recomputed.values().sum::<u64>();
            let receipt = Instant::now();
            let checked_source = checked_source_hash(
                &output.sources,
                selected_file(
                    &p,
                    &tree.parent().unwrap().join(source.file_name().unwrap()),
                ),
            );
            let products = p.engine.checked_products(&p.entry).unwrap();
            save_products(&spec, index, &products);
            let digest = products.digest;
            serde_json::json!({"checked_source_sha256":checked_source,"digest":digest.0,"load_ns":load_ns,"check_ns":check_ns,"receipt_ns":ns(receipt),"adoptions":adopted,"recomputations":recomputed})
        });
        emit(row);
        return;
    }
    let base = if mode == "frozen" {
        Some(workspace.capture(entry).unwrap())
    } else {
        None
    };
    let mut resident = Session::open_configured(entry, config.clone());
    if mode == "disk" || mode == "frozen" {
        hirsel_engine::with_compiler_stack(|| {
            let mut p = compiler.load_program_source(entry, None).unwrap();
            assert!(
                p.check(LoweringPolicy::Comptime)
                    .unwrap()
                    .diagnostics
                    .is_empty()
            );
        });
    }
    if mode == "resident" {
        assert!(resident.check_applied().unwrap().diagnostics.is_empty());
    }
    let mut leases = vec![];
    emit(
        serde_json::json!({"ready":true,"initialization_ns":ns(started),"compiler":format!("{:02x?}",hirsel_driver::COMPILER_IDENTITY),"mode":mode,"store":config.store.name(),"retention":format!("{:?}",config.retention)}),
    );
    for line in io::stdin().lock().lines() {
        let i: usize = line.unwrap().parse().unwrap();
        let begin = Instant::now();
        let mut load_ns = 0;
        let mut check_ns = 0;
        let mut receipt_ns = 0;
        let mut restore_ns = 0;
        let mut adopted = 0;
        let mut recomputed = 0;
        let mut restore_adoptions = 0;
        let mut restore_recomputations = 0;
        let resident_before = if mode == "resident" {
            (resident.store_adoptions(), resident.recomputations())
        } else {
            (0, 0)
        };
        let digest;
        let checked_source;
        let mut admission_ns = 0;
        if mode == "frozen" {
            let admission = Instant::now();
            let base = base.as_ref().unwrap();
            let (key, old, expected) = base
                .sources()
                .find(|(k, _, _)| base.source_path(k) == Some(source))
                .unwrap();
            let lease = workspace
                .submit(
                    base,
                    CandidateRequest {
                        id: CandidateId {
                            client: 1,
                            nonce: i as u64,
                        },
                        base: base.token().clone(),
                        priority: Priority::Selected,
                        edits: vec![FileEdit {
                            file: key.clone(),
                            expected,
                            edits: vec![ByteEdit {
                                start: 0,
                                end: old.len() as u32,
                                replacement: variants[i].clone(),
                            }],
                        }],
                    },
                )
                .unwrap();
            leases.push(lease);
            admission_ns = ns(admission);
            let CandidateResult::Ready(r) = workspace.result(&leases[i]).unwrap() else {
                panic!("serial pending")
            };
            if r.completion != Completion::Complete || r.verdict != Verdict::Pass {
                emit(
                    serde_json::json!({"id":i,"refused":true,"completion":format!("{:?}",r.completion),"verdict":format!("{:?}",r.verdict),"counters":format!("{:?}",workspace.counters()),"result_limit_bytes":Limits::default().result_bytes,"workspace_limit_bytes":Limits::default().bytes,"reason":r.loader_error}),
                );
                return;
            }
            let products = r.checked_products.as_ref().unwrap();
            save_products(&spec, i, products);
            digest = products.digest;
            checked_source = checked_source_hash(
                &r.output.sources,
                fresh_receipt_file(&r.output.sources, source),
            );
        } else if mode == "resident" {
            let t = Instant::now();
            resident.apply(vec![FileChange {
                path: source.into(),
                bytes: Some(variants[i].as_bytes().to_vec()),
            }]);
            let output = resident.check_applied().unwrap();
            check_ns = ns(t);
            assert_eq!(output.completion, CheckCompletion::Complete);
            assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
            checked_source =
                checked_source_hash(&output.sources, fresh_receipt_file(&output.sources, source));
            let t = Instant::now();
            adopted = resident.store_adoptions() - resident_before.0;
            recomputed = resident.recomputations() - resident_before.1;
            let products = resident.checked_products(true).unwrap();
            save_products(&spec, i, &products);
            digest = products.digest;
            receipt_ns = ns(t);
            let t = Instant::now();
            resident.apply(vec![FileChange {
                path: source.into(),
                bytes: Some(original.clone()),
            }]);
            assert!(resident.check_applied().unwrap().diagnostics.is_empty());
            restore_ns = ns(t);
            restore_adoptions = resident.store_adoptions() - resident_before.0 - adopted;
            restore_recomputations = resident.recomputations() - resident_before.1 - recomputed;
        } else if mode == "cold" {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["cold-once", &args[2], &i.to_string()])
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let row: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            load_ns = row["load_ns"].as_u64().unwrap();
            check_ns = row["check_ns"].as_u64().unwrap();
            receipt_ns = row["receipt_ns"].as_u64().unwrap();
            checked_source = row["checked_source_sha256"].as_str().unwrap().to_owned();
            adopted = row["adoptions"].as_u64().unwrap();
            recomputed = row["recomputations"].as_u64().unwrap();
            digest = hirsel_syntax::ContentHash(
                row["digest"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|n| n.as_u64().unwrap() as u8)
                    .collect::<Vec<_>>()
                    .try_into()
                    .unwrap(),
            );
        } else {
            let tree = Path::new(spec["trees"][i].as_str().unwrap());
            let output = hirsel_engine::with_compiler_stack(|| {
                let t = Instant::now();
                let mut p = compiler.load_program_source(tree, None).unwrap();
                load_ns = ns(t);
                let t = Instant::now();
                let out = p.check(LoweringPolicy::Comptime).unwrap();
                check_ns = ns(t);
                assert_eq!(out.completion, CheckCompletion::Complete);
                assert!(out.diagnostics.is_empty(), "{:?}", out.diagnostics);
                adopted = p.engine.store_adoptions();
                recomputed = p.engine.memo_stats().recomputed.values().sum::<u64>();
                let t = Instant::now();
                let products = p.engine.checked_products(&p.entry).unwrap();
                save_products(&spec, i, &products);
                receipt_ns = ns(t);
                (
                    products.digest,
                    checked_source_hash(
                        &out.sources,
                        selected_file(
                            &p,
                            &tree.parent().unwrap().join(source.file_name().unwrap()),
                        ),
                    ),
                )
            });
            (digest, checked_source) = output;
        }
        emit(
            serde_json::json!({"id":i,"checked_source_sha256":checked_source,"digest":digest.0,"total_ns":ns(begin),"load_ns":load_ns,"check_ns":check_ns,"receipt_ns":receipt_ns,"admission_ns":admission_ns,"restore_ns":restore_ns,"restore_adoptions":restore_adoptions,"restore_recomputations":restore_recomputations,"adoptions":adopted,"recomputations":recomputed}),
        );
    }
    let session_stats = format!("{:?}", resident.stats());

    let c = workspace.counters();
    drop(leases);
    drop(base);
    let after_release = workspace.counters();
    emit(
        serde_json::json!({"session_stats":session_stats,"counters":format!("{c:?}"),"frozen_loads":c.frozen_loads,"adoptions":c.store_adoptions,"recomputations":c.recomputations,"completed":c.completed_results,"load_ns":c.load_nanos,"check_ns":c.check_nanos,"receipt_ns":c.receipt_nanos,"retained_bytes":c.retained_bytes,"retained_after_release_bytes":after_release.retained_bytes,"dropped_engines":c.dropped_engines,"type_bytes":c.peak_type_bytes}),
    );
}
