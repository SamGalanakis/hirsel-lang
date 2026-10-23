//! Finite truth tables, independent of both production matrix algorithms.
//! Boolean and nested-product vectors adapt rust-analyzer's
//! missing_match_arms.rs:80-157 at 3131751eaf1c3b5da6e618acdc2148ad7db65bc7
//! (MIT/Apache-2.0, rust-analyzer contributors). Guard presence follows its
//! pat_analysis.rs:87-105 interface; Hirsel rejects dead alternatives and
//! permits covered catch-alls. No specialization algorithm is imported.
#[derive(Clone, Debug)]
pub enum Domain {
    Bool,
    Unit,
    Pair(Box<Domain>, Box<Domain>),
    Sum(Box<Domain>),
}
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    Unit,
    Pair(Box<Value>, Box<Value>),
    Sum(u32, Box<Value>),
}
#[derive(Clone, Debug)]
pub enum Pattern {
    Wild,
    Exact(Value),
    Pair(Box<Pattern>, Box<Pattern>),
    Sum(u32, Box<Pattern>),
    Or(Vec<Pattern>),
}
#[derive(Clone, Debug)]
pub struct Arm {
    pub pattern: Pattern,
    pub guard: Option<bool>,
}
#[derive(Clone, Debug)]
pub struct Case {
    pub name: String,
    pub domain: Domain,
    pub arms: Vec<Arm>,
}
#[derive(Debug)]
pub struct Truth {
    pub exhaustive: bool,
    pub dead_alternatives: usize,
    pub reachable_arms: Vec<bool>,
}
impl Domain {
    pub fn values(&self) -> Vec<Value> {
        match self {
            Self::Bool => vec![Value::Bool(false), Value::Bool(true)],
            Self::Unit => vec![Value::Unit],
            Self::Pair(a, b) => a
                .values()
                .into_iter()
                .flat_map(|a| {
                    b.values()
                        .into_iter()
                        .map(move |b| Value::Pair(Box::new(a.clone()), Box::new(b)))
                })
                .collect(),
            Self::Sum(p) => (0..2)
                .flat_map(|tag| {
                    p.values()
                        .into_iter()
                        .map(move |v| Value::Sum(tag, Box::new(v)))
                })
                .collect(),
        }
    }
    pub fn source(&self) -> String {
        match self {
            Self::Bool => "bool".into(),
            Self::Unit => "()".into(),
            Self::Pair(a, b) => format!("({},{})", a.source(), b.source()),
            Self::Sum(_) => "Choice".into(),
        }
    }
    pub fn declaration(&self) -> String {
        match self {
            Self::Sum(p) => format!("type Choice = A({0}) | B({0})\n", p.source()),
            Self::Pair(a, b) => {
                let d = a.declaration();
                if d.is_empty() { b.declaration() } else { d }
            }
            _ => String::new(),
        }
    }
    // A broad first slice, so later nested alternatives must be useful within
    // their own constructor even when another constructor still has values.
    fn first_slice(&self) -> Pattern {
        match self {
            Self::Bool => Pattern::Exact(Value::Bool(false)),
            Self::Unit => Pattern::Exact(Value::Unit),
            Self::Pair(a, _) => Pattern::Pair(Box::new(a.first_slice()), Box::new(Pattern::Wild)),
            Self::Sum(p) => Pattern::Sum(0, Box::new(p.first_slice())),
        }
    }
    fn alternatives(&self) -> Pattern {
        match self {
            Self::Bool => Pattern::Or(self.values().into_iter().map(Pattern::Exact).collect()),
            Self::Unit => Pattern::Exact(Value::Unit),
            Self::Pair(a, b) => {
                Pattern::Pair(Box::new(a.alternatives()), Box::new(b.alternatives()))
            }
            Self::Sum(p) => Pattern::Or(
                (0..2)
                    .map(|tag| Pattern::Sum(tag, Box::new(p.alternatives())))
                    .collect(),
            ),
        }
    }
}
impl Value {
    pub fn source(&self) -> String {
        match self {
            Self::Bool(b) => b.to_string(),
            Self::Unit => "()".into(),
            Self::Pair(a, b) => format!("({},{})", a.source(), b.source()),
            Self::Sum(tag, p) => format!("{}({})", if *tag == 0 { "A" } else { "B" }, p.source()),
        }
    }
}
impl Pattern {
    pub fn source(&self) -> String {
        match self {
            Self::Wild => "_".into(),
            Self::Exact(v) => v.source(),
            Self::Pair(a, b) => format!("({},{})", a.source(), b.source()),
            Self::Sum(tag, p) => format!("{}({})", if *tag == 0 { "A" } else { "B" }, p.source()),
            Self::Or(ps) => ps.iter().map(Self::source).collect::<Vec<_>>().join(" | "),
        }
    }
    fn matches(&self, value: &Value, chosen: Option<(&[usize], usize)>) -> bool {
        let child =
            |i| chosen.and_then(|(path, alt)| path.strip_prefix(&[i]).map(|tail| (tail, alt)));
        match (self, value) {
            (Self::Wild, _) => true,
            (Self::Exact(a), b) => a == b,
            (Self::Pair(a, b), Value::Pair(x, y)) => {
                a.matches(x, child(0)) && b.matches(y, child(1))
            }
            (Self::Sum(t, p), Value::Sum(u, v)) => t == u && p.matches(v, child(0)),
            (Self::Or(ps), _) => match chosen {
                Some(([], alt)) => ps[alt].matches(value, None),
                Some(([branch, ..], _)) => {
                    !ps[..*branch].iter().any(|p| p.matches(value, None))
                        && ps[*branch].matches(value, child(*branch))
                }
                _ => ps
                    .iter()
                    .enumerate()
                    .any(|(i, p)| p.matches(value, child(i))),
            },
            _ => false,
        }
    }
    fn or_sites(&self, path: Vec<usize>, out: &mut Vec<(Vec<usize>, usize)>) {
        let children: Vec<&Pattern> = match self {
            Self::Or(ps) => {
                out.push((path.clone(), ps.len()));
                ps.iter().collect()
            }
            Self::Pair(a, b) => vec![a, b],
            Self::Sum(_, p) => vec![p],
            _ => vec![],
        };
        for (i, p) in children.into_iter().enumerate() {
            let mut next = path.clone();
            next.push(i);
            p.or_sites(next, out);
        }
    }
    /// Syntactic distribution for TIR, which has no Or node. This is an input
    /// adapter only: expected coverage always comes from concrete values.
    pub fn expanded(&self) -> Vec<Pattern> {
        match self {
            Self::Or(ps) => ps.iter().flat_map(Self::expanded).collect(),
            Self::Pair(a, b) => a
                .expanded()
                .into_iter()
                .flat_map(|a| {
                    b.expanded()
                        .into_iter()
                        .map(move |b| Self::Pair(Box::new(a.clone()), Box::new(b)))
                })
                .collect(),
            Self::Sum(t, p) => p
                .expanded()
                .into_iter()
                .map(|p| Self::Sum(*t, Box::new(p)))
                .collect(),
            p => vec![p.clone()],
        }
    }
}
impl Case {
    pub fn truth(&self) -> Truth {
        let values = self.domain.values();
        assert!(!values.is_empty() && values.len() <= 16);
        let mut covered = vec![false; values.len()];
        let mut dead_alternatives = 0;
        let mut reachable_arms = vec![];
        for arm in &self.arms {
            let matched: Vec<_> = values
                .iter()
                .map(|v| arm.pattern.matches(v, None))
                .collect();
            reachable_arms.push(matched.iter().zip(&covered).any(|(m, c)| *m && !c));
            let mut sites = vec![];
            arm.pattern.or_sites(vec![], &mut sites);
            for (site, count) in sites {
                let mut earlier = covered.clone();
                for alt in 0..count {
                    let selected: Vec<_> = values
                        .iter()
                        .map(|v| arm.pattern.matches(v, Some((&site, alt))))
                        .collect();
                    if !selected.iter().zip(&earlier).any(|(m, c)| *m && !c) {
                        dead_alternatives += 1;
                    }
                    for (c, m) in earlier.iter_mut().zip(selected) {
                        *c |= m;
                    }
                }
            }
            if arm.guard.is_none() {
                for (c, m) in covered.iter_mut().zip(matched) {
                    *c |= m;
                }
            }
        }
        Truth {
            exhaustive: covered.into_iter().all(|c| c),
            dead_alternatives,
            reachable_arms,
        }
    }
    pub fn source(&self) -> String {
        let arms = self
            .arms
            .iter()
            .enumerate()
            .map(|(i, a)| {
                format!(
                    "{}{}=>{i}u32",
                    a.pattern.source(),
                    a.guard.map(|b| format!(" if {b}")).unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "{}fn main(x:{})->u32{{match x{{{arms}}}}}",
            self.domain.declaration(),
            self.domain.source()
        )
    }
}
pub fn cases() -> Vec<Case> {
    use Domain::*;
    let pair = |a, b| Pair(Box::new(a), Box::new(b));
    let sum = |p| Sum(Box::new(p));
    let domains = [
        Bool,
        Unit,
        sum(Bool),
        pair(Bool, Bool),
        pair(Bool, pair(Unit, Bool)),
        pair(sum(Bool), Bool),
        sum(pair(Bool, Bool)),
        pair(sum(Bool), sum(Bool)),
    ];
    let mut cases = vec![];
    for (d, domain) in domains.into_iter().enumerate() {
        let values = domain.values();
        let exact: Vec<_> = values
            .iter()
            .map(|v| Arm {
                pattern: Pattern::Exact(v.clone()),
                guard: None,
            })
            .collect();
        for template in 0..12 {
            let wild = Arm {
                pattern: Pattern::Wild,
                guard: None,
            };
            let first = Pattern::Exact(values[0].clone());
            let mut arms = exact.clone();
            match template {
                0 => (),
                1 => {
                    arms.pop();
                }
                2..=5 => {
                    arms.last_mut().unwrap().guard = Some(template % 2 == 0);
                    if template >= 4 {
                        arms.push(wild);
                    }
                }
                6 => arms.push(wild),
                7 => arms.push(exact.last().unwrap().clone()),
                8 => {
                    arms = vec![
                        Arm {
                            pattern: Pattern::Or(vec![first.clone(), first]),
                            guard: None,
                        },
                        wild,
                    ]
                }
                9 => {
                    arms = vec![
                        Arm {
                            pattern: Pattern::Or(vec![first, Pattern::Wild]),
                            guard: None,
                        },
                        wild,
                    ]
                }
                10 => {
                    arms = vec![
                        Arm {
                            pattern: domain.alternatives(),
                            guard: None,
                        },
                        wild,
                    ]
                }
                11 => {
                    arms = vec![
                        Arm {
                            pattern: domain.first_slice(),
                            guard: None,
                        },
                        Arm {
                            pattern: domain.alternatives(),
                            guard: None,
                        },
                        wild,
                    ]
                }
                _ => unreachable!(),
            }
            cases.push(Case {
                name: format!("domain-{d}-matrix-{template}"),
                domain: domain.clone(),
                arms,
            });
        }
    }
    assert_eq!(cases.len(), 96);
    cases
}

/// Optional exact identities make a failed matrix replayable without rerunning
/// already established cells. The normal suite always runs all 96 matrices.
pub fn selected_cases() -> Vec<Case> {
    let all = cases();
    let Ok(names) = std::env::var("MATCH_COVERAGE_22_CASES") else {
        return all;
    };
    let names: Vec<_> = names.split(',').collect();
    let selected: Vec<_> = all
        .into_iter()
        .filter(|c| names.contains(&c.name.as_str()))
        .collect();
    assert!(!selected.is_empty());
    assert_eq!(
        selected.len(),
        names.len(),
        "unknown or repeated matrix identity"
    );
    selected
}
