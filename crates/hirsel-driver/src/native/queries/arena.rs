//! Settled arena component records. Dependency outputs are checked before
//! loading a prior result; no Value or layout table participates in a hit.
use super::*;
use hirsel_engine::store::{Action, Domain, Reference, ResultStore};

pub const EQUATIONS_DOMAIN: Domain = Domain {
    kind: 0x4e00_0010,
    schema: 1,
};

pub struct EquationProduct {
    pub owner: InstanceAddress,
    pub reference: Reference,
}

/// Query-local counters, suitable for the no-unchanged-Value assertion.
#[derive(Default, Debug)]
pub struct ArenaQueryCounts {
    pub hits: u64,
    pub misses: u64,
}

/// Ordinary product references are never proof capabilities. Independent
/// native import, C, Ready and cross-unit bindings still check actual units.
pub fn arena_query(
    store: &dyn ResultStore,
    key: &ComponentKey,
    members: &[EquationProduct],
    own_candidates: Option<&ArenaSummary>,
    outgoing: &[(InstanceAddress, Vec<u32>, ProductRef)],
    counts: &mut ArenaQueryCounts,
) -> Result<Arc<ArenaSummary>, String> {
    let address = key.address()?;
    if members.len() != key.members.len()
        || members
            .iter()
            .zip(&key.members)
            .any(|(m, a)| &m.owner != a || m.reference.domain != EQUATIONS_DOMAIN)
        || outgoing
            .windows(2)
            .any(|w| !w[0].0.canonical_cmp(&w[1].0).is_lt())
    {
        return Err("arena query inputs differ from canonical component".into());
    }
    // Complete action binds the exact equation archive content and only the
    // settled outgoing facet outputs read by this stratum. Producer actions
    // are excluded: equal recomputation backdates canonical output identity.
    let mut dependencies: Vec<_> = members.iter().map(|m| m.reference).collect();
    match (key.stratum, own_candidates) {
        (ArenaStratum::Candidates, None) => {}
        (ArenaStratum::Chosen, Some(own)) => {
            let mut candidates_key = key.clone();
            candidates_key.stratum = ArenaStratum::Candidates;
            if own.product.address != candidates_key.address()? {
                return Err("Chosen reads another Candidates component".into());
            }
            dependencies.push(
                own.product
                    .archive(hirsel_engine::runtime::ArchiveAdmission::Retained)
                    .map_err(|e| e.to_string())?
                    .reference(),
            );
        }
        _ => return Err("invalid arena stratum read".into()),
    }
    dependencies.extend(
        outgoing
            .iter()
            .map(|(_, _, p)| {
                p.archive(hirsel_engine::runtime::ArchiveAdmission::Retained)
                    .map(|archive| archive.reference())
                    .map_err(|e| e.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?,
    );
    let outgoing_rows: Vec<_> = outgoing
        .iter()
        .map(|(a, xs, _)| (a.instance.encoding(), xs))
        .collect();
    let own_rows: Vec<_> = own_candidates.map_or_else(Vec::new, |own| {
        own.candidates
            .iter()
            .map(|(a, xs)| (a.instance.encoding(), xs))
            .collect()
    });
    let bytes =
        serde_json::to_vec(&(dependencies, outgoing_rows, own_rows)).map_err(|e| e.to_string())?;
    let action = Action(ContentHash::of_parts(
        "hirsel.native.arena-action.v1",
        [
            address.0.0.as_slice(),
            crate::COMPILER_IDENTITY.as_slice(),
            bytes.as_slice(),
        ],
    ));
    let domain = Domain {
        kind: match key.stratum {
            ArenaStratum::Candidates => ARENA_CANDIDATES_KIND,
            ArenaStratum::Chosen => ARENA_CHOSEN_KIND,
        },
        schema: 1,
    };
    if let Some(previous) = store.lookup_product(address)
        && previous.action == action
        && previous.reference.domain == domain
        && let Some(bytes) = store.get_content(previous.reference)
        && let Ok(rows) = serde_json::from_slice::<Vec<(Vec<u8>, Vec<u32>)>>(&bytes)
        && rows.len() == key.members.len()
        && rows.iter().zip(&key.members).all(|((owner, xs), a)| {
            owner == a.instance.encoding()
                && xs.windows(2).all(|w| w[0] < w[1])
                && (key.stratum == ArenaStratum::Candidates || xs.len() <= 1)
        })
        && let Some(archive) = store.copy_product_archive(previous.reference)
    {
        counts.hits += 1;
        return Ok(Arc::new(ArenaSummary {
            retained_owner: None,
            product: ProductRef::import_archive(address, action, archive),
            candidates: key
                .members
                .iter()
                .cloned()
                .zip(rows.into_iter().map(|(_, xs)| xs))
                .collect(),
        }));
    }
    counts.misses += 1;
    let local: Vec<_> = members
        .iter()
        .map(|m| {
            let bytes = store
                .get_content(m.reference)
                .ok_or("missing local arena equations")?;
            let equations: LocalArenaEquations =
                serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            equations.validate()?;
            if equations.owner()
                != &(
                    m.owner.instance.definition().clone(),
                    m.owner.instance.digest(),
                )
            {
                return Err("local arena equations name another instance".into());
            }
            Ok((m.owner.clone(), Arc::new(equations)))
        })
        .collect::<Result<_, String>>()?;
    let external: Vec<_> = outgoing
        .iter()
        .map(|(a, xs, _)| (a.clone(), xs.clone()))
        .collect();
    let candidates = solve_arena_component(key, &local, own_candidates, &external)?;
    let rows: Vec<_> = candidates
        .iter()
        .map(|(a, xs)| (a.instance.encoding(), xs))
        .collect();
    let bytes = serde_json::to_vec(&rows).map_err(|e| e.to_string())?;
    let record = store
        .put_content(domain, &bytes)
        .ok_or("arena product publication failed")?;
    store.associate_product(address, action, record);
    Ok(Arc::new(ArenaSummary {
        retained_owner: None,
        product: ProductRef::import_archive(
            address,
            action,
            store
                .admit_product_archive(record, bytes.into())
                .ok_or("native product archive is unavailable or exceeds admission")?,
        ),
        candidates,
    }))
}
