//! Ordered acceptance of generator text before capturing the final immutable
//! front. All bootstrap work uses explicit parts of the caller's existing grant.
use super::*;
use crate::runtime::{
    AdmittedRuntime, MemoryReservation, MemoryWeight, QueryRuntime, RuntimeErrors, RuntimeStats,
};
use hirsel_intern::jobs::Jobs;

/// Explicit ownership partitions. No process budget is created or enlarged by
/// bootstrap; the runtime partition includes its ordinary compiler worker stack.
pub struct FrontRegistrationBootstrapAdmission {
    pub jobs: Jobs,
    pub weight: MemoryWeight,
    pub max_reads: usize,
    pub runtime: MemoryReservation,
    pub front: MemoryReservation,
    pub lower: MemoryReservation,
    pub text: MemoryReservation,
    pub syntax: Vec<MemoryReservation>,
    pub metadata: MemoryReservation,
}

pub struct FrontRegistrationBootstrap {
    registrations: Vec<Arc<AcceptedFrontRegistration>>,
    stats: RuntimeStats,
    query_capacity: usize,
    _metadata: MemoryReservation,
}
impl FrontRegistrationBootstrap {
    pub fn registrations(&self) -> &[Arc<AcceptedFrontRegistration>] {
        &self.registrations
    }
    pub fn stats(&self) -> RuntimeStats {
        self.stats
    }
    pub fn query_capacity(&self) -> usize {
        self.query_capacity
    }
    /// Ordinary P, body-demand, Lower and Evaluate addresses, generator text
    /// addresses and the four factories. Includes no guessed output allowance.
    pub fn required_queries(ordinary: usize, generators: usize) -> Option<usize> {
        ordinary
            .checked_mul(4)?
            .checked_add(generators)?
            .checked_add(4)
    }
    /// Copied path cursors and the returned view vector. Bodies, text and
    /// accepted syntax keep the distinct producer reservations above.
    pub fn input_bytes(definitions: &[DefPath], generators: usize) -> Option<u64> {
        let mut weight = crate::NativeMetadataWeight::default();
        weight.add(size_of::<Self>() + size_of::<FrontRegistrationBootstrapAdmission>());
        weight.add(size_of::<Vec<DefPath>>());
        weight.add(definitions.len().checked_mul(size_of::<DefPath>())?);
        for path in definitions {
            weight.path(path);
        }
        weight.add(generators.checked_mul(size_of::<Arc<AcceptedFrontRegistration>>())?);
        weight.add(generators.checked_mul(size_of::<MemoryReservation>())?);
        Some(weight.bytes())
    }
}

