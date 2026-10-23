//! SEM-1 cross-width source controls, using the shared history observation runner.
//! Division shapes: Rust coretests/tests/num/{int_macros.rs:166-170,
//! uint_macros.rs:212-216}, f6c1107f (MIT OR Apache-2.0). None becomes SEM-1;
//! remainder and shifts follow Hirsel rather than Rust's overflow settings.
//! Reuses rc_legalize's I/H division/conversion controls; adds source twins and
//! native dispositions, rather than another full conversion Cartesian matrix.
use crate::rc_support::history::{Category, Runner, persist};
use std::fmt::{Display, Formatter, Write};

/// Mathematical sign/magnitude, including positive u128::MAX and negative i128::MIN.
/// Arithmetic checks the destination range before adding or multiplying, so it
/// does not rely on host signed overflow or the interpreter's scalar kernels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Integer {
    negative: bool,
    magnitude: u128,
}

impl Integer {
    fn new(negative: bool, magnitude: u128) -> Self {
        Self {
            negative: negative && magnitude != 0,
            magnitude,
        }
    }

    fn negated(self) -> Self {
        Self::new(!self.negative, self.magnitude)
    }

    fn neighbour(self, up: bool) -> Option<Self> {
        if self.negative == up {
            if self.magnitude == 0 {
                Some(Self::new(!up, 1))
            } else {
                Some(Self::new(self.negative, self.magnitude - 1))
            }
        } else {
            self.magnitude
                .checked_add(1)
                .map(|m| Self::new(self.negative, m))
        }
    }
}

impl Display for Integer {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}{}",
            if self.negative { "-" } else { "" },
            self.magnitude
        )
    }
}

#[derive(Clone, Copy, Debug)]
struct Kind {
    width: u32,
    signed: bool,
}

impl Kind {
    fn name(self) -> String {
        format!("{}{}", if self.signed { 'i' } else { 'u' }, self.width)
    }
    fn limit(self, negative: bool) -> u128 {
        if self.signed {
            (1u128 << (self.width - 1)) - u128::from(!negative)
        } else if negative {
            0
        } else {
            u128::MAX >> (128 - self.width)
        }
    }
    fn fits(self, n: Integer) -> bool {
        n.magnitude <= self.limit(n.negative)
    }
    fn mask(self) -> u128 {
        u128::MAX >> (128 - self.width)
    }
    fn bits(self, n: Integer) -> u128 {
        (if n.negative {
            (!n.magnitude).wrapping_add(1)
        } else {
            n.magnitude
        }) & self.mask()
    }
    fn decode(self, bits: u128) -> Integer {
        let bits = bits & self.mask();
        if self.signed && bits & (1 << (self.width - 1)) != 0 {
            Integer::new(true, ((!bits) & self.mask()) + 1)
        } else {
            Integer::new(false, bits)
        }
    }
    fn model(self, op: &str, a: Integer, mut b: Integer) -> Option<Integer> {
        assert!(self.fits(a) && self.fits(b));
        let result = match op {
            "+" | "-" => {
                if op == "-" {
                    b = b.negated();
                }
                if a.negative == b.negative {
                    let limit = self.limit(a.negative);
                    if a.magnitude > limit || b.magnitude > limit - a.magnitude {
                        return None;
                    }
                    Integer::new(a.negative, a.magnitude + b.magnitude)
                } else if a.magnitude >= b.magnitude {
                    Integer::new(a.negative, a.magnitude - b.magnitude)
                } else {
                    Integer::new(b.negative, b.magnitude - a.magnitude)
                }
            }
            "*" => {
                let negative = a.negative != b.negative;
                if a.magnitude != 0 && b.magnitude > self.limit(negative) / a.magnitude {
                    return None;
                }
                Integer::new(negative, a.magnitude * b.magnitude)
            }
            "/" | "%" => {
                if b.magnitude == 0 {
                    return None;
                }
                if op == "/" {
                    Integer::new(a.negative != b.negative, a.magnitude / b.magnitude)
                } else {
                    Integer::new(a.negative, a.magnitude % b.magnitude)
                }
            }
            "<<" | ">>" => {
                if b.negative || b.magnitude >= u128::from(self.width) {
                    return None;
                }
                let count = b.magnitude as u32;
                let bits = self.bits(a);
                return Some(self.decode(if op == "<<" {
                    bits << count
                } else if a.negative && count != 0 {
                    (bits >> count) | (u128::MAX << (self.width - count))
                } else {
                    bits >> count
                }));
            }
            _ => unreachable!(),
        };
        self.fits(result).then_some(result)
    }
}

#[derive(Debug)]
struct Vector {
    kind: Kind,
    op: &'static str,
    a: Integer,
    b: Integer,
    to: Kind,
    expected: Option<Integer>,
}

