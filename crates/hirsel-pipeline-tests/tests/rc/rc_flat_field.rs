//! Byte fields built in place (MEM-12, LIR schema revision 24): a byte list
//! only reserved and pushed, then frozen whole into a Bytes field of a flat
//! list builder's element, is written where that element will lie
//! (`Inst::FlatField`, completed by `Inst::FlatClose`); other byte lists stay
//! ordinary, and C and H reject a builder written while an element is open.
use crate::rc_flat_builder::{heap, ready, reference, violations};
use crate::rc_support::*;

use hirsel_lir::ir::{FlatFieldOp, Inst};
use hirsel_lir::validate::ViolationKind;
use hirsel_rc::import::Lowered;

/// `main` returns `value()` as its `Err` payload, so both executors report it.
const ENTRY: &str = "pub fn main() -> Result[(), u64] {\n  Error(value())\n}\n";

const TYPES: &str = "flat type Message { id: u64, kind: u64, payload: Bytes }
flat type Messages { items: List[Message] }
flat type Tailed { id: u64, payload: Bytes, tail: u64, name: String }
flat type Tails { items: List[Tailed] }
flat type Tagged { tags: List[u64], payload: Bytes }
flat type Tags { items: List[Tagged] }
flat type Pair { id: u64, first: Bytes, second: Bytes }
flat type Pairs { items: List[Pair] }
flat type Other { id: u64, kind: u64, payload: Bytes }
flat type Others { items: List[Other] }
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
";

fn source(body: &str) -> String {
    format!("{TYPES}{body}{ENTRY}")
}

/// `value`'s open-element writes: opens, reservations, pushes and closes.
fn writes(lowered: &Lowered) -> [usize; 4] {
    let mut n = [0; 4];
    for i in &unit_named(lowered, "value").instructions {
        match i {
            Inst::FlatField { op, .. } => match op {
                FlatFieldOp::Open => n[0] += 1,
                FlatFieldOp::Reserve { .. } => n[1] += 1,
                FlatFieldOp::Push { .. } => n[2] += 1,
            },
            Inst::FlatClose { .. } => n[3] += 1,
            _ => {}
        }
    }
    n
}

/// Lower `body`, expect `built` open-element writes in `value`, and agree
/// with the interpreter under H's fault campaign with C accepting every
/// stage.
fn check(body: &str, built: [usize; 4]) {
    let source = source(body);
    let (_, lowered) = lower(&source);
    assert_eq!(writes(&lowered), built, "{source}");
    let h = heap(&ready(&source, lowered)).unwrap_or_else(|v| panic!("{source}\n{v}"));
    assert_eq!(
        h,
        reference(&source),
        "{source}\nH disagrees with the interpreter"
    );
}

/// records-messages/flat's construction, at a size H's fault campaign
/// covers quickly.
const BUILT: &str = "fn value() -> u64 {
  let n: u64 = 12
  let mut items: List[Message] = []
  list.reserve(mut items, n)
  for i in 0u64..n {
    let mut payload: List[u8] = []
    let length = 3u64 + i % 11u64
    list.reserve(mut payload, length)
    for j in 0u64..length { list.push(mut payload, int.narrow(97u64 + (i + j * 7u64) % 26u64)) }
    list.push(mut items, Message(id: i, kind: i % 7u64, payload: bytes.from_list(take payload)))
  }
  digest(Messages(items: take items))
}
";

#[test]
fn a_byte_list_frozen_into_a_built_element_is_written_in_place() {
    check(BUILT, [1, 1, 1, 1]);
    // Without a reservation, through growth, empty, and conditional pushes.
    check(
        "fn value() -> u64 {
  let mut items: List[Message] = []
  for i in 0u64..12u64 {
    let mut payload: List[u8] = []
    for j in 0u64..i * 3u64 {
      if j % 3u64 != 1u64 {
        list.push(mut payload, int.narrow(65u64 + j % 50u64))
      }
    }
    list.push(mut items, Message(id: i, kind: 2, payload: bytes.from_list(take payload)))
  }
  digest(Messages(items: take items))
}
",
        [1, 0, 1, 1],
    );
    // A reservation smaller than the pushes, a moved list and Bytes.
    check(
        "fn value() -> u64 {
  let mut items: List[Message] = []
  for i in 0u64..14u64 {
    let mut payload: List[u8] = []
    list.reserve(mut payload, 2u64)
    for j in 0u64..i { list.push(mut payload, int.narrow(j)) }
    let moved = take payload
    let frozen = bytes.from_list(take moved)
    list.push(mut items, Message(id: i, kind: 3, payload: take frozen))
  }
  digest(Messages(items: take items))
}
",
        [1, 1, 1, 1],
    );
}