pub enum FrontRegistrationBootstrapError {
    Admission(QueryError),
    Runtime {
        reference: crate::store::Reference,
        error: Option<QueryError>,
        sources: SourceMap,
        owner: Arc<FrontRegistrationQueries>,
    },
    Acceptance(FrontRegistrationFailure),
}
impl std::fmt::Debug for FrontRegistrationBootstrapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Admission(error) => f.debug_tuple("Admission").field(error).finish(),
            Self::Runtime {
                reference, error, ..
            } => f
                .debug_struct("Runtime")
                .field("reference", reference)
                .field("error", error)
                .finish_non_exhaustive(),
            Self::Acceptance(error) => f.debug_tuple("Acceptance").field(error).finish(),
        }
    }
}
impl QueryEngine {
    /// Select written generator declarations without executing or checking a
    /// body. The caller keeps this order as the source publication prefix.
    pub fn front_generator_roots(
        &mut self,
        definitions: &[DefPath],
    ) -> Result<Vec<DefPath>, QueryError> {
        let mut roots = Vec::new();
        for path in definitions {
            if self
                .source_facts(path)
                .is_some_and(|facts| !facts.generator)
            {
                continue;
            }
            self.materialize(path)?;
            if generator(self.snapshot(path)?.as_ref()).is_some() {
                roots.push(path.clone());
            }
        }
        Ok(roots)
    }
    /// Complete private generator tasks and ordered acceptance under a bounded
    /// standalone S5 runtime. It is shut down before returning final-front
    /// inputs; no legacy mode/evaluation controller runs in this helper.
    pub fn bootstrap_front_registrations(
        &mut self,
        definitions: &[DefPath],
        generators: &[DefPath],
        admission: FrontRegistrationBootstrapAdmission,
        errors: RuntimeErrors,
    ) -> Result<FrontRegistrationBootstrap, FrontRegistrationBootstrapError> {
        use FrontRegistrationBootstrapError as Failure;
        let refused = |message: &str| {
            Failure::Admission(QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "generator bootstrap admission".into(),
                message: message.into(),
            }))
        };
        let required = FrontRegistrationBootstrap::input_bytes(definitions, generators.len())
            .and_then(|bytes| {
                let spare = admission.syntax.capacity().checked_sub(generators.len())?;
                bytes.checked_add(spare.checked_mul(size_of::<MemoryReservation>())? as u64)
            })
            .ok_or_else(|| refused("bootstrap input inventory overflow"))?;
        if required > admission.metadata.bytes() || admission.syntax.len() != generators.len() {
            return Err(refused(
                "bootstrap metadata or per-generator syntax grants are incomplete",
            ));
        }
        if generators
            .iter()
            .enumerate()
            .any(|(index, path)| !definitions.contains(path) || generators[..index].contains(path))
        {
            return Err(refused(
                "generator roots must be unique members of the source inventory",
            ));
        }
        let mut ordinary = Vec::with_capacity(definitions.len() - generators.len());
        ordinary.extend(
            definitions
                .iter()
                .filter(|path| !generators.contains(path))
                .cloned(),
        );
        let capacity =
            FrontRegistrationBootstrap::required_queries(ordinary.len(), generators.len())
                .ok_or_else(|| refused("bootstrap query inventory overflow"))?;
        let FrontRegistrationBootstrapAdmission {
            jobs,
            weight,
            max_reads,
            runtime,
            front,
            lower,
            text,
            syntax,
            metadata,
        } = admission;
        let runtime = self
            .prepare_private_inference(runtime)
            .map_err(Failure::Admission)?;
        let front = self
            .front_body_queries(&ordinary, weight, front, errors)
            .map_err(Failure::Admission)?;
        let lower = self
            .front_lower_queries(&ordinary, front.clone(), weight, max_reads, lower, errors)
            .map_err(Failure::Admission)?;
        let registration = self
            .front_registration_queries(generators, lower.clone(), weight, text, errors)
            .map_err(Failure::Admission)?;
        let mut runtime = AdmittedRuntime::with_query_capacity(jobs, runtime, errors, capacity)
            .map_err(|_| {
                refused("bootstrap runtime cannot admit its query inventory and worker stacks")
            })?;
        runtime
            .register(FRONT_BODY_KIND, front.factory())
            .map_err(refused)?;
        runtime
            .register_local(FRONT_LOWER_KIND, lower.factory())
            .map_err(refused)?;
        runtime
            .register_local(FRONT_EVALUATE_KIND, lower.factory())
            .map_err(refused)?;
        runtime
            .register_local(FRONT_REGISTRATION_KIND, registration.factory())
            .map_err(refused)?;
        let result = (|| {
            let mut registrations = Vec::with_capacity(generators.len());
            for (path, allowance) in generators.iter().zip(syntax) {
                let query = registration
                    .query(path)
                    .ok_or_else(|| refused("selected declaration is not an admitted generator"))?;
                let request = runtime.request(&[query]);
                let complete = runtime
                    .wait(request)
                    .map_err(|reference| Failure::Runtime {
                        reference,
                        error: registration
                            .failed(reference)
                            .and_then(|output| output.text().err())
                            .or_else(|| lower.failure(reference))
                            .or_else(|| {
                                front
                                    .outcome(reference)
                                    .and_then(|output| output.outcome.result.err())
                            }),
                        sources: self.sources.clone(),
                        owner: registration.clone(),
                    })?;
                let [product] = complete.as_slice() else {
                    return Err(refused("bootstrap completed another root inventory"));
                };
                let output = registration
                    .outcome(product.clone())
                    .ok_or_else(|| refused("bootstrap completed text is unavailable"))?;
                registrations.push(
                    self.accept_front_registration(output, allowance)
                        .map_err(Failure::Acceptance)?,
                );
            }
            Ok(registrations)
        })();
        runtime.shutdown();
        let stats = runtime.stats();
        drop(runtime);
        Ok(FrontRegistrationBootstrap {
            registrations: result?,
            stats,
            query_capacity: capacity,
            _metadata: metadata,
        })
    }
}