impl Vector {
    fn expression(&self, parameters: bool) -> String {
        let (a, b) = if parameters {
            ("a".into(), "b".into())
        } else {
            (
                format!("{}{}", self.a, self.kind.name()),
                format!("{}{}", self.b, self.kind.name()),
            )
        };
        if self.op == "narrow" {
            format!("int.narrow({a})")
        } else {
            format!("{a} {} {b}", self.op)
        }
    }
    fn helper(&self, name: &str) -> String {
        let t = self.kind.name();
        let parameters = if self.op == "narrow" {
            format!("a: {t}")
        } else {
            format!("a: {t}, b: {t}")
        };
        format!(
            "fn {name}({parameters}) -> {} {{\n  {}\n}}\n",
            self.to.name(),
            self.expression(true)
        )
    }
    fn call(&self, name: &str) -> String {
        if self.op == "narrow" {
            format!("{name}({}{})", self.a, self.kind.name())
        } else {
            format!(
                "{name}({}{}, {}{})",
                self.a,
                self.kind.name(),
                self.b,
                self.kind.name()
            )
        }
    }
}

/// The 418 finite vectors prepared by the independent unbounded-integer model.
fn vectors(k: Kind) -> Vec<Vector> {
    let n = |v| Integer::new(false, v);
    let min = Integer::new(k.signed, k.limit(true));
    let max = n(k.limit(false));
    let w = u128::from(k.width);
    let mut shapes = vec![
        ("+", max, n(0)),
        ("+", max, n(1)),
        ("+", min, n(1)),
        ("-", min, n(0)),
        ("-", min, n(1)),
        ("-", max, n(1)),
        ("*", max, n(1)),
        ("*", max, n(2)),
        ("*", min, n(0)),
        ("*", min, n(1)),
        ("*", min, n(2)),
        ("/", n(10), n(2)),
        ("/", n(5), n(0)),
        ("/", max, n(3)),
        ("/", min, n(1)),
        ("%", n(5), n(0)),
        ("%", max, n(3)),
        ("<<", n(1), n(0)),
        ("<<", n(1), n(w - 1)),
        ("<<", n(1), n(w)),
        ("<<", max, n(1)),
        ("<<", min, n(1)),
        (
            "<<",
            if k.signed { min } else { n(1 << (k.width - 1)) },
            n(1),
        ),
        (">>", max, n(w - 1)),
        (">>", min, n(w - 1)),
        (">>", min, n(0)),
        (">>", n(1), n(w)),
    ];
    if k.signed {
        let minus_one = n(1).negated();
        shapes.extend([
            ("+", min, minus_one),
            ("+", min, max),
            ("-", max, minus_one),
            ("*", min, minus_one),
            ("*", minus_one, min),
            ("/", min, minus_one),
            ("%", min, minus_one),
            ("/", n(7).negated(), n(3)),
            ("%", n(7).negated(), n(3)),
            ("/", n(7), n(3).negated()),
            ("%", n(7), n(3).negated()),
            ("<<", n(1), minus_one),
            (">>", n(1), minus_one),
            (">>", n(7).negated(), n(1)),
        ]);
    }
    let mut seen = Vec::new();
    shapes.retain(|shape| {
        if seen.contains(shape) {
            false
        } else {
            seen.push(*shape);
            true
        }
    });
    let mut out: Vec<_> = shapes
        .into_iter()
        .map(|(op, a, b)| Vector {
            kind: k,
            op,
            a,
            b,
            to: k,
            expected: k.model(op, a, b),
        })
        .collect();
    let destinations = [
        Some(Kind {
            width: k.width,
            signed: !k.signed,
        }),
        (k.width > 8).then_some(Kind {
            width: k.width / 2,
            signed: k.signed,
        }),
    ];
    for to in destinations.into_iter().flatten() {
        let low = Integer::new(to.signed, to.limit(true));
        let high = n(to.limit(false));
        let mut seen = Vec::new();
        for x in [
            Some(n(0)),
            low.neighbour(false),
            Some(low),
            low.neighbour(true),
            high.neighbour(false),
            Some(high),
            high.neighbour(true),
        ]
        .into_iter()
        .flatten()
        {
            if !k.fits(x) || seen.contains(&x) {
                continue;
            }
            seen.push(x);
            out.push(Vector {
                kind: k,
                op: "narrow",
                a: x,
                b: n(0),
                to,
                expected: to.fits(x).then_some(x),
            });
        }
    }
    out
}