/// Whether each of `value`'s open field pushes is `spare`.
fn spares(lowered: &Lowered) -> Vec<bool> {
    unit_named(lowered, "value")
        .instructions
        .iter()
        .filter_map(|i| match i {
            Inst::FlatField {
                op: FlatFieldOp::Push { spare, .. },
                ..
            } => Some(*spare),
            _ => None,
        })
        .collect()
}

/// `spare_push::prove_fields` marks records-messages/flat's push spare: it
/// runs once per index of a counted loop below the field's reservation
/// (FACT-3). Without a reservation, or with one smaller than the loop, the
/// push keeps its capacity test.
#[test]
fn a_push_per_index_below_the_field_reservation_is_spare() {
    let (_, lowered) = lower(&source(BUILT));
    assert_eq!(spares(&lowered), [true]);
    for body in [
        "fn value() -> u64 {
  let mut items: List[Message] = []
  for i in 0u64..4u64 {
    let mut payload: List[u8] = []
    for j in 0u64..i { list.push(mut payload, int.narrow(j)) }
    list.push(mut items, Message(id: i, kind: 1, payload: bytes.from_list(take payload)))
  }
  digest(Messages(items: take items))
}
",
        "fn value() -> u64 {
  let mut items: List[Message] = []
  for i in 0u64..4u64 {
    let mut payload: List[u8] = []
    list.reserve(mut payload, i)
    for j in 0u64..i + 1u64 { list.push(mut payload, int.narrow(j)) }
    list.push(mut items, Message(id: i, kind: 1, payload: bytes.from_list(take payload)))
  }
  digest(Messages(items: take items))
}
",
    ] {
        let (_, lowered) = lower(&source(body));
        assert_eq!(spares(&lowered), [false], "{body}");
    }
}

/// A field followed by fixed and variable fields completes its element
/// around the bytes in place, growing for the rest.
#[test]
fn fields_after_the_open_one_are_written_around_its_bytes() {
    check(
        "fn value() -> u64 {
  let mut items: List[Tailed] = []
  for i in 0u64..10u64 {
    let mut payload: List[u8] = []
    for j in 0u64..i * 3u64 { list.push(mut payload, int.narrow(j % 256u64)) }
    list.push(mut items, Tailed(id: i, payload: bytes.from_list(take payload), tail: i * 3u64, name: \"tail name\"))
  }
  let tails = Tails(items: take items)
  let mut s: u64 = 0
  for t in tails.items {
    s = int.wrapping_add(s ^ t.tail, t.id ^ checksum(t.payload) ^ string.byte_len(t.name))
  }
  s
}
",
        [1, 0, 1, 1],
    );
}

/// Byte lists read as lists, after a variable field, beside an open element,
/// or able to skip their element stay ordinary.
#[test]
fn byte_lists_read_or_not_written_whole_into_one_element_stay_ordinary() {
    for (body, built) in [
        // Read as a list before its freeze.
        (
            "fn value() -> u64 {
  let mut items: List[Message] = []
  for i in 0u64..9u64 {
    let mut payload: List[u8] = []
    list.push(mut payload, int.narrow(i))
    let kind = list.len(payload)
    list.push(mut items, Message(id: i, kind: kind, payload: bytes.from_list(take payload)))
  }
  digest(Messages(items: take items))
}
",
            [0, 0, 0, 0],
        ),
        // After a variable field, so at no fixed offset.
        (
            "fn value() -> u64 {
  let mut items: List[Tagged] = []
  for i in 0u64..9u64 {
    let mut payload: List[u8] = []
    list.push(mut payload, int.narrow(i))
    list.push(mut items, Tagged(tags: [i], payload: bytes.from_list(take payload)))
  }
  let tags = Tags(items: take items)
  let mut s: u64 = 0
  for t in tags.items { s = int.wrapping_add(s ^ t.tags[0], checksum(t.payload)) }
  s
}
",
            [0, 0, 0, 0],
        ),
        // A path that skips the element, so the next list would open again.
        (
            "fn value() -> u64 {
  let mut items: List[Message] = []
  for i in 0u64..9u64 {
    let mut payload: List[u8] = []
    list.push(mut payload, int.narrow(i))
    if i % 4u64 == 1u64 {
      continue
    }
    list.push(mut items, Message(id: i, kind: 4, payload: bytes.from_list(take payload)))
  }
  digest(Messages(items: take items))
}
",
            [0, 0, 0, 0],
        ),
        // Another element appended while the list is being built.
        (
            "fn value() -> u64 {
  let mut items: List[Message] = []
  for i in 0u64..9u64 {
    let mut payload: List[u8] = []
    list.push(mut payload, int.narrow(i))
    list.push(mut items, Message(id: 100u64 + i, kind: 5, payload: b\"between\"))
    list.push(mut items, Message(id: i, kind: 5, payload: bytes.from_list(take payload)))
  }
  digest(Messages(items: take items))
}
",
            [0, 0, 0, 0],
        ),
    ] {
        check(body, built);
    }
    // Of two byte fields only the first follows fixed fields alone.
    check(
        "fn value() -> u64 {
  let mut items: List[Pair] = []
  for i in 0u64..12u64 {
    let mut first: List[u8] = []
    let mut second: List[u8] = []
    for j in 0u64..i { list.push(mut first, int.narrow(j)) }
    list.push(mut second, int.narrow(i))
    list.push(mut items, Pair(id: i, first: bytes.from_list(take first), second: bytes.from_list(take second)))
  }
  let pairs = Pairs(items: take items)
  let mut s: u64 = 0
  for p in pairs.items { s = int.wrapping_add(s ^ p.id, checksum(p.first) ^ checksum(p.second)) }
  s
}
",
        [1, 0, 1, 1],
    );
}

