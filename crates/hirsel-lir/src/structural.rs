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
