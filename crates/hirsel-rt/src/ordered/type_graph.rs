//! Known type definitions in descriptor order. Missing definitions remain
//! distinct from known empty edges. Sparse dynamic IDs do not allocate gaps.
#[derive(Clone, Debug, Default)]
pub struct TypeGraph(Vec<(u32, Box<[u32]>)>);

impl TypeGraph {
    pub(super) fn with_capacity(count: usize) -> Self {
        Self(Vec::with_capacity(count))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn get(&self, id: &u32) -> Option<&[u32]> {
        self.0
            .binary_search_by_key(id, |row| row.0)
            .ok()
            .map(|at| self.0[at].1.as_ref())
    }

    pub fn keys(&self) -> impl Iterator<Item = &u32> {
        self.0.iter().map(|row| &row.0)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&u32, &[u32])> {
        self.0.iter().map(|row| (&row.0, row.1.as_ref()))
    }

    pub(super) fn define(&mut self, id: u32, mut edges: Vec<u32>) {
        edges.sort_unstable();
        edges.dedup();
        let row = (id, edges.into_boxed_slice());
        // Linked descriptor IDs arrive in order. Runtime definitions also
        // support replacement and arbitrary sparse IDs without a dense resize.
        if self.0.last().is_none_or(|last| last.0 < id) {
            self.0.push(row);
        } else {
            match self.0.binary_search_by_key(&id, |row| row.0) {
                Ok(at) => self.0[at] = row,
                Err(at) => self.0.insert(at, row),
            }
        }
    }
}

impl std::ops::Index<&u32> for TypeGraph {
    type Output = [u32];
    fn index(&self, id: &u32) -> &Self::Output {
        self.get(id).expect("a known type definition")
    }
}
