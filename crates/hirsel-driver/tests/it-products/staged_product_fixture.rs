//! Explicit request owners for staged-product replay and authority controls.
use hirsel_engine::{runtime::*, *};
use std::sync::Arc;

pub(super) struct StagedProducts {
    pub runtime: AdmittedRuntime,
    pub front: Arc<FrontBodyQueries>,
    pub lower: Arc<FrontLowerQueries>,
    pub budget: Arc<ProcessBudget>,
}
impl StagedProducts {
    pub fn capture(engine: &mut QueryEngine, jobs: usize) -> Result<Self, QueryError> {
        let budget = ProcessBudget::new(1 << 30);
        let errors = errors();
        let weight = MemoryWeight {
            retained: 2 << 20,
            scratch: 8 << 20,
            stack: 0,
        };
        let definitions = engine.front_source_roots();
        let roots = engine.front_staged_roots(&definitions)?;
        if !roots.is_empty() {
            let front = engine.front_body_queries(
                &definitions,
                weight,
                budget.reserve(16 << 20).unwrap(),
                errors,
            )?;
            let stage = FrontStageQueries::new(
                &roots,
                front.clone(),
                weight,
                budget.reserve(1 << 20).unwrap(),
                errors,
            )?;
            let mut runtime = AdmittedRuntime::new(
                hirsel_intern::jobs::Jobs::new(jobs),
                budget.reserve(512 << 20).unwrap(),
                errors,
            )
            .unwrap();
            runtime.register(FRONT_BODY_KIND, front.factory()).unwrap();
            runtime.register(FRONT_STAGE_KIND, stage.factory()).unwrap();
            for path in roots {
                let request = runtime.request(&[stage.query(&path).unwrap()]);
                let products = runtime.wait(request).map_err(|reference| {
                    stage
                        .failure(reference)
                        .unwrap_or(QueryError::NotImplemented)
                })?;
                let output = stage.outcome(products[0]).unwrap();
                let bytes = output.installation_bytes()?;
                engine.accept_front_staged(&output, budget.reserve(bytes).unwrap())?;
            }
            runtime.shutdown();
        }
        let front = engine.front_body_queries(
            &definitions,
            weight,
            budget.reserve(16 << 20).unwrap(),
            errors,
        )?;
        let lower = engine.front_lower_queries(
            &definitions,
            front.clone(),
            weight,
            4096,
            budget.reserve(32 << 20).unwrap(),
            errors,
        )?;
        let mut runtime = AdmittedRuntime::new(
            hirsel_intern::jobs::Jobs::new(jobs),
            budget.reserve(512 << 20).unwrap(),
            errors,
        )
        .unwrap();
        runtime.register(FRONT_BODY_KIND, front.factory()).unwrap();
        runtime
            .register_local(FRONT_LOWER_KIND, lower.factory())
            .unwrap();
        runtime
            .register_local(FRONT_EVALUATE_KIND, lower.factory())
            .unwrap();
        Ok(Self {
            runtime,
            front,
            lower,
            budget,
        })
    }
}

fn errors() -> RuntimeErrors {
    let reference = |text: &[u8]| {
        hirsel_engine::store::Domain {
            kind: 0x4600_00ff,
            schema: 1,
        }
        .reference(text)
        .unwrap()
    };
    RuntimeErrors {
        resource: reference(b"resource"),
        cycle: reference(b"cycle"),
        cancelled: reference(b"cancelled"),
        protocol: reference(b"protocol"),
        panicked: reference(b"panic"),
    }
}
