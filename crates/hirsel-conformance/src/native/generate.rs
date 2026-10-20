//! Seeded programs for the differential gate: each is valid by
//! construction, prints computed values (the interpreter is the oracle),
//! and exercises every integer and float width, records, variants and
//! `match`, an inline array read, a generic function, a closure with a
//! capture, a self-tail loop and a resource whose finalizer prints.
//!
//! A directed failure adds one statement that must fail, after some output
//! and with a live resource, so the failure's rule and site, the earlier
//! output and the finalizer's output are all compared.

/// The statement a directed failure adds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Fault {
    /// SEM-1: an unsigned addition past the type's maximum.
    Overflow,
    /// SEM-1: an unsigned subtraction below zero.
    Underflow,
    /// SEM-1: a division by zero.
    DivideByZero,
    /// MEM-14: an inline array index equal to its length.
    Index,
    /// FACT-1: a call that violates its `requires`.
    Requires,
}

pub const FAULTS: [Fault; 5] = [
    Fault::Overflow,
    Fault::Underflow,
    Fault::DivideByZero,
    Fault::Index,
    Fault::Requires,
];

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // splitmix64
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// A bounded expression over the u64 names in `names`: every operand is
/// reduced below 1000 before a multiplication and every divisor is nonzero,
/// so no arithmetic fails.
fn expression(rng: &mut Rng, names: &[String], depth: u32) -> String {
    let leaf = |rng: &mut Rng| {
        if names.is_empty() || rng.below(3) == 0 {
            format!("{}u64", rng.below(900))
        } else {
            names[rng.below(names.len() as u64) as usize].clone()
        }
    };
    if depth == 0 {
        return leaf(rng);
    }
    let a = expression(rng, names, depth - 1);
    let b = expression(rng, names, depth - 1);
    match rng.below(6) {
        0 => format!("({a} % 1000u64 + {b} % 1000u64)"),
        1 => format!("({a} % 1000u64) * ({b} % 1000u64)"),
        2 => format!("{a} / ({b} % 7u64 + 1u64)"),
        3 => format!("{a} % ({b} % 5u64 + 2u64)"),
        4 => format!("(if {a} < {b} {{ {b} % 1000u64 }} else {{ {a} % 1000u64 + 1u64 }})"),
        _ => leaf(rng),
    }
}