/// `body` at Ready, with one instruction of `value` rewritten.
fn forged(
    body: &str,
    rewrite: impl Fn(&Inst) -> Option<Inst>,
) -> (String, hirsel_heapcheck::Program<hirsel_lir::stage::Ready>) {
    let source = source(body);
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
                .any(|i| matches!(i, Inst::FlatClose { .. }))
        })
        .expect("a close");
    let at = u
        .instructions
        .iter()
        .position(|i| rewrite(i).is_some())
        .expect("the instruction to forge");
    u.instructions[at] = rewrite(&u.instructions[at]).unwrap();
    (source, program)
}

/// A builder reservation where the open field's was: a write that may move
/// the builder's block keeping only its used bytes. C finds the pushes and
/// the close without their open element, and H refuses the write.
#[test]
fn a_foreign_builder_write_inside_an_open_element_fails_c_and_h() {
    let (source, program) = forged(BUILT, |i| match *i {
        Inst::FlatField {
            address,
            op: FlatFieldOp::Reserve { capacity },
            version,
            ..
        } => Some(Inst::SeqReserve {
            address,
            capacity,
            version,
        }),
        _ => None,
    });
    let found = violations(&source, &program);
    assert!(
        found
            .iter()
            .any(|k| matches!(k, ViolationKind::Type("flat field of no open element"))),
        "{found:?}"
    );
    let h = heap(&program).expect_err("H writes no builder with an open element");
    assert!(
        h.contains("a write into a builder with an open element"),
        "{h}"
    );
}

/// A close naming another field than the open one: C finds no element open
/// at it and the site's fields mismatched, and H refuses it.
#[test]
fn a_close_of_another_field_fails_c_and_h() {
    let (source, program) = forged(BUILT, |i| match *i {
        Inst::FlatClose {
            address,
            construct,
            version,
            ..
        } => Some(Inst::FlatClose {
            address,
            construct,
            field: 0,
            version,
        }),
        _ => None,
    });
    let found = violations(&source, &program);
    assert!(
        found
            .iter()
            .any(|k| matches!(k, ViolationKind::Type("flat close"))),
        "{found:?}"
    );
    let h = heap(&program).expect_err("H closes no other field");
    assert!(h.contains("a write of another open field"), "{h}");
}

