//! Private registration rounds over admitted products. Every stage borrows
//! from an existing partition; outputs retain only their measured subcharge.
use super::*;
use crate::runtime::{
    AdmittedRuntime, MemoryReservation, MemoryReservationPool, MemoryWeight, QueryRuntime,
    RuntimeErrors, RuntimeStats,
};
use FrontInstanceBootstrapError as Failure;

pub enum FrontInstanceBootstrapError {
    Bootstrap(FrontRegistrationBootstrapError),
    Front {
        reference: crate::store::Reference,
        error: Option<QueryError>,
        sources: SourceMap,
        owner: Arc<FrontBodyQueries>,
    },
    Discovery {
        error: QueryError,
        sources: SourceMap,
        owner: Arc<FrontBodyQueries>,
    },
}
impl std::fmt::Debug for FrontInstanceBootstrapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bootstrap(error) => f.debug_tuple("Bootstrap").field(error).finish(),
            Self::Front {
                reference, error, ..
            } => f
                .debug_struct("Front")
                .field("reference", reference)
                .field("error", error)
                .finish_non_exhaustive(),
            Self::Discovery { error, .. } => f.debug_tuple("Discovery").field(error).finish(),
        }
    }
}
fn admission_error(error: QueryError) -> Failure {
    Failure::Bootstrap(FrontRegistrationBootstrapError::Admission(error))
}
fn acceptance(error: FrontRegistrationFailure) -> Failure {
    Failure::Bootstrap(FrontRegistrationBootstrapError::Acceptance(error))
}
use hirsel_intern::jobs::Jobs;

pub struct FrontInstanceBootstrapAdmission {
    pub jobs: Jobs,
    pub weight: MemoryWeight,
    pub max_reads: usize,
    pub runtime: MemoryReservation,
    pub front: MemoryReservation,
    pub lower: MemoryReservation,
    pub text: MemoryReservation,
    pub syntax: MemoryReservation,
    pub discovery: MemoryReservation,
    pub metadata: MemoryReservation,
}