/// The program of `seed`, failing by `fault` when one is given.
pub fn program(seed: u64, fault: Option<Fault>) -> String {
    let mut rng = Rng(seed.wrapping_mul(0x2545_f491_4f6c_dd1d) ^ 0x6a09_e667_f3bc_c908);
    let mut text = String::from(
        "type Pair { x: u64, y: u64 }
type Fig = Dot | Line(len: u64) | Rect(w: u64, h: u64)
resource type Guard { out: Stdout, id: u64 }
drop(g) {
  let _ = out.print(g.out, if g.id % 2u64 == 0u64 { \"drop even\\n\" } else { \"drop odd\\n\" })
}
fn area(s: Fig) -> u64 {
  match s {
    Dot => 0u64,
    Line(len: n) => n,
    Rect(w: w, h: h) => (w % 1000u64) * (h % 1000u64),
  }
}
fn keep[T: copyable](take a: T) -> T {
  a
}
fn sum_to(i: u64, n: u64, acc: u64) -> u64 {
  if i == n { acc } else { sum_to(i + 1u64, n, acc + i * i) }
}
fn small(a: u64) -> u64
  requires a < 1000u64
{
  a + 1u64
}
",
    );
    let helpers = 1 + rng.below(3);
    for h in 0..helpers {
        let names = vec!["a".to_string(), "b".to_string()];
        let body = expression(&mut rng, &names, 2);
        text.push_str(&format!(
            "fn f{h}(a: u64, b: u64) -> u64 {{\n  {body}\n}}\n"
        ));
    }
    for signed in [false, true] {
        for width in [8, 16, 32, 64, 128] {
            let ty = format!("{}{width}", if signed { 'i' } else { 'u' });
            text.push_str(&format!(
                "fn calc_{ty}(x: {ty}) -> {ty} {{ ((x >> 1{ty}) ^ 3{ty}) % 97{ty} + 2{ty} }}\n"
            ));
            let negative = if signed {
                format!("n <= -10{ty} || ")
            } else {
                String::new()
            };
            let digit = if signed {
                format!("if r < 0{ty} {{ -r }} else {{ r }}")
            } else {
                "r".to_owned()
            };
            text.push_str(&format!(
                "fn digits_{ty}(s: Stdout, n: {ty}) {{\n  if {negative}n >= 10{ty} {{ digits_{ty}(s, n / 10{ty}) }}\n  let r = n % 10{ty}\n  let d = {digit}\n  let _ = out.print(s, match d {{ 0{ty} => \"0\", 1{ty} => \"1\", 2{ty} => \"2\", 3{ty} => \"3\", 4{ty} => \"4\", 5{ty} => \"5\", 6{ty} => \"6\", 7{ty} => \"7\", 8{ty} => \"8\", _ => \"9\" }})\n}}\n"
            ));
            let sign = if signed {
                format!("  if n < 0{ty} {{ let _ = out.print(s, \"-\") }}\n")
            } else {
                String::new()
            };
            text.push_str(&format!(
                "fn print_{ty}(s: Stdout, n: {ty}) {{\n{sign}  digits_{ty}(s, n)\n}}\n"
            ));
        }
    }
    for ty in ["f32", "f64"] {
        text.push_str(&format!(
            "fn calc_{ty}(x: {ty}) -> {ty} {{ (x * 0.5{ty} + 0.25{ty}) / 2.0{ty} }}\n"
        ));
    }
    text.push_str(
        "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {\n  let s = io.stdout(rt)?\n  let salt = arg.len(args) % 2u64\n",
    );
    text.push_str(&format!(
        "  let g = Guard(out: s, id: {}u64)\n",
        rng.below(100)
    ));
    // B2: parity-only output hides wrong answers that keep the same low bit.
    // An admitted Args observation keeps native operands dynamic through O2,
    // even when the harness supplies the empty argument snapshot.
    for signed in [false, true] {
        for width in [8, 16, 32, 64, 128] {
            let ty = format!("{}{width}", if signed { 'i' } else { 'u' });
            let bits = ((rng.next() as u128) << 64) | rng.next() as u128;
            let value = if signed {
                let maximum = (1u128 << (width - 1)) - 1;
                let magnitude = bits & maximum;
                if rng.below(2) == 0 {
                    format!("-{magnitude}{ty}")
                } else {
                    format!("{magnitude}{ty}")
                }
            } else {
                let mask = u128::MAX >> (128 - width);
                format!("{}{ty}", bits & mask)
            };
            let value = match seed % 4 {
                0 if signed => format!("-{}{ty}", 1u128 << (width - 1)),
                0 => format!("{}{ty}", u128::MAX >> (128 - width)),
                1 if signed => format!("{}{ty}", (1u128 << (width - 1)) - 1),
                1 => format!("0{ty}"),
                _ => value,
            };
            text.push_str(&format!(
                "  let x_{ty}: {ty} = {value} ^ (if salt == 0u64 {{ 0{ty} }} else {{ 1{ty} }})\n  print_{ty}(s, x_{ty})\n  let _ = out.print(s, \" \")\n  let y_{ty} = calc_{ty}(x_{ty})\n  print_{ty}(s, y_{ty})\n  let _ = out.print(s, \"\\n\")\n"
            ));
        }
    }
    for ty in ["f32", "f64"] {
        // Dyadic operands make exact, finite float arithmetic at both widths.
        // The f64 fraction above 2^24 cannot survive accidental f32 narrowing.
        let value = rng.below(10000) + if ty == "f64" { 1 << 24 } else { 0 };
        text.push_str(&format!(
            "  let x_{ty}: {ty} = {value}.25{ty} + (if salt == 0u64 {{ 0.0{ty} }} else {{ 0.5{ty} }})\n  print_u64(s, int.widen({ty}.bits(calc_{ty}(x_{ty}))))\n  let _ = out.print(s, \"\\n\")\n"
        ));
    }
    let mut names: Vec<String> = Vec::new();
    let statements = 4 + rng.below(5);
    let fault_at = rng.below(statements);
    for i in 0..statements {
        let name = format!("v{i}");
        let value = expression(&mut rng, &names, 1);
        let line = match rng.below(7) {
            0 => format!(
                "  let {name} = f{}({value}, {}u64)\n",
                rng.below(helpers),
                rng.below(50)
            ),
            1 => format!(
                "  let p{i} = Pair(x: {value}, y: {}u64)\n  let {name} = p{i}.x % 1000u64 + p{i}.y\n",
                rng.below(100)
            ),
            2 => {
                let fig = match rng.below(3) {
                    0 => "Dot".to_string(),
                    1 => format!("Line(len: {value})"),
                    _ => format!("Rect(w: {value}, h: {}u64)", rng.below(30)),
                };
                format!("  let {name} = area({fig})\n")
            }
            3 => format!(
                "  let xs{i}: Inline[u64, 4] = [{value}, {}u64, {}u64, 9u64]\n  let i{i}: u64 = {value} % 4u64\n  let {name} = if xs{i}[i{i}] == 9u64 {{ 1u64 }} else {{ 2u64 }}\n",
                rng.below(9),
                rng.below(9)
            ),
            4 => format!(
                "  let c{i}: u64 = {value}\n  let add{i} = fn(x: u64) -> u64 {{ x % 1000u64 + c{i} % 1000u64 }}\n  let {name} = add{i}({}u64)\n",
                rng.below(500)
            ),
            5 => format!("  let {name} = sum_to(0u64, {}u64, 0u64)\n", rng.below(12)),
            _ => format!("  let k{i}: u64 = {value}\n  let {name} = keep(take k{i})\n"),
        };
        text.push_str(&line);
        text.push_str(&format!(
            "  print_u64(s, {name})\n  let _ = out.print(s, \"\\n\")\n"
        ));
        names.push(name.clone());
        let other = &names[rng.below(names.len() as u64) as usize];
        text.push_str(&match rng.below(3) {
            0 => format!(
                "  let _ = out.print(s, if {name} % 2u64 == 0u64 {{ \"{i} even\\n\" }} else {{ \"{i} odd\\n\" }})\n"
            ),
            1 => format!(
                "  let _ = out.print(s, if {name} < {other} + {}u64 {{ \"{i} lt\\n\" }} else {{ \"{i} ge\\n\" }})\n",
                rng.below(40)
            ),
            _ => String::new(),
        });
        if let (Some(fault), true) = (fault, i == fault_at) {
            // `{name} - {name}` is zero, but only at run time.
            text.push_str(&match fault {
                Fault::Overflow => format!(
                    "  let big: u64 = 18446744073709551615u64\n  let bad = big + ({name} - {name} + 1u64)\n"
                ),
                Fault::Underflow => format!("  let bad = {name} - {name} - 1u64\n"),
                Fault::DivideByZero => format!("  let bad = 7u64 / ({name} - {name})\n"),
                Fault::Index => format!(
                    "  let ys: Inline[u64, 3] = [1u64, 2u64, 3u64]\n  let at: u64 = {name} - {name} + 3u64\n  let bad = if ys[at] == 1u64 {{ 1u64 }} else {{ 2u64 }}\n"
                ),
                Fault::Requires => format!("  let bad = small({name} - {name} + 1000u64)\n"),
            });
            text.push_str("  let _ = out.print(s, \"after the failure\\n\")\n");
        }
    }
    text.push_str("  let _ = out.print(s, \"end\")\n  Ok(())\n}\n");
    text
}

/// A fresh, reproducible window per campaign seed; IDs retain the program seed
/// so a single generated witness can be selected and replayed.
pub fn cases(campaign_seed: u64, count: u64) -> Vec<super::Case> {
    let mut cases = Vec::new();
    for offset in 0..count {
        let seed = campaign_seed.wrapping_mul(64).wrapping_add(offset);
        cases.push(super::Case {
            path: None,
            id: format!("generated/success-{seed}"),
            source: program(seed, None),
        });
        let fault = FAULTS[(seed % FAULTS.len() as u64) as usize];
        cases.push(super::Case {
            path: None,
            id: format!("generated/{fault:?}-{seed}").to_lowercase(),
            source: program(seed, Some(fault)),
        });
    }
    cases
}

/// Reconstruct one generated witness independently of the current campaign window.
pub fn from_id(id: &str) -> Option<super::Case> {
    let (kind, seed) = id.strip_prefix("generated/")?.rsplit_once('-')?;
    let seed: u64 = seed.parse().ok()?;
    if kind == "bug" {
        return Some(super::Case {
            id: id.into(),
            source: bug_program(seed),
            path: None,
        });
    }
    let fault = if kind == "success" {
        None
    } else {
        Some(
            *FAULTS
                .iter()
                .find(|fault| format!("{fault:?}").to_lowercase() == kind)?,
        )
    };
    Some(super::Case {
        path: None,
        id: id.into(),
        source: program(seed, fault),
    })
}

/// GAP 4--8 cross product: three mutable places (local, field, nested field), replace/take, and three
/// aggregate contexts. Every seed also exercises borrowed/destructured literal
/// matches, let-else payloads, const Inline reads, try arms and mut-field loops.
/// The loop's collection follows the place. Seed modulo 18 enumerates the product.
/// Known failures of the bug-seeded generator, one exclusion row per
/// combination and configuration; stale and changed rows fail like any other.
pub const BUG_PENDING: &str = "native-tests/regressions/bug-pending.tsv";

pub fn bug_program(seed: u64) -> String {
    let place = (seed % 3) as usize;
    let operation = (seed / 3) % 2;
    let context = (seed / 6) % 3;
    let setup = [
        "let mut slot = b\"left\"",
        "let mut holder = Pair(left: b\"left\", right: b\"unused\")",
        "let mut outer = Outer(inner: Pair(left: b\"left\", right: b\"unused\"))",
    ][place];
    let slot = ["slot", "holder.left", "outer.inner.left"][place];
    // `take` moves only a whole local (OWN-8); a field moves out by `unique replace`.
    let read = match (operation, place) {
        (0, _) => format!("replace(mut {slot}, b\"next\")"),
        (_, 0) => format!("take {slot}"),
        _ => format!("unique replace(mut {slot}, b\"next\")"),
    };
    let aggregate = match context {
        0 => format!(
            "let aggregate = Pair(left: {read}, right: NAMES[1])\n  let selected = aggregate.left"
        ),
        1 => format!("let aggregate = ({read}, NAMES[1])\n  let (selected, _) = aggregate"),
        _ => format!(
            "let aggregate: Option[Bytes] = Some({read})\n  let Some(selected) = aggregate else {{ return Error(\"missing\") }}"
        ),
    };
    let mut source = String::from(
        r#"type Pair { left: Bytes, right: Bytes }
type Outer { inner: Pair }
const NAMES: Inline[Bytes, 2] = [b"left", b"right"]
const VALUES: Inline[Bytes, 2] = [b"zero", b"one"]
type Session { pending: bool, outgoing: List[Bytes] }
type Value = Missing | Object(fields: List[(String, u64)])
type Kind = Scalar(String) | Ref(u64)
type Field { boxed: bool, kind: Kind }
type Message { fields: List[Field] }
type Model { messages: List[Message] }
fn check(n:u64)->Result[u64,String] uses {} {
  if n > 100u64 { return Error("large") }
  Ok(n)
}
fn payload(v:Value)->Result[u64,String] uses {} {
  let Object(fields: fields) = v else { return Error("missing object") }
  let mut total:u64 = 0
  for (key,value) in fields {
    match key {
      "x" => { total = check(total + value)? },
      _ => (),
    }
  }
  Ok(total)
}
fn destructured(v:(String,u64))->u64 uses {} {
  let (key,value) = v
  match key { "x" => value, _ => 0u64 }
}
fn borrowed(key:String)->u64 uses {} {
  match key { "x" => 1u64, _ => 0u64 }
}
fn drain(mut s:Session)->(Bytes,List[Bytes]) uses {} {
  let prefix = if replace(mut s.pending,false) { b"PRI" } else { b"" }
  (prefix, unique replace(mut s.outgoing,[]))
}
fn lookup(i:u64)->Result[Pair,String] uses {} {
  if i > 1u64 { return Error("index") }
  Ok(Pair(left:NAMES[i],right:VALUES[i]))
}
"#,
    );
    // The collection a loop writes fields through follows the place: a `mut`
    // parameter, a `unique` element read into a local, or a nested `mut` field.
    source.push_str(match place {
        0 => "fn mark(mut model:Model,take owned:List[Field]) uses {} {\n  let mut fields = owned\n  for f in mut fields {\n    if let Ref(j) = f.kind {\n      f.boxed = j == 0u64\n    }\n  }\n  model.messages[0].fields = take fields\n}\n",
        1 => "fn mark(mut model:Model,resolved:List[List[Field]]) uses {} {\n  let mut fields = unique resolved[0]\n  for f in mut fields {\n    if let Ref(j) = f.kind {\n      f.boxed = j == 0u64\n    }\n  }\n  model.messages[0].fields = take fields\n}\n",
        _ => "fn mark(mut model:Model,take fields:List[Field]) uses {} {\n  model.messages[0].fields = fields\n  for f in mut model.messages[0].fields {\n    if let Ref(j) = f.kind {\n      f.boxed = j == 0u64\n    }\n  }\n}\n",
    });
    source.push_str("pub fn main()->Result[(),String] {\n");
    source.push_str(&format!("  {setup}\n  {aggregate}\n"));
    source.push_str(r#"  if selected != b"left" { return Error("aggregate") }
  let mut s = Session(pending:true,outgoing:[b"a",b"b"])
  let (prefix,frames) = drain(mut s)
  if bytes.len(prefix) != 3u64 || list.len(frames) != 2u64 || list.len(s.outgoing) != 0u64 { return Error("drain") }
  let pair = lookup(1u64)?
  if pair.left != b"right" || pair.right != b"one" { return Error("const") }
  if payload(Object(fields:[("x",1u64)]))? != 1u64 { return Error("payload") }
  if destructured(("x",1u64)) != 1u64 || borrowed("x") != 1u64 { return Error("match") }
  let mut model = Model(messages:[Message(fields:[])])
  let fields:List[Field] = [Field(boxed:false,kind:Ref(0u64))]
  mark(mut model,MARK_ARGUMENT)
  if !model.messages[0].fields[0].boxed { return Error("loop") }
  Ok(())
}
"#);
    source.replace(
        "MARK_ARGUMENT",
        if place == 1 {
            "[fields]"
        } else {
            "take fields"
        },
    )
}

/// Every combination: seed modulo 18 enumerates the product exactly.
pub const BUG_COMBINATIONS: u64 = 18;

pub fn bug_cases() -> Vec<super::Case> {
    (0..BUG_COMBINATIONS)
        .map(|seed| super::Case {
            id: format!("generated/bug-{seed}"),
            source: bug_program(seed),
            path: None,
        })
        .collect()
}
