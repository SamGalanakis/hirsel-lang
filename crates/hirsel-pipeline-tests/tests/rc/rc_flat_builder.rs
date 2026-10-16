//! Flat list builders (MEM-12, LIR schema revision 22): a local list of
//! directly constructed variable flat records taken whole into one flat field
//! is built in that field's encoding (`Inst::FlatAppend`), other lists stay
//! ordinary, and C and H reject forged appends and builder sources.
use crate::rc_support as support;

use hirsel_driver::compiler_adapters;
use hirsel_engine::{
    QueryEngine, RunConfiguration, check_progress, default_context, default_environment,
    entry_path, run_observed_snapshot, with_compiler_stack,
};
use hirsel_heapcheck::{Config, NoOperations, Observed};
use hirsel_interp::observation::ObservedNode;
use hirsel_lir::ir::{Inst, LirUnit};
use hirsel_lir::stage::Ready;
use hirsel_lir::validate::{Inputs, ViolationKind};
use hirsel_rc::cleanup::Finalizers;
use hirsel_rc::import::Lowered;
use hirsel_rc::insert::Proofs;
use hirsel_rc::instance::InstanceId;
use hirsel_rc::placement::Options;

use support::*;

/// `main` returns `value()` as its `Err` payload, so both executors report it.
const ENTRY: &str = "pub fn main() -> Result[(), u64] {\n  Error(value())\n}\n";

const TYPES: &str = "flat type Message { id: u64, kind: u64, payload: Bytes }
flat type Messages { items: List[Message] }
fn checksum(b: Bytes) -> u64 {
  let mut h: u64 = 14695981039346656037u64
  for i in 0u64..bytes.len(b) {
    let product: u128 = int.widen(h ^ int.widen(b[i])) * 1099511628211u128
    h = int.truncate(product)
  }
  h
}
fn digest(messages: Messages) -> u64 {
  let mut s: u64 = list.len(messages.items)
  for m in messages.items {
    s = int.wrapping_add(s ^ 1540483477u64, m.id ^ m.kind ^ checksum(m.payload))
  }
  s
}
fn bytes_of(i: u64) -> Bytes {
  let mut payload: List[u8] = []
  for j in 0u64..i % 11u64 { list.push(mut payload, int.narrow(97u64 + (i + j * 7u64) % 26u64)) }
  bytes.from_list(take payload)
}
";

fn source(body: &str) -> String {
    format!("{TYPES}{body}{ENTRY}")
}

/// The flat appends of `value`.
fn appends(lowered: &Lowered) -> usize {
    unit_named(lowered, "value")
        .instructions
        .iter()
        .filter(|i| matches!(i, Inst::FlatAppend { .. }))
        .count()
}

/// Every stage to Ready; C accepts every unit at every stage.
fn ready(source: &str, lowered: Lowered) -> hirsel_heapcheck::Program<Ready> {
    let program = compile(source);
    let table = lowered.table.clone();
    let manifest = manifest(&program);
    let target = target();
    let operations = program.operation_registry();
    let inputs = Inputs {
        operations: &operations,
        target: &target,
        layouts: &table,
        manifest: &manifest,
    };
    let checked = |stage: &str, v: Vec<hirsel_lir::validate::Violation>, d: String| {
        assert!(v.is_empty(), "{source}\n{stage}\n{d}\n{v:#?}");
    };
    let mut rc = Vec::new();
    for (i, u) in lowered.units.iter().enumerate() {
        checked("value", hirsel_lir::validate::check(u, &inputs), dump(u));
        let id = InstanceId(i as u32);
        let options = Options::for_instance(true, &lowered.instances, id);
        let placed = hirsel_rc::placement::place(u.clone(), &table, &options).unwrap();
        let proofs = Proofs::for_instance(&lowered.instances, id);
        let r = hirsel_rc::insert::insert(placed, &table, &proofs)
            .unwrap_or_else(|e| panic!("{source}\ninsert: {e:?}"));
        checked("rc", hirsel_lir::validate::check(&r, &inputs), dump(&r));
        rc.push(hirsel_rc::elide::elide(r, &table).unwrap());
    }
    let finalizers = Finalizers::new(&lowered.instances, &rc);
    let units: Vec<LirUnit<Ready>> = rc
        .into_iter()
        .map(|u| hirsel_rc::ready::ready(u, &table, &finalizers).unwrap())
        .collect();
    for u in &units {
        checked("ready", hirsel_lir::validate::check(u, &inputs), dump(u));
    }
    hirsel_heapcheck::Program {
        target,
        layouts: table,
        units,
        statics: lowered.statics,
    }
}