pub struct FrontInstanceBootstrap {
    definitions: Vec<DefPath>,
    registrations: Vec<Arc<AcceptedFrontRegistration>>,
    rounds: usize,
    stats: RuntimeStats,
    _metadata: MemoryReservation,
}
impl FrontInstanceBootstrap {
    pub fn definitions(&self) -> &[DefPath] {
        &self.definitions
    }
    pub fn registrations(&self) -> &[Arc<AcceptedFrontRegistration>] {
        &self.registrations
    }
    pub fn rounds(&self) -> usize {
        self.rounds
    }
    pub fn stats(&self) -> RuntimeStats {
        self.stats
    }
    /// Path cursors, query-root vectors and accepted handles. Stage products,
    /// syntax and discovery have separate existing reservation partitions.
    pub fn input_bytes(definitions: &[DefPath], generators: &[DefPath]) -> Option<u64> {
        metadata_bound(definitions, generators, None)
    }
}
fn metadata_bound(
    ordinary: &[DefPath],
    generators: &[DefPath],
    extra: Option<&DefPath>,
) -> Option<u64> {
    let count = ordinary
        .len()
        .checked_add(generators.len())?
        .checked_add(usize::from(extra.is_some()))?;
    let mut weight = crate::NativeMetadataWeight::default();
    weight.add(size_of::<FrontInstanceBootstrap>() + size_of::<FrontInstanceBootstrapAdmission>());
    weight.add(4 * size_of::<Vec<DefPath>>() + size_of::<Vec<crate::runtime::QueryId>>());
    // At growth, old/new cursor allocations can coexist with the current front
    // inventory. Each path has at most these three owned copies.
    for path in ordinary.iter().chain(generators).chain(extra) {
        for _ in 0..3 {
            weight.add(size_of::<DefPath>());
            weight.path(path);
        }
    }
    weight.add(count.checked_mul(size_of::<crate::runtime::QueryId>())?);
    weight.add(
        generators
            .len()
            .checked_add(usize::from(extra.is_some()))?
            .checked_mul(size_of::<Arc<AcceptedFrontRegistration>>())?,
    );
    Some(weight.bytes())
}
fn refused(message: impl Into<String>) -> Failure {
    admission_error(QueryError::Operational(hirsel_types::OperationalFailure {
        operation: "generated instance bootstrap admission".into(),
        message: message.into(),
    }))
}
fn pool(grant: MemoryReservation) -> Result<MemoryReservationPool, Failure> {
    MemoryReservationPool::new(grant)
        .map_err(|_| refused("partition cannot admit its reusable owner metadata"))
}
fn borrow(pool: &MemoryReservationPool) -> Result<MemoryReservation, Failure> {
    pool.take(pool.available_bytes())
        .ok_or_else(|| refused("stage partition is still owned by an earlier round"))
}
fn finish(
    runtime: &mut AdmittedRuntime,
    parent: &mut MemoryReservation,
    stats: &mut RuntimeStats,
) -> Result<(), Failure> {
    let grant = runtime
        .shutdown_with_reservation()
        .ok_or_else(|| refused("runtime reservation was already retired"))?;
    let round = runtime.stats();
    parent
        .join(grant)
        .map_err(|_| refused("runtime returned another reservation partition"))?;
    stats.claims = stats
        .claims
        .checked_add(round.claims)
        .ok_or_else(|| refused("runtime statistics overflow"))?;
    stats.polls = stats
        .polls
        .checked_add(round.polls)
        .ok_or_else(|| refused("runtime statistics overflow"))?;
    stats.waits = stats
        .waits
        .checked_add(round.waits)
        .ok_or_else(|| refused("runtime statistics overflow"))?;
    stats.wakes = stats
        .wakes
        .checked_add(round.wakes)
        .ok_or_else(|| refused("runtime statistics overflow"))?;
    stats.edges = stats
        .edges
        .checked_add(round.edges)
        .ok_or_else(|| refused("runtime statistics overflow"))?;
    stats.settled = stats
        .settled
        .checked_add(round.settled)
        .ok_or_else(|| refused("runtime statistics overflow"))?;
    stats.peak_bytes = stats.peak_bytes.max(round.peak_bytes);
    stats.workers = stats.workers.max(round.workers);
    Ok(())
}
impl QueryEngine {
    /// Discover and settle closed generated instances using only admitted
    /// front products. New heads invalidate old execution receipts: every
    /// subsequent round regenerates its accepted outputs in the full current
    /// namespace. No scalar front/evaluation controller is invoked.
    pub fn bootstrap_front_instances(
        &mut self,
        definitions: &[DefPath],
        roots: &[DefPath],
        generators: &[DefPath],
        admission: FrontInstanceBootstrapAdmission,
        errors: RuntimeErrors,
    ) -> Result<FrontInstanceBootstrap, Failure> {
        let required = FrontInstanceBootstrap::input_bytes(definitions, generators)
            .ok_or_else(|| refused("input metadata overflow"))?;
        if required > admission.metadata.bytes() {
            return Err(refused("input cursors exceed their metadata partition"));
        }
        if generators
            .iter()
            .enumerate()
            .any(|(i, p)| !definitions.contains(p) || generators[..i].contains(p))
            || roots.iter().any(|p| !definitions.contains(p))
        {
            return Err(refused(
                "roots and unique generator declarations must belong to the source inventory",
            ));
        }
        let FrontInstanceBootstrapAdmission {
            jobs,
            weight,
            max_reads,
            mut runtime,
            front,
            lower,
            text,
            syntax,
            discovery,
            mut metadata,
        } = admission;
        let front_pool = pool(front)?;
        let lower_pool = pool(lower)?;
        let text_pool = pool(text)?;
        let syntax_pool = pool(syntax)?;
        let discovery_pool = pool(discovery)?;
        let mut ordinary = Vec::with_capacity(definitions.len() - generators.len());
        ordinary.extend(
            definitions
                .iter()
                .filter(|p| !generators.contains(p))
                .cloned(),
        );
        let mut selected = Vec::with_capacity(generators.len());
        for path in generators {
            self.materialize(path).map_err(admission_error)?;
            let snapshot = self.snapshot(path).map_err(admission_error)?;
            if generator(&snapshot).is_none() {
                return Err(refused("selected declaration is not a generator"));
            }
            if instance_bodied(&snapshot) {
                // The declaration has no executable body, but remains a
                // required checked header for Growth and package Check.
                ordinary.reserve_exact(1);
                ordinary.push(path.clone());
            } else {
                selected.push(path.clone());
            }
        }
        selected.shrink_to_fit();
        let runtime_bytes = runtime.bytes();
        let mut accepted = Vec::new();
        let mut stats = RuntimeStats::default();
        // Same finite selection walk bound as discover_front_instances. Every
        // unfinished iteration must add a canonical instance or missing body.
        for round in 1..=65536 {
            runtime = self
                .prepare_private_inference(runtime)
                .map_err(admission_error)?;
            accepted.clear();
            if !selected.is_empty() {
                let front = self
                    .front_body_queries(&ordinary, weight, borrow(&front_pool)?, errors)
                    .map_err(admission_error)?;
                let lower = self
                    .front_lower_queries(
                        &ordinary,
                        front.clone(),
                        weight,
                        max_reads,
                        borrow(&lower_pool)?,
                        errors,
                    )
                    .map_err(admission_error)?;
                let registration = self
                    .front_registration_queries(
                        &selected,
                        lower.clone(),
                        weight,
                        borrow(&text_pool)?,
                        errors,
                    )
                    .map_err(admission_error)?;
                let capacity =
                    FrontRegistrationBootstrap::required_queries(ordinary.len(), selected.len())
                        .ok_or_else(|| refused("query inventory overflow"))?;
                let grant = runtime
                    .split(runtime_bytes)
                    .ok_or_else(|| refused("runtime partition unavailable"))?;
                let mut executor =
                    AdmittedRuntime::with_query_capacity(jobs, grant, errors, capacity)
                        .map_err(|e| refused(e.to_string()))?;
                executor
                    .register(FRONT_BODY_KIND, front.factory())
                    .map_err(refused)?;
                executor
                    .register_local(FRONT_LOWER_KIND, lower.factory())
                    .map_err(refused)?;
                executor
                    .register_local(FRONT_EVALUATE_KIND, lower.factory())
                    .map_err(refused)?;
                executor
                    .register_local(FRONT_REGISTRATION_KIND, registration.factory())
                    .map_err(refused)?;
                accepted.reserve_exact(selected.len());
                for path in &selected {
                    let query = registration.query(path).ok_or_else(|| {
                        refused("selected generator is outside the registration view")
                    })?;
                    let request = executor.request(&[query]);
                    let complete = executor.wait(request).map_err(|reference| {
                        Failure::Bootstrap(FrontRegistrationBootstrapError::Runtime {
                            reference,
                            error: registration
                                .failed(reference)
                                .and_then(|o| o.text().err())
                                .or_else(|| lower.failure(reference))
                                .or_else(|| {
                                    front
                                        .outcome(reference)
                                        .and_then(|o| o.outcome.result.err())
                                }),
                            sources: self.sources.clone(),
                            owner: registration.clone(),
                        })
                    })?;
                    let [product] = complete.as_slice() else {
                        return Err(refused("generator completed another root inventory"));
                    };
                    let output = registration
                        .outcome(*product)
                        .ok_or_else(|| refused("generator output is unavailable"))?;
                    accepted.push(
                        self.accept_front_registration(output, borrow(&syntax_pool)?)
                            .map_err(acceptance)?,
                    );
                }
                finish(&mut executor, &mut runtime, &mut stats)?;
                // Opaque outputs own only Data's measured reservation; all
                // completed producer inputs return before the discovery front.
                drop(executor);
                drop(registration);
                drop(lower);
                drop(front);
            }
            let bound = metadata_bound(&ordinary, &selected, None)
                .ok_or_else(|| refused("frontier metadata overflow"))?;
            if bound > metadata.bytes() {
                return Err(refused("frontier cursors exceed metadata partition"));
            }
            let mut paths = Vec::with_capacity(ordinary.len() + accepted.len());
            paths.extend(ordinary.iter().cloned());
            paths.extend(accepted.iter().map(|a| a.definition().clone()));
            let front = self
                .front_body_queries_with_registrations(
                    &paths,
                    &accepted,
                    weight,
                    borrow(&front_pool)?,
                    errors,
                )
                .map_err(admission_error)?;
            let capacity = paths
                .len()
                .checked_mul(2)
                .and_then(|n| n.checked_add(1))
                .ok_or_else(|| refused("front query inventory overflow"))?;
            let grant = runtime
                .split(runtime_bytes)
                .ok_or_else(|| refused("runtime partition unavailable"))?;
            let mut executor = AdmittedRuntime::with_query_capacity(jobs, grant, errors, capacity)
                .map_err(|e| refused(e.to_string()))?;
            executor
                .register(FRONT_BODY_KIND, front.factory())
                .map_err(refused)?;
            let mut queries = Vec::with_capacity(paths.len());
            for path in &paths {
                queries.push(
                    front
                        .query(path)
                        .ok_or_else(|| refused("front root missing"))?,
                );
            }
            if !queries.is_empty() {
                let request = executor.request(&queries);
                executor.wait(request).map_err(|reference| Failure::Front {
                    reference,
                    error: front
                        .outcome(reference)
                        .and_then(|o| o.outcome.result.err()),
                    sources: self.sources.clone(),
                    owner: front.clone(),
                })?;
            }
            finish(&mut executor, &mut runtime, &mut stats)?;
            drop(executor);
            drop(queries);
            let found = self
                .discover_front_instances(roots, &[front.clone()], borrow(&discovery_pool)?)
                .map_err(|error| Failure::Discovery {
                    error,
                    sources: self.sources.clone(),
                    owner: front.clone(),
                })?;
            let mut changed = false;
            for request in found.requests() {
                let key = hirsel_tir::InstanceBodies::key(&self.types, &request.actuals.arguments)
                    .ok_or_else(|| refused("discovery returned open actuals"))?;
                let path = instance_path(&request.declaration, key);
                if selected.contains(&path) {
                    continue;
                }
                let bound = metadata_bound(&ordinary, &selected, Some(&path))
                    .ok_or_else(|| refused("registered frontier metadata overflow"))?;
                if bound > metadata.bytes() {
                    return Err(refused("registered paths exceed metadata partition"));
                }
                let registered = self
                    .register_front_instance(request, borrow(&syntax_pool)?)
                    .map_err(acceptance)?;
                if registered.definition() != &path {
                    return Err(refused("canonical instance identity changed"));
                }
                selected.reserve_exact(1);
                selected.push(path);
                changed = true;
            }
            for path in found.pending_bodies() {
                if selected.contains(path) || ordinary.contains(path) {
                    continue;
                }
                if !self.declared(path) {
                    return Err(refused("discovery needs an unavailable source declaration"));
                }
                self.materialize(path).map_err(admission_error)?;
                let snapshot = self.snapshot(path).map_err(admission_error)?;
                if generator(&snapshot).is_some() {
                    return Err(refused("discovery needs an unregistered generator"));
                }
                let bound = metadata_bound(&ordinary, &selected, Some(path))
                    .ok_or_else(|| refused("discovered body metadata overflow"))?;
                if bound > metadata.bytes() {
                    return Err(refused("discovered bodies exceed metadata partition"));
                }
                ordinary.reserve_exact(1);
                ordinary.push(path.clone());
                changed = true;
            }
            if !changed {
                if !found.pending_bodies().is_empty() {
                    return Err(refused("instance discovery made no progress"));
                }
                drop(found);
                drop(front);
                let mut weight = crate::NativeMetadataWeight::default();
                weight.add(size_of::<FrontInstanceBootstrap>());
                weight.vec(&paths, |w, p| w.path(p));
                weight.flat_vec(&accepted);
                let retained = metadata
                    .split(weight.bytes())
                    .ok_or_else(|| refused("completed metadata exceeds its partition"))?;
                return Ok(FrontInstanceBootstrap {
                    definitions: paths,
                    registrations: accepted,
                    rounds: round,
                    stats,
                    _metadata: retained,
                });
            }
            // New declaration heads changed V's universe. Retire the old view
            // before regenerating every selection in the next full namespace.
            drop(found);
            drop(front);
        }
        Err(refused("instance bootstrap selection expansion limit"))
    }
}
