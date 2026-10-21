//! First-match source identities; checked arena values remain authoritative.
use super::*;
use hirsel_modes::rows::ExprRows;
use std::collections::BTreeMap;
pub(super) struct SourceRows {
    pub expressions: ExprRows<usize>,
    pub places: ExprRows<usize>,
    pub calls: ExprRows<usize>,
    pub closures: ExprRows<usize>,
    pub unscoped: BTreeMap<(u8, u32), usize>,
    pub sites: BTreeMap<NameSite, Vec<usize>>,
    pub bindings: BTreeMap<(NameSite, u8, u32), usize>,
    pub other: Vec<usize>,
}
impl SourceRows {
    pub fn new(body: &CheckedBody) -> Self {
        let mut out = Self {
            expressions: ExprRows::default(),
            places: ExprRows::default(),
            calls: ExprRows::default(),
            closures: ExprRows::default(),
            unscoped: BTreeMap::new(),
            sites: BTreeMap::new(),
            bindings: BTreeMap::new(),
            other: vec![],
        };
        for (i, row) in body.expressions.iter().enumerate() {
            work::count(Counter::OwnershipSourceRows);
            out.expressions.entry(row.expression).get_or_insert(i);
        }
        for (i, row) in body.places.iter().enumerate() {
            work::count(Counter::OwnershipSourceRows);
            out.places.entry(row.expression).get_or_insert(i);
        }
        for (i, row) in body.calls.iter().enumerate() {
            work::count(Counter::OwnershipSourceRows);
            out.calls.entry(row.expression).get_or_insert(i);
        }
        for (i, row) in body.closures.iter().enumerate() {
            work::count(Counter::OwnershipSourceRows);
            out.closures.entry(row.expression).get_or_insert(i);
        }
        for (i, row) in body.bindings.iter().enumerate() {
            work::count(Counter::OwnershipSourceRows);
            out.sites.entry(row.site).or_default().push(i);
            let key = match row.binding {
                ResolvedName::Parameter(p) => Some((0, p.0)),
                ResolvedName::Local(l) => Some((1, l.0)),
                _ => None,
            };
            if let Some((kind, n)) = key {
                out.bindings.entry((row.owner, kind, n)).or_insert(i);
                out.unscoped.entry((kind, n)).or_insert(i);
            } else {
                out.other.push(i);
            }
        }
        out
    }
    pub fn expression(&self, expression: ExprId) -> Option<usize> {
        work::count(Counter::OwnershipSourceRows);
        self.expressions.get(&expression).copied()
    }
    pub fn source_place(&self, expression: ExprId) -> Option<usize> {
        work::count(Counter::OwnershipSourceRows);
        self.places.get(&expression).copied()
    }
    pub fn closure(&self, e: ExprId) -> Option<usize> {
        work::count(Counter::OwnershipSourceRows);
        self.closures.get(&e).copied()
    }
    pub fn call(&self, e: ExprId) -> Option<usize> {
        work::count(Counter::OwnershipSourceRows);
        self.calls.get(&e).copied()
    }
    pub fn binding(
        &self,
        body: &CheckedBody,
        owner: Option<NameSite>,
        name: &ResolvedName,
    ) -> Option<usize> {
        work::count(Counter::OwnershipSourceRows);
        let key = match name {
            ResolvedName::Parameter(p) => Some((0, p.0)),
            ResolvedName::Local(l) => Some((1, l.0)),
            _ => None,
        };
        if let Some((kind, n)) = key {
            if let Some(owner) = owner {
                self.bindings.get(&(owner, kind, n)).copied()
            } else {
                self.unscoped.get(&(kind, n)).copied()
            }
        } else {
            self.other.iter().copied().find(|i| {
                work::count(Counter::OwnershipSourceRows);
                owner.is_none_or(|o| body.bindings[*i].owner == o)
                    && body.bindings[*i].binding == *name
            })
        }
    }
    pub fn at(&self, site: NameSite) -> impl Iterator<Item = usize> + '_ {
        work::count(Counter::OwnershipSourceRows);
        self.sites
            .get(&site)
            .into_iter()
            .flatten()
            .copied()
            .inspect(|_| work::count(Counter::OwnershipSourceRows))
    }
}

#[derive(Debug)]
pub(super) struct CaptureRows {
    values: Vec<CaptureMode>,
    first: BTreeMap<ExprId, usize>,
    rows: BTreeMap<(ExprId, u32), usize>,
}
impl CaptureRows {
    pub fn new(values: &[CaptureMode]) -> Self {
        let mut out = Self {
            values: values.to_vec(),
            first: BTreeMap::new(),
            rows: BTreeMap::new(),
        };
        for (i, v) in values.iter().enumerate() {
            work::count(Counter::OwnershipSourceRows);
            out.first.entry(v.expression).or_insert(i);
            out.rows.entry((v.expression, v.ordinal)).or_insert(i);
        }
        out
    }
    pub fn first(&self, e: ExprId) -> Option<&CaptureMode> {
        work::count(Counter::OwnershipSourceRows);
        self.first.get(&e).map(|i| &self.values[*i])
    }
    pub fn get(&self, e: ExprId, ordinal: u32) -> Option<&CaptureMode> {
        work::count(Counter::OwnershipSourceRows);
        self.rows.get(&(e, ordinal)).map(|i| &self.values[*i])
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capture_positions_keep_first_rows_and_shared_graph_inputs() {
        let row = |ordinal, usage| CaptureMode {
            expression: ExprId(3),
            ordinal,
            owner: NameSite::Expression(ExprId(7)),
            usage,
            context: CaptureContext::Escaping,
        };
        let rows = std::sync::Arc::new(CaptureRows::new(&[
            row(1, UseKind::Take),
            row(0, UseKind::Copy),
            row(1, UseKind::Mut),
        ]));
        let sibling = rows.clone();
        assert!(std::sync::Arc::ptr_eq(&rows, &sibling));
        assert_eq!(rows.first(ExprId(3)).unwrap().usage, UseKind::Take);
        assert_eq!(rows.get(ExprId(3), 1).unwrap().usage, UseKind::Take);
        assert_eq!(sibling.get(ExprId(3), 0).unwrap().usage, UseKind::Copy);
        assert!(rows.get(ExprId(3), 2).is_none());
    }
}
