//! The scoped document stays within ordinary layout and cleanup machinery.
use super::*;
use crate::native::{Options, TargetSpec};
use hirsel_engine::with_compiler_stack;
use hirsel_heapcheck::{Config, Event};
use hirsel_lir::layout_types::{LayoutKind, ScalarKind, SizeSpec};
use std::collections::BTreeMap;
use std::path::Path;

#[test]
fn dense_nodes_and_detached_text_drain_in_both_audit_modes() {
    with_compiler_stack(|| {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../conformance/SEM/SEM-3-json-document-retention-run.hir");
        // Keep four parses, all three document owners and exact ID checks;
        // full per-step recount uses a compact escaped token to bound audit work.
        let source = std::fs::read_to_string(&path)
            .unwrap()
            .replace("0u64..96u64", "0u64..2u64");
        let mut loaded = crate::load_program_source(&path, Some(&source)).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let snapshot = loaded.engine.export_native_input().unwrap();
        let input = loaded.engine.import_native_input(snapshot).unwrap();
        let ready = crate::native::build(
            &input,
            &Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        let table = &ready.table;
        let nodes: Vec<_> = table
            .layouts
            .iter()
            .filter(|r| {
                matches!(r.kind, LayoutKind::Aggregate)
                    && r.fields.len() == 10
                    && r.fields.iter().all(|f| {
                        matches!(
                            table.layout(f.layout).unwrap().kind,
                            LayoutKind::Scalar(ScalarKind::U64)
                        )
                    })
            })
            .collect();
        assert!(!nodes.is_empty(), "the emitted node record must be present");
        for node in nodes {
            assert!(table.layouts.iter().any(|row| matches!(row.kind, LayoutKind::Sequence { element } if table.layout(element) == Some(node))));
            eprintln!("plain node layout: {node:?}");
            assert_eq!(node.size, SizeSpec::Fixed(80));
            assert!(node.scan.is_empty());
            assert!(node.drop.is_empty());
        }
        let program = hirsel_heapcheck::Program {
            target: ready.target.clone(),
            layouts: ready.table.clone(),
            units: ready.raw_units(),
            statics: ready.statics.clone(),
        };
        let mut previous = None;
        for full_recount_every_step in [false, true] {
            let clean = hirsel_heapcheck::run_ready(
                &program,
                &loaded.entry,
                &[],
                &mut Io::new(),
                &Config {
                    full_recount_every_step,
                    ..Config::default()
                },
            );
            eprintln!(
                "document audit full_recount={full_recount_every_step}: sites={} counters={:?}",
                clean.sites, clean.counters
            );
            assert!(clean.sites > 0);
            let audit = clean.result.as_ref().unwrap();
            assert!(
                audit
                    .exemptions
                    .iter()
                    .all(|e| matches!(e, hirsel_heapcheck::Exemption::Static { .. })),
                "{:?}",
                audit.exemptions
            );
            // Odd-sized 41/43-byte boxes delimit callback entry/exit.
            // Sequence buffer capacities are aligned; these exact box sizes
            // identify test markers without expensive large inline payloads.
            // Record actual input/node/decoded IDs, then require every one freed
            // and a distinct, positively identified selected-text owner alive.
            // A loop-local marker may remain live until its next replacement;
            // exclude only its recorded allocation ID, never a capacity range.
            let mut live = BTreeMap::new();
            let mut markers = 0;
            let mut document_owners = BTreeSet::new();
            let mut selected_owner = None;
            let mut marker_owners = BTreeSet::new();
            for event in clean.trace.iter() {
                match event {
                    Event::Alloc {
                        alloc,
                        capacity,
                        owner,
                    } if *owner != "static" => {
                        if *capacity == 41 {
                            document_owners = live
                                .keys()
                                .copied()
                                .filter(|id| {
                                    Some(*id) != selected_owner && !marker_owners.contains(id)
                                })
                                .collect();
                            assert_eq!(document_owners.len(), 3, "{live:?}");
                            eprintln!(
                                "callback entry document owners: {document_owners:?}; old selected={selected_owner:?}"
                            );
                        }
                        if *capacity == 43 {
                            markers += 1;
                            assert!(!document_owners.is_empty());
                            assert!(
                                document_owners.iter().all(|id| !live.contains_key(id)),
                                "document owner survived: {live:?}"
                            );
                            let selected: Vec<_> = live
                                .keys()
                                .filter(|id| !marker_owners.contains(*id))
                                .copied()
                                .collect();
                            assert_eq!(
                                selected.len(),
                                1,
                                "exactly the selected text owner remains besides identified test markers: {live:?}"
                            );
                            let current = selected[0];
                            assert_ne!(
                                Some(current),
                                selected_owner,
                                "extraction allocated independently"
                            );
                            selected_owner = Some(current);
                            eprintln!(
                                "callback exit selected owner: {selected_owner:?}; input/node/decoded owners all gone"
                            );
                        }
                        if *capacity == 41 || *capacity == 43 {
                            marker_owners.insert(*alloc);
                        }
                        live.insert(*alloc, *capacity);
                    }
                    Event::Free { alloc } => {
                        marker_owners.remove(alloc);
                        live.remove(alloc);
                    }
                    _ => (),
                }
            }
            assert_eq!(markers, 4);
            assert!(live.is_empty());
            if let Some(previous) = &previous {
                assert_eq!(previous, &clean);
            }
            previous = Some(clean);
        }
    });
}

#[test]
fn construction_and_extraction_failures_drain_in_both_audit_modes() {
    with_compiler_stack(|| {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../conformance/SEM/SEM-3-json-document-retention-run.hir");
        // Exhaust failure sites of the same parser/extraction path on a small
        // dynamic input; the separate clean test checks repeated retention.
        let source = std::fs::read_to_string(&path)
            .unwrap()
            .replace("0u64..4u64", "0u64..1u64")
            .replace("0u64..96u64", "0u64..2u64");
        let mut loaded = crate::load_program_source(&path, Some(&source)).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let snapshot = loaded.engine.export_native_input().unwrap();
        let input = loaded.engine.import_native_input(snapshot).unwrap();
        let ready = crate::native::build(
            &input,
            &Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        let program = heap_program(&ready);
        for full_recount_every_step in [false, true] {
            let campaign = hirsel_heapcheck::audit_campaign_ready(
                &program,
                &loaded.entry,
                &[],
                &mut || Box::new(Io::new()),
                &Config {
                    full_recount_every_step,
                    ..Config::default()
                },
            );
            assert!(
                campaign.first_violation().is_none(),
                "{:?}",
                campaign.first_violation()
            );
            assert!(campaign.injected > 0);
            eprintln!(
                "small construction/extraction audit full_recount={full_recount_every_step}: {} injected failures, counters={:?}",
                campaign.injected, campaign.clean.counters
            );
        }
    });
}

#[test]
fn private_malformed_spans_links_and_tags_have_checked_twins() {
    with_compiler_stack(|| {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../conformance/SEM/SEM-3-json-document-retention-run.hir");
        // Compile the actual library source as this test package's owner module;
        // the installed std package exposes no constructor or unchecked helper.
        let source = format!(
            "{}\n{}",
            include_str!("../../../../../stdlib/std@1/json_document.hir"),
            r#"pub fn main() -> Result[(), u64] {
  let mut valid = empty_node(3)
  valid.value_source = 1
  valid.value_length = 1
  let good = Document(input: "x", decoded: b"", nodes: [valid])
  failure.check(text_bytes(good, 0) == Some(b"x"), TextInvariant())
  valid.value_start = 18446744073709551615
  let bad_start = Document(input: "x", decoded: b"", nodes: [valid])
  failure.check(text_bytes(bad_start, 0) == None(), TextInvariant())
  valid.value_start = 0
  valid.value_length = 18446744073709551615
  let bad_length = Document(input: "x", decoded: b"", nodes: [valid])
  failure.check(text_bytes(bad_length, 0) == None(), TextInvariant())
  valid.value_length = 1
  valid.value_source = 3
  let bad_source = Document(input: "x", decoded: b"", nodes: [valid])
  failure.check(text_bytes(bad_source, 0) == None(), TextInvariant())
  let mut parent = empty_node(4)
  parent.first = 1
  let linked = Document(input: "", decoded: b"", nodes: [parent, empty_node(0)])
  failure.check(first_child(linked, 0) == Some(1), TextInvariant())
  parent.first = 18446744073709551614
  parent.next = 18446744073709551614
  let broken = Document(input: "", decoded: b"", nodes: [parent])
  failure.check(first_child(broken, 0) == None(), TextInvariant())
  failure.check(next_sibling(broken, 0) == None(), TextInvariant())
  parent.kind = 99
  let bad_tag = Document(input: "", decoded: b"", nodes: [parent])
  failure.check(kind(bad_tag, 0) == None(), TextInvariant())
  Ok(())
}"#
        );
        let mut loaded = crate::load_program_source(&fixture, Some(&source)).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let snapshot = loaded.engine.export_native_input().unwrap();
        let input = loaded.engine.import_native_input(snapshot).unwrap();
        let ready = crate::native::build(
            &input,
            &Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        let program = heap_program(&ready);
        for full_recount_every_step in [false, true] {
            let campaign = hirsel_heapcheck::audit_campaign_ready(
                &program,
                &loaded.entry,
                &[],
                &mut || Box::new(Io::new()),
                &Config {
                    full_recount_every_step,
                    ..Config::default()
                },
            );
            assert!(
                campaign.first_violation().is_none(),
                "{:?}",
                campaign.first_violation()
            );
        }
    });
}

#[test]
fn a_small_input_slice_keeps_its_ordinary_backing_during_the_callback() {
    with_compiler_stack(|| {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../conformance/SEM/SEM-3-json-document-backing-run.hir");
        let mut loaded = crate::load_program(&path).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let snapshot = loaded.engine.export_native_input().unwrap();
        let input = loaded.engine.import_native_input(snapshot).unwrap();
        let ready = crate::native::build(
            &input,
            &Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        let program = heap_program(&ready);
        for full_recount_every_step in [false, true] {
            let clean = hirsel_heapcheck::run_ready(
                &program,
                &loaded.entry,
                &[],
                &mut Io::new(),
                &Config {
                    full_recount_every_step,
                    ..Config::default()
                },
            );
            assert!(clean.result.is_ok(), "{:?}", clean.result);
            let mut live = BTreeMap::new();
            let mut backing = None;
            for event in clean.trace.iter() {
                match event {
                    Event::Alloc {
                        alloc,
                        capacity,
                        owner,
                    } if *owner != "static" => {
                        if *capacity == 56 {
                            let retained = *live.values().max().expect("the backing owner is live");
                            assert!(retained >= 4099, "the three-byte input retains its backing");
                            backing = Some(retained);
                        }
                        live.insert(*alloc, *capacity);
                    }
                    Event::Free { alloc } => {
                        live.remove(alloc);
                    }
                    _ => (),
                }
            }
            assert!(live.is_empty());
            assert!(backing.is_some());
            eprintln!(
                "three-byte input retains {backing:?} capacity bytes during callback; audit full_recount={full_recount_every_step}"
            );
        }
    });
}
