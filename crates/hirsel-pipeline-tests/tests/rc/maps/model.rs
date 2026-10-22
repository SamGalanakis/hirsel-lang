//! Independent SEM-5 oracle: ordered pairs, linear equality, no hashing.
//! Contract: docs/spec/semantics.md at 2359c0f3, lines 204-235.
//! The SipHash13 reference controls already live in SEM-3-siphash-vectors-run;
//! these tests deliberately derive no expectations from a hash implementation.
use std::fmt::Write;

pub const SEEDS: [u64; 4] = [0, 1, 2, 0x9e3779b97f4a7c15];

#[derive(Clone, Copy, Debug)]
pub enum Op {
    Put(usize, u64),
    Get(usize),
    Remove(usize),
    Borrow,
}

#[derive(Clone, Copy, Debug)]
pub enum Keys {
    Scalar,
    Utf8,
    Composite,
}

#[derive(Debug)]
pub struct Trace {
    pub keys: Keys,
    pub ops: Vec<Op>,
}

pub fn traces() -> Vec<Trace> {
    let mut traces = Vec::new();
    for (keys, count) in [(Keys::Scalar, 24), (Keys::Utf8, 4), (Keys::Composite, 4)] {
        for variant in 0..count {
            let rotate = |k| (k + variant / 4) % 8;
            let mut ops = vec![
                Op::Borrow,
                Op::Get(0),
                Op::Remove(0),
                Op::Put(0, 10),
                Op::Put(1, 11),
                Op::Get(0),
                Op::Put(0, 20),
                Op::Remove(1),
                Op::Get(1),
                Op::Put(1, 21),
                Op::Borrow,
                Op::Remove(7),
            ];
            // Stay within eight keys; the existing 32-entry seed control
            // separately crosses the native small-map/index threshold.
            ops.extend((2..8).map(|k| Op::Put(k, 30 + k as u64)));
            ops.extend([Op::Remove(0), Op::Put(0, 40)]);
            let alphabet = [Op::Put(0, 50), Op::Get(0), Op::Remove(0), Op::Borrow];
            // Exhaust every two-operation suffix of this four-operation alphabet.
            ops.extend([alphabet[variant % 4], alphabet[(variant / 4) % 4]]);
            if variant >= 16 {
                // Empty again, then get/remove/consume an empty map.
                ops.extend((0..8).map(Op::Remove));
            }
            for op in &mut ops {
                match op {
                    Op::Put(k, _) | Op::Get(k) | Op::Remove(k) => *k = rotate(*k),
                    Op::Borrow => {}
                }
            }
            // Reserve two operations for shared and unique consuming exits.
            assert!(ops.len() + 2 <= 32);
            traces.push(Trace { keys, ops });
        }
    }
    assert_eq!(traces.len(), 32);
    traces
}

fn key(keys: Keys, k: usize, reverse: bool) -> String {
    const STRINGS: [&str; 8] = ["é", "é", "猫", "🐑", "", "a", "ab", "ß"];
    match keys {
        Keys::Scalar => format!("{k}u64"),
        Keys::Utf8 => format!("\"{}\"", STRINGS[k]),
        Keys::Composite => format!("composite({k}, {reverse})"),
    }
}

fn key_expr(keys: Keys, k: usize) -> String {
    key(keys, k, false)
}

fn value(v: u64) -> String {
    format!("[{v}u64, {}u64]", v + 100)
}

fn answer(v: Option<u64>) -> String {
    v.map_or_else(|| "None".into(), |v| format!("Some({})", value(v)))
}