fn check(
    runner: &Runner,
    name: &str,
    source: &str,
    failure_site: Option<(&str, u32, u32)>,
    vectors: usize,
) {
    let report = runner.run(name, source);
    let differences = report.disagreements(b"", failure_site.is_some());
    let mut valid = report.agrees() && differences.is_empty() && report.cells.len() == 4;
    for cell in &report.cells {
        valid &= cell.category == Category::Observation;
        if let Some((definition, line, column)) = failure_site {
            valid &= cell.observation.as_ref().is_some_and(|o| {
                o.failures.len() == 1 && {
                    let f = &o.failures[0];
                    f.rule == "SEM-1"
                        && f.definition == crate::rc_support::path(definition)
                        && f.file == "main.hir"
                        && f.operation.is_none()
                        && f.line == line
                        && f.column == column
                }
            });
        }
    }
    let receipt = serde_json::json!({"name":name,"vectors":vectors,"source":source,
        "hash":format!("{:?}",hirsel_syntax::ContentHash::of_bytes(source.as_bytes())),
        "expected_site":failure_site,"accepted":valid,
        "cells":report.cells.iter().map(|c| serde_json::json!({"engine":c.engine,"category":format!("{:?}",c.category),"observation":c.observation.as_ref().map(|o|o.render()),"detail":c.detail})).collect::<Vec<_>>()});
    eprintln!("INTEGER_BOUNDARY_RECEIPT {receipt}");
    if !valid {
        let artifacts = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join("integer-boundary-21-mismatch");
        persist(
            &artifacts,
            name,
            0,
            source,
            &format!("expected_site={failure_site:?}\n{report:#?}"),
        );
    }
    assert!(
        valid,
        "{name}: expected site {failure_site:?}\n{source}\n{report:#?}"
    );
}

fn chunk(k: Kind, index: usize) {
    let vectors = vectors(k);
    let expected_count = match (k.signed, k.width) {
        (false, 8) => 31,
        (false, _) => 36,
        (true, 8) => 43,
        (true, _) => 50,
    };
    assert_eq!(vectors.len(), expected_count);
    let group = vectors.chunks(6).nth(index).expect("nonempty chunk");
    let directory = tempfile::tempdir().unwrap();
    let runner = Runner::new(directory.path()).unwrap();
    let stem = format!("{}-{index}", k.name());
    let mut success = String::new();
    let mut conditions = String::new();
    let mut successes = 0;
    for (i, v) in group.iter().enumerate() {
        let helper = format!("operation_{i}");
        if let Some(value) = v.expected {
            success.push_str(&v.helper(&helper));
            for expression in [v.call(&helper), v.expression(false)] {
                writeln!(
                    conditions,
                    "  if ({expression}) != {value}{} {{ return Error(1u8) }}",
                    v.to.name()
                )
                .unwrap();
                successes += 1;
            }
        } else {
            for runtime in [false, true] {
                let source = if runtime {
                    format!(
                        "{}pub fn main() -> Result[(), u8] {{\n  let value: {} = {}\n  Ok(())\n}}\n",
                        v.helper("operation"),
                        v.to.name(),
                        v.call("operation")
                    )
                } else {
                    format!(
                        "pub fn main() -> Result[(), u8] {{\n  let value: {} = {}\n  Ok(())\n}}\n",
                        v.to.name(),
                        v.expression(false)
                    )
                };
                // Pin the expression's source start independently of the runner.
                let site = if runtime {
                    ("operation", 2, 3)
                } else {
                    let line = source.lines().nth(1).unwrap();
                    ("main", 2, line.find(" = ").unwrap() as u32 + 4)
                };
                check(
                    &runner,
                    &format!("{stem}-{i}-{runtime}"),
                    &source,
                    Some(site),
                    1,
                );
            }
        }
    }
    if successes != 0 {
        write!(
            success,
            "pub fn main() -> Result[(), u8] {{\n{conditions}  Ok(())\n}}\n"
        )
        .unwrap();
        check(
            &runner,
            &format!("{stem}-values"),
            &success,
            None,
            successes,
        );
    }
    eprintln!(
        "integer_boundary_21 {stem}: {} logical vectors, {} literal/runtime checks per engine",
        group.len(),
        group.len() * 2
    );
}

macro_rules! chunks {
    ($($module:ident: $width:literal,$signed:literal => [$($name:ident:$index:literal),*];)*) => {$(
        mod $module { $( #[test] fn $name() { super::chunk(super::Kind {width:$width,signed:$signed},$index); } )* }
    )*};
}
chunks! {
    u8:8,false => [c0:0,c1:1,c2:2,c3:3,c4:4,c5:5];
    u16:16,false => [c0:0,c1:1,c2:2,c3:3,c4:4,c5:5];
    u32:32,false => [c0:0,c1:1,c2:2,c3:3,c4:4,c5:5];
    u64:64,false => [c0:0,c1:1,c2:2,c3:3,c4:4,c5:5];
    u128:128,false => [c0:0,c1:1,c2:2,c3:3,c4:4,c5:5];
    i8:8,true => [c0:0,c1:1,c2:2,c3:3,c4:4,c5:5,c6:6,c7:7];
    i16:16,true => [c0:0,c1:1,c2:2,c3:3,c4:4,c5:5,c6:6,c7:7,c8:8];
    i32:32,true => [c0:0,c1:1,c2:2,c3:3,c4:4,c5:5,c6:6,c7:7,c8:8];
    i64:64,true => [c0:0,c1:1,c2:2,c3:3,c4:4,c5:5,c6:6,c7:7,c8:8];
    i128:128,true => [c0:0,c1:1,c2:2,c3:3,c4:4,c5:5,c6:6,c7:7,c8:8];
}