/// A close completing another record of the same fields, the element of
/// another builder whose values reach it: of C's rules only the close's
/// typing sees that its construction is not its builder's content record,
/// and H refuses it.
#[test]
fn a_close_of_another_record_fails_c_and_h() {
    let source = source(
        "fn value() -> u64 {
  let mut items: List[Message] = []
  let mut others: List[Other] = []
  for i in 0u64..4u64 {
    let mut first: List[u8] = []
    list.push(mut first, int.narrow(i))
    list.push(mut others, Other(id: i, kind: 1, payload: bytes.from_list(take first)))
    let mut second: List[u8] = []
    list.push(mut second, int.narrow(i))
    list.push(mut items, Message(id: i, kind: 1, payload: bytes.from_list(take second)))
  }
  let o = Others(items: take others)
  int.wrapping_add(digest(Messages(items: take items)), list.len(o.items))
}
",
    );
    let (_, lowered) = lower(&source);
    let mut program = ready(&source, lowered);
    assert!(violations(&source, &program).is_empty());
    let u = program
        .units
        .iter_mut()
        .find(|u| {
            u.instructions
                .iter()
                .any(|i| matches!(i, Inst::FlatClose { .. }))
        })
        .expect("a close");
    let closes: Vec<usize> = (0..u.instructions.len())
        .filter(|&i| matches!(u.instructions[i], Inst::FlatClose { .. }))
        .collect();
    let [other, message] = closes[..] else {
        panic!("two closes: {closes:?}");
    };
    let Inst::FlatClose { construct, .. } = u.instructions[other] else {
        unreachable!()
    };
    if let Inst::FlatClose { construct: c, .. } = &mut u.instructions[message] {
        *c = construct;
    }
    let found = violations(&source, &program);
    assert_eq!(found, [ViolationKind::Type("flat close")]);
    let h = heap(&program).expect_err("H closes no element of another record");
    assert!(h.contains("a flat close of another record"), "{h}");
}

/// A reservation rewritten into a second opening: C and H refuse to open
/// an element in a builder that has one open.
#[test]
fn an_element_opened_twice_fails_c_and_h() {
    let (source, program) = forged(BUILT, |i| match *i {
        Inst::FlatField {
            address,
            field,
            op: FlatFieldOp::Reserve { .. },
            version,
        } => Some(Inst::FlatField {
            address,
            field,
            op: FlatFieldOp::Open,
            version,
        }),
        _ => None,
    });
    let found = violations(&source, &program);
    assert!(
        found
            .iter()
            .any(|k| matches!(k, ViolationKind::Type("flat field opened twice"))),
        "{found:?}"
    );
    let h = heap(&program).expect_err("H opens no element twice");
    assert!(
        h.contains("an element opened in a builder with one open"),
        "{h}"
    );
}

/// A push of the reservation's u64 length instead of a byte: C's typing of
/// an open element's writes refuses a pushed value that is not a u8.
#[test]
fn a_push_of_a_wider_value_fails_c() {
    let source = source(BUILT);
    let (_, lowered) = lower(&source);
    let mut program = ready(&source, lowered);
    let u = program
        .units
        .iter_mut()
        .find(|u| {
            u.instructions
                .iter()
                .any(|i| matches!(i, Inst::FlatClose { .. }))
        })
        .expect("a close");
    let length = u
        .instructions
        .iter()
        .find_map(|i| match *i {
            Inst::FlatField {
                op: FlatFieldOp::Reserve { capacity },
                ..
            } => Some(capacity),
            _ => None,
        })
        .expect("the field's reservation");
    let push = u
        .instructions
        .iter_mut()
        .find_map(|i| match i {
            Inst::FlatField {
                op: FlatFieldOp::Push { value, .. },
                ..
            } => Some(value),
            _ => None,
        })
        .expect("the field's push");
    *push = length;
    let found = violations(&source, &program);
    assert!(
        found
            .iter()
            .any(|k| matches!(k, ViolationKind::Type("flat field"))),
        "{found:?}"
    );
}

/// Pushes without a reservation marked spare: C cannot see the room, as
/// for a library spare push, and H fails the first push past it.
#[test]
fn a_spare_push_past_the_room_fails_in_h() {
    let (source, program) = forged(
        "fn value() -> u64 {
  let mut items: List[Message] = []
  for i in 0u64..3u64 {
    let mut payload: List[u8] = []
    for j in 0u64..i * 100u64 { list.push(mut payload, int.narrow(j % 200u64)) }
    list.push(mut items, Message(id: i, kind: 6, payload: bytes.from_list(take payload)))
  }
  digest(Messages(items: take items))
}
",
        |i| match *i {
            Inst::FlatField {
                address,
                field,
                op:
                    FlatFieldOp::Push {
                        value,
                        spare: false,
                    },
                version,
            } => Some(Inst::FlatField {
                address,
                field,
                op: FlatFieldOp::Push { value, spare: true },
                version,
            }),
            _ => None,
        },
    );
    assert!(violations(&source, &program).is_empty());
    let h = heap(&program).expect_err("H fails a spare push past the room");
    assert!(
        h.contains("a spare push past the builder's capacity"),
        "{h}"
    );
}