pub fn source(traces: &[Trace]) -> String {
    // Concrete helpers keep generic proof/tuple validation outside this study.
    // Operations are emitted directly; there is no second state-machine oracle.
    // Only the written key type differs between observation helpers.
    let (keys, helpers) = SUPPORT.split_once("fn visible").unwrap();
    let mut source = keys.to_owned();
    for (ty, family) in [
        ("u64", "scalar"),
        ("String", "utf8"),
        ("Set[String]", "composite"),
    ] {
        let mut helper = format!("fn visible{helpers}")
            .replace("[K: key]", "")
            .replace('K', ty);
        for name in ["visible", "consume", "borrowed_first"] {
            helper = helper.replace(&format!("{name}("), &format!("{name}_{family}("));
        }
        source.push_str(&helper);
    }
    for (id, trace) in traces.iter().enumerate() {
        let ty = match trace.keys {
            Keys::Scalar => "u64",
            Keys::Utf8 => "String",
            Keys::Composite => "Set[String]",
        };
        let family = match trace.keys {
            Keys::Scalar => "scalar",
            Keys::Utf8 => "utf8",
            Keys::Composite => "composite",
        };
        let mut model: Vec<(usize, u64)> = Vec::new();
        writeln!(
            source,
            "pub fn trace{id}() -> u64 {{\n let mut m: unique Map[{ty},List[u64]] = map.empty()"
        )
        .unwrap();
        let mut state = "[]".to_owned();
        for (step, op) in trace.ops.iter().enumerate() {
            let (tag, k, v, result) = match *op {
                Op::Put(k, v) => {
                    if let Some(pair) = model.iter_mut().find(|pair| pair.0 == k) {
                        pair.1 = v;
                    } else {
                        model.push((k, v));
                    }
                    (0, k, v, Some(v))
                }
                Op::Get(k) => (1, k, 0, model.iter().find(|p| p.0 == k).map(|p| p.1)),
                Op::Remove(k) => {
                    let removed = model
                        .iter()
                        .position(|p| p.0 == k)
                        .map(|i| model.remove(i).1);
                    (2, k, 0, removed)
                }
                Op::Borrow => (3, 0, 0, model.first().map(|p| p.1)),
            };
            let code = (id + 1) * 1000 + (step + 1) * 10;
            let key = key(trace.keys, k, step % 2 == 0);
            let expected = answer(result);
            match tag {
                0 => writeln!(source, "map.insert(mut m, unique {key}, {})\n if map.get(m,{key}) != {expected} {{ return {} }}", value(v),code+1).unwrap(),
                1 => writeln!(source, "if map.get(m,{key}) != {expected} {{ return {} }}",code+2).unwrap(),
                2 => writeln!(source, "m = unique m\n if map.remove(mut m,{key}) != {expected} {{ return {} }}",code+3).unwrap(),
                _ => writeln!(source, "if borrowed_first_{family}(m) != {expected} {{ return {} }}",code+4).unwrap(),
            }
            state = format!(
                "[{}]",
                model
                    .iter()
                    .map(|&(k, v)| format!("({}, {})", key_expr(trace.keys, k), value(v)))
                    .collect::<Vec<_>>()
                    .join(",")
            );
            writeln!(
                source,
                "if !visible_{family}(m,{state}) {{ return {} }}",
                code + 5
            )
            .unwrap();
        }
        let first = model.first().map_or_else(
            || "None".to_owned(),
            |&(k, v)| format!("Some(({},{}))", key_expr(trace.keys, k), value(v)),
        );
        let code = (id + 1) * 1000 + trace.ops.len() * 10;
        writeln!(source,"let expected: Option[({ty},List[u64])] = {first}\n let saved = copy m\n if consume_{family}(take m) != expected {{ return {} }}\n if !visible_{family}(saved,{state}) {{ return {} }}\n if consume_{family}(take saved) != expected {{ return {} }}\n 0\n}}",code+6,code+7,code+8).unwrap();
    }

    source.push_str("pub fn main() -> Result[(),u64] {\n");
    for id in 0..traces.len() {
        writeln!(
            source,
            "let r{id} = trace{id}()\n if r{id} != 0 {{ return Error(r{id}) }}"
        )
        .unwrap();
    }
    source.push_str("Ok(())\n}\n");
    source
}

/// Every check is made before proceeding. Code = trace*1000 + prefix*10 + check;
/// deleting the suffix after that prefix deterministically shrinks a failure.
pub fn failure(code: u64, traces: &[Trace]) -> String {
    let trace = (code / 1000).saturating_sub(1) as usize;
    let prefix = ((code % 1000) / 10) as usize;
    format!(
        "code={code}, trace={trace}, shortest failing prefix={prefix}, check={}, {:?}",
        code % 10,
        &traces[trace].ops[..prefix.min(traces[trace].ops.len())]
    )
}

const SUPPORT: &str = r#"
fn composite(n: u64, reverse: bool) -> Set[String] {
  let s = match n { 0 => "é", 1 => "é", 2 => "猫", 3 => "🐑", 4 => "", 5 => "a", 6 => "ab", _ => "ß" }
  let mut key: Set[String] = set.empty()
  if reverse { set.insert(mut key, take s); set.insert(mut key, "anchor") }
  else { set.insert(mut key, "anchor"); set.insert(mut key, take s) }
  take key
}
fn visible[K: key](m: Map[K,List[u64]], take expected: List[(K,List[u64])]) -> bool {
  if map.len(m) != list.len(expected) { return false }
  let mut i: u64 = 0
  for (k,v) in m {
    let (expected_key,expected_value) = expected[i]
    if k != expected_key || v != expected_value { return false }
    i += 1
  }
  let mut forward: Map[K,List[u64]] = map.empty()
  let mut reverse: Map[K,List[u64]] = map.empty()
  for (k,v) in expected { map.insert(mut forward, copy k, copy v) }
  let mut j = list.len(expected)
  while j > 0 { j -= 1; let (k,v) = expected[j]; map.insert(mut reverse, copy k, copy v) }
  m == forward && std@1/map.same_entries(m, reverse) && ((m == reverse) == (i <= 1))
}
fn consume[K: key](take m: Map[K,List[u64]]) -> Option[(K,List[u64])] {
  let mut first: Option[(K,List[u64])] = None
  for (k,v) in take m { first = Some((take k,take v)); break }
  take first
}
fn borrowed_first[K: key](m: Map[K,List[u64]]) -> Option[List[u64]] {
  let mut first: Option[List[u64]] = None
  for (_,value) in m { first = Some(unique value); break }
  take first
}

"#;
