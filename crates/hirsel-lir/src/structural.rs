//! Logical value shapes. These nodes are deliberately independent of LayoutId.
use hirsel_codec::{snapshot_enum, snapshot_struct};

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum Shape {
    Leaf,
    Fields(Vec<u32>),
    Variant(Vec<Vec<u32>>),
    Box(u32),
    Sequence(u32),
    Map { key: u32, value: u32 },
    Set(u32),
    Distinct(u32),
    Transparent(u32),
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ShapeGraph {
    pub root: u32,
    pub nodes: Vec<Shape>,
}

impl ShapeGraph {
    /// Maximum logical edge count of every value of this shape. A reachable
    /// cycle or malformed graph has no static bound. This is derived from the
    /// same source graph V validates, never from asserted physical edge totals.
    /// Iterative postorder keeps arbitrarily deep type graphs off the host stack.
    pub fn static_depth_bound(&self) -> Option<u64> {
        let mut state = vec![0u8; self.nodes.len()];
        let mut bounds = vec![0u64; self.nodes.len()];
        let mut work = vec![(self.root, false)];
        while let Some((id, finish)) = work.pop() {
            let node = self.nodes.get(id as usize)?;
            if finish {
                let mut bound = Some(0);
                node.children(|child| {
                    bound = bound
                        .zip(bounds.get(child as usize))
                        .and_then(|(max, &child)| {
                            child
                                .checked_add(u64::from(!matches!(node, Shape::Transparent(_))))
                                .map(|child| max.max(child))
                        });
                });
                bounds[id as usize] = bound?;
                state[id as usize] = 2;
            } else {
                match state[id as usize] {
                    1 => return None,
                    2 => continue,
                    _ => (),
                }
                state[id as usize] = 1;
                work.push((id, true));
                node.children(|child| work.push((child, false)));
            }
        }
        bounds.get(self.root as usize).copied()
    }

    pub fn check(&self) -> Result<(), &'static str> {
        if self.root as usize >= self.nodes.len() {
            return Err("structural root outside graph");
        }
        for node in &self.nodes {
            let children: Vec<u32> = match node {
                Shape::Leaf => vec![],
                Shape::Fields(fields) => fields.clone(),
                Shape::Variant(arms) => arms.iter().flatten().copied().collect(),
                Shape::Map { key, value } => vec![*key, *value],
                Shape::Box(child)
                | Shape::Sequence(child)
                | Shape::Set(child)
                | Shape::Distinct(child)
                | Shape::Transparent(child) => vec![*child],
            };
            if children
                .iter()
                .any(|&child| child as usize >= self.nodes.len())
            {
                return Err("structural child outside graph");
            }
        }
        // Transparency never consumes a value edge. A transparency-only cycle
        // would turn finite value admission into an implementation loop.
        for start in 0..self.nodes.len() {
            let mut at = start;
            let mut seen = std::collections::BTreeSet::new();
            while let Shape::Transparent(child) = self.nodes[at] {
                if !seen.insert(at) {
                    return Err("structural transparency cycle");
                }
                at = child as usize;
            }
        }
        Ok(())
    }
}

impl Shape {
    fn children(&self, mut visit: impl FnMut(u32)) {
        match self {
            Self::Leaf => (),
            Self::Fields(fields) => fields.iter().copied().for_each(visit),
            Self::Variant(arms) => arms.iter().flatten().copied().for_each(visit),
            Self::Map { key, value } => {
                visit(*key);
                visit(*value);
            }
            Self::Box(child)
            | Self::Sequence(child)
            | Self::Set(child)
            | Self::Distinct(child)
            | Self::Transparent(child) => visit(*child),
        }
    }
}

snapshot_enum!(Shape {
    Leaf,
    Fields(fields),
    Variant(arms),
    Box(child),
    Sequence(child),
    Map { key, value },
    Set(child),
    Distinct(child),
    Transparent(child)
});
snapshot_struct!(ShapeGraph { root, nodes });
