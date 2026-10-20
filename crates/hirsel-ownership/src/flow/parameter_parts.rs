//! Finite observations of parameter field paths for the frame's result relation.
//! A matched prefix stays matched under further projection; a divergent prefix
//! stays outside. Only prefixes still capable of matching need distinct states.
use super::*;

const WITHIN: usize = 0;
const OUTSIDE: usize = 1;

pub(super) struct ParameterParts {
    roots: BTreeMap<usize, usize>,
    next: Vec<BTreeMap<u32, usize>>,
}

impl ParameterParts {
    pub fn new(body: &CheckedBody, graph: &Graph, rows: &crate::source_rows::SourceRows) -> Self {
        let mut parts = Self {
            roots: BTreeMap::new(),
            next: vec![BTreeMap::new(), BTreeMap::new()],
        };
        for reach in graph.result.relation.reaches() {
            let Reach::Place(Origin {
                root: OriginRoot::Parameter(parameter),
                fields,
            }) = reach
            else {
                continue;
            };
            let (scope, parameter) = match parameter {
                ParameterRef::Bound {
                    depth: 0,
                    parameter,
                } => (graph.owner, *parameter),
                ParameterRef::Body { scope, parameter } => (*scope, *parameter),
                _ => continue,
            };
            let Some(root) = rows.binding(body, Some(scope), &ResolvedName::Parameter(parameter))
            else {
                continue;
            };
            parts.insert(root, fields.iter().map(|field| field.0));
        }
        parts
    }

    fn node(&mut self) -> usize {
        let id = self.next.len();
        self.next.push(BTreeMap::new());
        id
    }

    fn insert(&mut self, root: usize, fields: impl IntoIterator<Item = u32>) {
        let start = if let Some(&start) = self.roots.get(&root) {
            start
        } else {
            let start = self.node();
            self.roots.insert(root, start);
            start
        };
        let mut at = start;
        // The final edge enters the absorbing matched state. A shorter declared
        // prefix subsumes longer declarations, in either insertion order.
        let fields: Vec<_> = fields.into_iter().collect();
        if fields.is_empty() {
            self.roots.insert(root, WITHIN);
        }
        for (index, field) in fields.iter().enumerate() {
            if at == WITHIN {
                break;
            }
            if index + 1 == fields.len() {
                self.next[at].insert(*field, WITHIN);
            } else if let Some(&child) = self.next[at].get(field) {
                at = child;
            } else {
                let child = self.node();
                self.next[at].insert(*field, child);
                at = child;
            }
        }
    }

    pub fn start(&self, root: usize) -> Option<usize> {
        self.roots.get(&root).copied()
    }

    pub fn project(&self, mut state: usize, fields: impl IntoIterator<Item = u32>) -> usize {
        for field in fields {
            if state == WITHIN || state == OUTSIDE {
                break;
            }
            state = self.next[state].get(&field).copied().unwrap_or(OUTSIDE);
        }
        state
    }

    pub fn outside(&self, value: &Value) -> bool {
        let reached = value.reached();
        self.roots.iter().any(|(root, start)| {
            *start != WITHIN
                && reached.contains(root)
                && !matches!(&value.part_of, Some((source, states))
                    if source == root && states.iter().all(|state| *state == WITHIN))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finite_states_preserve_every_prefix_test_and_future_projection() {
        for declarations in [
            vec![vec![]],
            vec![vec![0]],
            vec![vec![0, 1], vec![1, 0]],
            vec![vec![0, 1], vec![0]],
            vec![vec![0], vec![0, 1]],
        ] {
            let mut parts = ParameterParts {
                roots: BTreeMap::new(),
                next: vec![BTreeMap::new(), BTreeMap::new()],
            };
            for path in &declarations {
                parts.insert(7, path.iter().copied());
            }
            // All ternary strings through length seven, projected one field at
            // a time, include matches, strict prefixes and divergent siblings.
            for length in 0..=7 {
                for mut number in 0usize..3usize.pow(length) {
                    let mut path = Vec::new();
                    let mut state = parts.start(7).unwrap();
                    for _ in 0..length {
                        let field = (number % 3) as u32;
                        number /= 3;
                        path.push(field);
                        state = parts.project(state, [field]);
                        assert_eq!(
                            state == WITHIN,
                            declarations.iter().any(|p| path.starts_with(p)),
                            "{declarations:?}, {path:?}"
                        );
                    }
                    assert!(state < parts.next.len());
                }
            }
            assert!(parts.next.len() <= 3 + declarations.iter().map(Vec::len).sum::<usize>());
        }
    }
}