/// H's `value()`, auditing every injected failure site, or the violation.
fn heap(program: &hirsel_heapcheck::Program<Ready>) -> Result<u64, String> {
    let campaign = hirsel_heapcheck::fault_campaign(
        program,
        &path("main"),
        &[],
        &mut || Box::new(NoOperations),
        &Config::default(),
    );
    if let Some((site, v)) = campaign.first_violation() {
        return Err(format!("site {site:?}: {v}"));
    }
    let observation = campaign
        .clean
        .result
        .map_err(|v| v.to_string())?
        .observation;
    assert!(
        observation.failures.is_empty(),
        "{:?}",
        observation.failures
    );
    match observation.returned {
        Some(Observed::Variant { fields, .. }) => match fields.into_iter().next().flatten() {
            Some(Observed::Integer { bits, .. }) => Ok(bits as u64),
            other => panic!("returned {other:?}"),
        },
        other => panic!("returned {other:?}"),
    }
}

/// The interpreter's `value()`.
fn reference(source: &str) -> u64 {
    let source = source.to_string();
    with_compiler_stack(move || {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("test.hir", source.clone()).unwrap();
        check_progress(&mut engine, file, &default_context()).unwrap();
        let config = RunConfiguration {
            entry: entry_path(&default_context()),
            hash_seed: 0,
            inputs: vec![],
        };
        let run = run_observed_snapshot(&mut engine, &config, Default::default()).unwrap();
        let o = run.observation.expect("an observation");
        assert!(o.failures.is_empty(), "{source}\n{:?}", o.failures);
        let value = o.returned.expect("a returned value");
        let payload = match &value.nodes[0] {
            ObservedNode::Aggregate { fields, .. } => fields[0].expect("an Err payload"),
            other => panic!("{source}\nreturned {other:?}"),
        };
        match &value.nodes[payload] {
            ObservedNode::Integer(i) => i.bits() as u64,
            other => panic!("{source}\nreturned {other:?}"),
        }
    })
}

/// Lower `body`, expect `built` flat appends in `value`, and agree with the
/// interpreter under H's fault campaign.
fn check(body: &str, built: usize) {
    let source = source(body);
    let (_, lowered) = lower(&source);
    assert_eq!(appends(&lowered), built, "{source}");
    let h = heap(&ready(&source, lowered)).unwrap_or_else(|v| panic!("{source}\n{v}"));
    assert_eq!(
        h,
        reference(&source),
        "{source}\nH disagrees with the interpreter"
    );
}

/// The frozen records-messages/flat construction, reduced.
const BUILT: &str = "fn value() -> u64 {
  let n: u64 = 37
  let mut items: List[Message] = []
  list.reserve(mut items, n)
  for i in 0u64..n {
    list.push(mut items, Message(id: i, kind: i % 7u64, payload: bytes_of(i)))
  }
  let messages = Messages(items: take items)
  digest(messages)
}
";

#[test]
fn records_constructed_for_their_push_are_appended_in_their_encoding() {
    check(BUILT, 1);
    // No element or reservation is left, and the empty list is a builder too.
    check(
        "fn value() -> u64 {
  let mut items: List[Message] = []
  for i in 0u64..0u64 {
    list.push(mut items, Message(id: i, kind: 0, payload: bytes_of(i)))
  }
  digest(Messages(items: take items))
}
",
        1,
    );
}

