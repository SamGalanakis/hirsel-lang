//! Parallel edges capture every source before changing any destination.
//! Temporary custody is diagnostic storage, absent from the ordinary unit.
use super::*;

pub(super) const TEMPORARY: u32 = u32::MAX;

pub(super) fn stage(plan: &mut LeafPlan) -> Result<(), String> {
    let mut edges: BTreeMap<EdgeId, Vec<NativeMarker>> = BTreeMap::new();
    for &(point, marker) in &plan.commits {
        if let Point::Edge(edge) = point {
            if marker.kind != 1 || marker.token.kind != 0 || marker.owner.kind != 0 {
                return Err("ownership edge has a non-SSA transition".into());
            }
            edges.entry(edge).or_default().push(marker);
        }
    }
    edges.retain(|_, rows| {
        rows.iter()
            .any(|row| row.token != row.owner && rows.iter().any(|other| other.token == row.owner))
    });
    if edges.is_empty() {
        return Ok(());
    }
    let staged: BTreeSet<_> = edges.keys().copied().collect();
    let originals: BTreeMap<_, _> = plan
        .markers
        .iter()
        .flat_map(|(id, markers)| markers.iter().map(|marker| (marker.id, *id)))
        .collect();
    let removable: BTreeSet<_> = edges
        .values()
        .flatten()
        .filter_map(|marker| {
            plan.commits
                .iter()
                .filter(|(_, at)| at.id == marker.id)
                .all(|(point, _)| matches!(point, Point::Edge(edge) if staged.contains(edge)))
                .then_some(marker.id)
        })
        .collect();
    let mut next = plan
        .markers
        .values()
        .flatten()
        .map(|marker| marker.id.0)
        .chain(plan.markers.keys().map(|id| id.0))
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or("ownership parallel marker overflow")?;
    let mut expanded = BTreeMap::new();
    let mut replaced = BTreeSet::new();
    for (edge, rows) in edges {
        let mut out = Vec::new();
        let mut incoming = Vec::new();
        let mut sources = BTreeSet::new();
        let mut targets = BTreeSet::new();
        for marker in rows {
            if !sources.insert(marker.token.index) || !targets.insert(marker.owner.index) {
                return Err(
                    "ownership parallel edge repeats an owned source or destination".into(),
                );
            }
            let logical = *originals
                .get(&marker.id)
                .ok_or("ownership parallel edge has no original marker")?;
            let temporary = NativeLocation {
                kind: TEMPORARY,
                index: logical.0,
            };
            let capture = NativeMarker {
                id: MarkerId(next),
                owner: temporary,
                ..marker
            };
            next = next
                .checked_add(1)
                .ok_or("ownership parallel marker overflow")?;
            let restore = NativeMarker {
                id: MarkerId(next),
                token: temporary,
                ..marker
            };
            next = next
                .checked_add(1)
                .ok_or("ownership parallel marker overflow")?;
            if removable.contains(&marker.id) && replaced.insert(marker.id) {
                plan.markers
                    .get_mut(&logical)
                    .unwrap()
                    .retain(|at| at.id != marker.id);
            }
            plan.markers
                .get_mut(&logical)
                .unwrap()
                .extend([capture, restore]);
            out.push((Point::Edge(edge), capture));
            incoming.push((Point::Edge(edge), restore));
        }
        out.extend(incoming);
        expanded.insert(edge, out);
    }
    let mut commits = Vec::new();
    for &(point, marker) in &plan.commits {
        if let Point::Edge(edge) = point {
            if let Some(rows) = expanded.remove(&edge) {
                commits.extend(rows);
                continue;
            }
            if staged.contains(&edge) {
                continue;
            }
        }
        commits.push((point, marker));
    }
    plan.commits = commits;
    Ok(())
}
