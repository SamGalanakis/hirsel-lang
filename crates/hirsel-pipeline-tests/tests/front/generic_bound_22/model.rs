//! Bounded recursive shape vectors, independent of either checker's bound proofs.
//! Adapts the leaf/recursive construction and depth/size budget from Proptest
//! recursive.rs:150-208 and tutorial/recursive.md:44-83 at 70cf37d46b7d83393b6ed94c611f8c52209f585d
//! (MIT OR Apache-2.0). Six fixed seeds replace random sampling/shrinking:
//! failures retain their original seed; no reduced passing seed replaces one.
#[derive(Clone, Debug)]
pub enum Shape {
    U32,
    F64,
    Callable,
    Tuple(Vec<Shape>),
    Record(Box<Shape>),
    Variant(Box<Shape>),
}
impl Shape {
    pub fn accepts(&self, key: bool) -> bool {
        match self {
            Self::U32 => true,
            Self::F64 => !key,
            Self::Callable => false,
            Self::Tuple(parts) => parts.iter().all(|s| s.accepts(key)),
            Self::Record(part) | Self::Variant(part) => part.accepts(key),
        }
    }
    pub fn budget(&self) -> (usize, usize) {
        let children: Vec<_> = match self {
            Self::Tuple(parts) => parts.iter().map(Self::budget).collect(),
            Self::Record(part) | Self::Variant(part) => vec![part.budget()],
            _ => return (0, 1),
        };
        (
            1 + children.iter().map(|s| s.0).max().unwrap_or(0),
            1 + children.iter().map(|s| s.1).sum::<usize>(),
        )
    }
    pub fn source(&self, declarations: &mut String, serial: &mut usize) -> String {
        match self {
            Self::U32 => "u32".into(),
            Self::F64 => "f64".into(),
            Self::Callable => "fn(u32)->u32 uses {}".into(),
            Self::Tuple(parts) => format!(
                "({},)",
                parts
                    .iter()
                    .map(|s| s.source(declarations, serial))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            Self::Record(part) | Self::Variant(part) => {
                let child = part.source(declarations, serial);
                let name = format!("Shape{serial}");
                *serial += 1;
                if matches!(self, Self::Record(_)) {
                    declarations.push_str(&format!("type {name} {{ field:{child} }}\n"));
                } else {
                    declarations.push_str(&format!("type {name}=Present({child}) | Absent\n"));
                }
                name
            }
        }
    }
}
pub fn shapes() -> Vec<Shape> {
    use Shape::*;
    let seeds = vec![
        U32,
        F64,
        Tuple(vec![U32, Record(Box::new(F64))]),
        Record(Box::new(Tuple(vec![U32, U32]))),
        Variant(Box::new(U32)),
        Record(Box::new(Variant(Box::new(Callable)))),
    ];
    for seed in &seeds {
        let (depth, nodes) = seed.budget();
        assert!(depth <= 3 && nodes <= 5, "{seed:?}: {depth}/{nodes}");
    }
    seeds
}