#[test]
fn several_and_conditional_pushes_of_an_unreserved_list_build_in_place() {
    check(
        "fn value() -> u64 {
  let mut items: List[Message] = []
  for i in 0u64..50u64 {
    if i % 3u64 == 0u64 {
      list.push(mut items, Message(id: i, kind: 1, payload: bytes_of(i)))
    } else if i % 5u64 == 1u64 {
      list.push(mut items, Message(id: i * 2u64, kind: 2, payload: bytes_of(i + 1u64)))
    }
    if i == 41u64 {
      break
    }
  }
  list.push(mut items, Message(id: 99, kind: 3, payload: b\"tail\"))
  let moved = take items
  digest(Messages(items: take moved))
}
",
        3,
    );
}

/// Lists read as lists, elements bound or joined before their push, and lists
/// not taken into a flat field keep their ordinary representation.
#[test]
fn lists_read_as_lists_or_given_other_elements_stay_ordinary() {
    for body in [
        // Read before its take.
        "fn value() -> u64 {
  let mut items: List[Message] = []
  for i in 0u64..9u64 {
    list.push(mut items, Message(id: i, kind: list.len(items), payload: bytes_of(i)))
  }
  digest(Messages(items: take items))
}
",
        // An element bound to a name before its push.
        "fn value() -> u64 {
  let mut items: List[Message] = []
  for i in 0u64..9u64 {
    let m = Message(id: i, kind: 4, payload: bytes_of(i))
    list.push(mut items, take m)
  }
  digest(Messages(items: take items))
}
",
        // An element chosen on two paths.
        "fn value() -> u64 {
  let mut items: List[Message] = []
  for i in 0u64..9u64 {
    list.push(mut items, if i % 2u64 == 0u64 { Message(id: i, kind: 5, payload: bytes_of(i)) } else { Message(id: 0, kind: 6, payload: b\"odd\") })
  }
  digest(Messages(items: take items))
}
",
        // Copied into the field, so it stays readable.
        "fn value() -> u64 {
  let mut items: List[Message] = []
  for i in 0u64..9u64 {
    list.push(mut items, Message(id: i, kind: 7, payload: bytes_of(i)))
  }
  int.wrapping_add(digest(Messages(items: copy items)), list.len(items))
}
",
        // Never taken into a flat field.
        "fn value() -> u64 {
  let mut items: List[Message] = []
  for i in 0u64..9u64 {
    list.push(mut items, Message(id: i, kind: 8, payload: bytes_of(i)))
  }
  let mut s: u64 = 0
  for m in items { s = int.wrapping_add(s, m.id ^ checksum(m.payload)) }
  s
}
",
    ] {
        check(body, 0);
    }
}

/// The C violations of every unit of a Ready program.
fn violations(source: &str, program: &hirsel_heapcheck::Program<Ready>) -> Vec<ViolationKind> {
    let compiled = compile(source);
    let manifest = manifest(&compiled);
    let operations = compiled.operation_registry();
    let inputs = Inputs {
        operations: &operations,
        target: &program.target,
        layouts: &program.layouts,
        manifest: &manifest,
    };
    program
        .units
        .iter()
        .flat_map(|u| hirsel_lir::validate::check(u, &inputs))
        .map(|v| v.kind)
        .collect()
}

/// An append whose construction is not its builder's element record: C
/// rejects it, and H refuses to write it.
#[test]
fn an_append_of_another_record_fails_c_and_h() {
    let source = source(BUILT);
    let (_, lowered) = lower(&source);
    let mut program = ready(&source, lowered);
    assert!(violations(&source, &program).is_empty());
    assert_eq!(heap(&program), Ok(reference(&source)));
    let u = program
        .units
        .iter_mut()
        .find(|u| {
            u.instructions
                .iter()
                .any(|i| matches!(i, Inst::FlatAppend { .. }))
        })
        .expect("an append");
    let construct = u
        .instructions
        .iter()
        .find_map(|i| match i {
            Inst::FlatAppend { construct, .. } => Some(*construct),
            _ => None,
        })
        .unwrap();
    // The taking construction's record: also Flat, but another one.
    let other = u
        .tables
        .constructs
        .iter()
        .map(|c| c.layout)
        .find(|&l| {
            l != u.tables.constructs[construct.index()].layout
                && program.layouts.layouts[l.0 as usize].kind
                    == hirsel_lir::layout_types::LayoutKind::Flat
        })
        .expect("another flat record");
    u.tables.constructs[construct.index()].layout = other;
    let found = violations(&source, &program);
    assert!(
        found
            .iter()
            .any(|k| matches!(k, ViolationKind::Type("flat append"))),
        "{found:?}"
    );
    let h = heap(&program).expect_err("H writes no other record");
    assert!(h.contains("another record"), "{h}");
}

/// A builder taken into a field encoding a list of another record: C
/// rejects the construction, and H refuses to encode it.
#[test]
fn a_builder_of_another_element_fails_c_and_h() {
    let source = format!(
        "{TYPES}flat type Tag {{ code: u64, label: Bytes }}
flat type Tags {{ items: List[Tag] }}
fn value() -> u64 {{
  let mut items: List[Message] = []
  let mut tags: List[Tag] = []
  for i in 0u64..6u64 {{
    list.push(mut items, Message(id: i, kind: 1, payload: bytes_of(i)))
    list.push(mut tags, Tag(code: i, label: bytes_of(i + 3u64)))
  }}
  let t = Tags(items: take tags)
  let mut s: u64 = digest(Messages(items: take items))
  for tag in t.items {{ s = int.wrapping_add(s, tag.code ^ checksum(tag.label)) }}
  s
}}
{ENTRY}"
    );
    let (_, lowered) = lower(&source);
    assert_eq!(appends(&lowered), 2);
    let mut program = ready(&source, lowered);
    assert!(violations(&source, &program).is_empty());
    assert_eq!(heap(&program), Ok(reference(&source)));
    // Swap the records the two taking constructions build: each now encodes
    // the other's builder.
    let u = program
        .units
        .iter_mut()
        .find(|u| {
            u.instructions
                .iter()
                .any(|i| matches!(i, Inst::FlatAppend { .. }))
        })
        .expect("an append");
    let appended: Vec<_> = u
        .instructions
        .iter()
        .filter_map(|i| match i {
            Inst::FlatAppend { construct, .. } => Some(*construct),
            _ => None,
        })
        .collect();
    let takers: Vec<usize> = u
        .tables
        .constructs
        .iter()
        .enumerate()
        .filter(|(i, c)| {
            !appended.iter().any(|a| a.index() == *i)
                && program.layouts.layouts[c.layout.0 as usize].kind
                    == hirsel_lir::layout_types::LayoutKind::Flat
                && program.layouts.layouts[c.layout.0 as usize].fields.len() == 1
        })
        .map(|(i, _)| i)
        .collect();
    let [a, b] = takers[..] else {
        panic!("two taking constructions: {takers:?}");
    };
    let (la, lb) = (u.tables.constructs[a].layout, u.tables.constructs[b].layout);
    u.tables.constructs[a].layout = lb;
    u.tables.constructs[b].layout = la;
    let found = violations(&source, &program);
    assert!(
        found
            .iter()
            .any(|k| matches!(k, ViolationKind::Type("constructed fields"))),
        "{found:?}"
    );
    let h = heap(&program).expect_err("H encodes no other element");
    assert!(h.contains("builder of another encoding"), "{h}");
}

/// A builder's `list.reserve` is a `SeqReserve` of its block: room for that
/// many of its least element, the only sequence write C admits outside the
/// library and on a sequential block. As a `SeqSetLen` C rejects it.
#[test]
fn a_builder_reservation_is_its_only_sequence_write_outside_the_library() {
    let source = source(BUILT);
    let (_, lowered) = lower(&source);
    let mut program = ready(&source, lowered);
    assert!(violations(&source, &program).is_empty());
    let u = program
        .units
        .iter_mut()
        .find(|u| {
            u.instructions
                .iter()
                .any(|i| matches!(i, Inst::FlatAppend { .. }))
        })
        .expect("an append");
    let at = u
        .instructions
        .iter()
        .position(|i| matches!(i, Inst::SeqReserve { .. }))
        .expect("the builder's reservation");
    let Inst::SeqReserve {
        address,
        capacity,
        version,
    } = u.instructions[at]
    else {
        unreachable!()
    };
    u.instructions[at] = Inst::SeqSetLen {
        address,
        len: capacity,
        version,
    };
    let found = violations(&source, &program);
    assert!(
        found
            .iter()
            .any(|k| matches!(k, ViolationKind::Vocabulary(_)))
            && found
                .iter()
                .any(|k| matches!(k, ViolationKind::Type("sequence write"))),
        "{found:?}"
    );
}
