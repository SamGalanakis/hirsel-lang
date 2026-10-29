//! Body-free physical guard selection over settled original instance records.
//! Neither this ordinary query product nor a true hint authorizes sealing.
use super::*;
use hirsel_engine::runtime::QueryId;
use hirsel_engine::store::{Action, Address, Domain, ResultStore};
use hirsel_rc::seal_demand::{
    UnitDemand,
    equations::{Function, LocalDemandEquations, solve},
};

pub const SEAL_DEMAND_KIND: u32 = 0x4e00_002e;
const OUTPUT: Domain = Domain {
    kind: SEAL_DEMAND_KIND,
    schema: 1,
};

/// Callers supply already independently admitted instance descriptors. The
/// associations below check current outputs; they confer no C authority.
#[derive(Clone)]
pub struct Input {
    pub query: QueryId,
    pub product: ProductRef,
    pub equations: LocalDemandEquations,
}

#[derive(Clone)]
pub struct Plan {
    query: QueryId,
    action: Action,
    inputs: Vec<Input>,
    entries: Vec<usize>,
}

pub struct Selection {
    pub product: ProductRef,
    units: Vec<UnitDemand>,
    allowances: Vec<Arc<hirsel_engine::runtime::MemoryReservation>>,
}
impl Selection {
    pub fn unit(&self, owner: &Function) -> Option<&UnitDemand> {
        self.units
            .binary_search_by(|unit| unit.owner().cmp(owner))
            .ok()
            .map(|i| &self.units[i])
    }
}

impl Plan {
    /// Plan plus the largest retained per-value selection. Count owned vector
    /// and path allocations; serialized output size is not a memory allowance.
    pub fn admission_bytes<'a>(locals: impl IntoIterator<Item = &'a LocalDemandEquations>) -> u64 {
        let mut weight = hirsel_engine::NativeMetadataWeight::default();
        weight.add(size_of::<GuardTasks>() + size_of::<Plan>() + 4096);
        for local in locals {
            weight.add(size_of::<Input>() + local.retained_bytes() as usize);
            weight.add(size_of::<UnitDemand>() + local.value_count() as usize * size_of::<u32>());
            weight.path(&local.owner().0);
            weight.add(size_of::<usize>() + size_of::<QueryId>());
        }
        weight.bytes()
    }

    fn scratch_bytes(&self) -> u64 {
        let mut n = 0usize;
        let mut closures = 0usize;
        let mut flow = 0usize;
        let mut calls = 0usize;
        let mut arguments = 0usize;
        let mut captures = 0usize;
        let mut returns = 0usize;
        let mut copied = 0usize;
        for input in &self.inputs {
            let size = input.equations.flow_size();
            n = n.saturating_add(size.values);
            closures = closures.saturating_add(size.closures);
            flow = flow.saturating_add(size.flow);
            calls = calls.saturating_add(size.calls);
            arguments = arguments.saturating_add(size.arguments);
            captures = captures.saturating_add(size.captures);
            returns = returns.saturating_add(size.returns);
            copied = copied.saturating_add(
                input.equations.retained_bytes() as usize + size_of::<LocalDemandEquations>(),
            );
        }
        // A call can bind every formal target, each return and each closure
        // environment. Origins contain only actual closure definitions. Bound
        // the existing BTree worklists using the same Rust-node estimator as
        // other native metadata; no guess from serialized byte counts.
        let edges = flow
            .saturating_add(arguments.saturating_mul(self.inputs.len()))
            .saturating_add(calls.saturating_mul(returns))
            .saturating_add(calls.saturating_mul(closures).saturating_mul(captures));
        let origins = n.saturating_mul(closures);
        let mut weight = hirsel_engine::NativeMetadataWeight::default();
        weight.add(copied);
        weight.add(n.saturating_mul(
            size_of::<std::collections::BTreeSet<usize>>() * 2
                + size_of::<bool>()
                + size_of::<usize>(),
        ));
        // Empty roots and separate per-value trees also consume allocations.
        weight.tree::<usize, ()>(edges);
        weight.tree::<(usize, usize), ()>(origins.saturating_mul(2));
        for _ in 0..n.saturating_mul(2) {
            weight.tree::<(usize, usize), ()>(0);
        }
        weight.tree::<Function, usize>(self.inputs.len());
        for input in &self.inputs {
            weight.path(&input.equations.owner().0);
        }
        weight.bytes()
    }
    /// Planning describes dependencies only. It opens no Value or source body
    /// and runs no solver. Explicit roots exclude unused generic templates.
    pub fn new(
        inputs: Vec<Input>,
        roots: &[Function],
        target: ContentHash,
        policy: ContentHash,
    ) -> Result<Self, String> {
        if inputs
            .windows(2)
            .any(|w| w[0].equations.owner() >= w[1].equations.owner())
            || roots.windows(2).any(|w| w[0] >= w[1])
        {
            return Err("noncanonical seal-demand query inputs".into());
        }
        for input in &inputs {
            if input.query.address != input.product.address
                || !matches!(
                    input.query.kind,
                    INSTANCE_KIND | super::instance::CALLABLE_VALUE_KIND
                )
            {
                return Err("seal-demand dependency has another query stage or address".into());
            }
            input.equations.validate()?;
        }
        let entries = roots
            .iter()
            .map(|root| {
                inputs
                    .binary_search_by(|input| input.equations.owner().cmp(root))
                    .map_err(|_| "missing seal-demand original root".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let owners = inputs
            .iter()
            .map(|input| input.equations.owner())
            .collect::<Vec<_>>();
        let selection =
            serde_json::to_vec(&(owners, roots, target, policy)).map_err(|e| e.to_string())?;
        let query = QueryId {
            kind: SEAL_DEMAND_KIND,
            address: Address(ContentHash::of_parts(
                "hirsel.native.seal-demand-address.v1",
                [selection.as_slice()],
            )),
        };
        let records = inputs
            .iter()
            .map(|input| {
                // Bind ordinary descriptor content, not its producer action. Equal
                // recomputation preserves the physical selection's complete action.
                (
                    &input.equations,
                    (input.query.kind, input.query.address.0),
                    input.product.address.0,
                    input.product.record,
                )
            })
            .collect::<Vec<_>>();
        let bytes = serde_json::to_vec(&(
            crate::COMPILER_IDENTITY.as_slice(),
            query.address.0,
            records,
        ))
        .map_err(|e| e.to_string())?;
        let action = Action(ContentHash::of_parts(
            "hirsel.native.seal-demand-action.v1",
            [bytes.as_slice()],
        ));
        Ok(Self {
            query,
            action,
            inputs,
            entries,
        })
    }

    pub fn query_id(&self) -> QueryId {
        self.query
    }
    pub fn retained_bytes(&self) -> u64 {
        Self::admission_bytes(self.inputs.iter().map(|input| &input.equations))
    }
    pub fn dependencies(&self) -> impl Iterator<Item = ProductRef> + '_ {
        self.inputs.iter().map(|input| input.product.clone())
    }

    /// Called only after the advertised dependencies settle. Current C and
    /// source checks remain the enclosing query owner's independent obligation.
    pub fn execute(&self, store: &dyn ResultStore) -> Result<Selection, String> {
        for input in &self.inputs {
            let current = store
                .lookup_product(input.product.address)
                .ok_or("missing seal-demand instance association")?;
            if current.action != input.product.action
                || current.reference != input.product.record
                || store.get_content(input.product.record).is_none()
            {
                return Err("seal-demand instance dependency changed".into());
            }
        }
        if let Some(previous) = store.lookup_product(self.query.address)
            && previous.action == self.action
            && previous.reference.domain == OUTPUT
            && let Some(bytes) = store.get_content(previous.reference)
            && let Ok(units) = serde_json::from_slice::<Vec<UnitDemand>>(&bytes)
            && self.check(&units).is_ok()
        {
            return Ok(Selection {
                product: ProductRef {
                    address: self.query.address,
                    action: self.action,
                    record: previous.reference,
                },
                units,
                allowances: vec![],
            });
        }
        let equations = self
            .inputs
            .iter()
            .map(|input| input.equations.clone())
            .collect::<Vec<_>>();
        let solved = solve(&equations, &self.entries)?;
        let units = equations
            .iter()
            .enumerate()
            .map(|(i, original)| solved.for_unit(i, original))
            .collect::<Result<Vec<_>, _>>()?;
        self.check(&units)?;
        let bytes = serde_json::to_vec(&units).map_err(|e| e.to_string())?;
        let record = store
            .put_content(OUTPUT, &bytes)
            .ok_or("seal-demand output publication failed")?;
        store.associate_product(self.query.address, self.action, record);
        Ok(Selection {
            product: ProductRef {
                address: self.query.address,
                action: self.action,
                record,
            },
            units,
            allowances: vec![],
        })
    }

    fn check(&self, units: &[UnitDemand]) -> Result<(), String> {
        if units.len() != self.inputs.len() {
            return Err("seal-demand output member count".into());
        }
        for (unit, input) in units.iter().zip(&self.inputs) {
            unit.validate()?;
            if unit.owner() != input.equations.owner()
                || unit.original_value_count() != input.equations.value_count()
            {
                return Err("seal-demand output owner differs".into());
            }
        }
        Ok(())
    }
}

/// One immutable metadata query. Creation performs no dependency work and
/// does not run the solver. Its snapshot owner admits retained plan storage.
pub struct GuardTasks {
    store: Arc<dyn ResultStore>,
    plan: Plan,
    weight: hirsel_engine::runtime::MemoryWeight,
    output: std::sync::OnceLock<Arc<Selection>>,
    failure: Reference,
    _allowances: Vec<Arc<hirsel_engine::runtime::MemoryReservation>>,
}

impl GuardTasks {
    pub fn new(
        store: Arc<dyn ResultStore>,
        plan: Plan,
        weight: hirsel_engine::runtime::MemoryWeight,
        admitted_bytes: u64,
        allowances: Vec<Arc<hirsel_engine::runtime::MemoryReservation>>,
    ) -> Result<Arc<Self>, String> {
        if (std::mem::size_of::<GuardTask>() + plan.inputs.len() * size_of::<QueryId>()) as u64
            > weight.retained
        {
            return Err("seal-demand continuation exceeds retained allowance".into());
        }
        let needed = plan.retained_bytes();
        if needed > admitted_bytes
            || admitted_bytes > allowances.iter().map(|a| a.bytes()).sum::<u64>()
        {
            return Err("seal-demand metadata exceeds its original admission".into());
        }
        if plan.scratch_bytes() > weight.scratch {
            return Err(format!(
                "seal-demand solver needs {} scratch bytes within {} admitted",
                plan.scratch_bytes(),
                weight.scratch
            ));
        }
        let failure = store
            .put_content(OUTPUT, b"seal-demand dependency or resource refusal")
            .ok_or("seal-demand failure transport refused")?;
        Ok(Arc::new(Self {
            store,
            plan,
            weight,
            output: Default::default(),
            failure,
            _allowances: allowances,
        }))
    }

    pub fn query_id(&self) -> QueryId {
        self.plan.query_id()
    }
    pub fn factory(self: &Arc<Self>) -> Arc<dyn hirsel_engine::runtime::QueryFactory> {
        Arc::new(GuardFactory(self.clone()))
    }
    pub fn selection(&self) -> Option<Arc<Selection>> {
        self.output.get().cloned()
    }
    pub fn accepts(&self, product: ProductRef) -> bool {
        self.output
            .get()
            .is_some_and(|output| output.product == product)
    }
}

struct GuardFactory(Arc<GuardTasks>);
impl hirsel_engine::runtime::QueryFactory for GuardFactory {
    fn create(
        &self,
        key: QueryId,
    ) -> Result<Box<dyn hirsel_engine::runtime::QueryTask>, Reference> {
        if key != self.0.query_id() {
            return Err(self.0.failure);
        }
        Ok(Box::new(GuardTask {
            owner: self.0.clone(),
            terminal: None,
        }))
    }
}
struct GuardTask {
    owner: Arc<GuardTasks>,
    terminal: Option<Result<ProductRef, Reference>>,
}
impl hirsel_engine::runtime::QueryTask for GuardTask {
    fn id(&self) -> QueryId {
        self.owner.query_id()
    }
    fn weight(&self) -> hirsel_engine::runtime::MemoryWeight {
        hirsel_engine::runtime::MemoryWeight {
            retained: (std::mem::size_of::<Self>()
                + self.owner.plan.inputs.len() * size_of::<QueryId>()) as u64,
            ..self.owner.weight
        }
    }
    fn continuation_bytes(&self) -> u64 {
        std::mem::size_of::<Self>() as u64
    }
    fn poll(
        &mut self,
        cx: &mut dyn hirsel_engine::runtime::QueryContext,
    ) -> hirsel_engine::runtime::QueryPoll {
        use hirsel_engine::runtime::QueryPoll;
        use std::task::Poll;
        let finish = |result| match result {
            Ok(p) => QueryPoll::Complete(p),
            Err(e) => QueryPoll::Failed(e),
        };
        if let Some(result) = self.terminal.clone() {
            return finish(result);
        }
        let mut waiting = vec![];
        for input in &self.owner.plan.inputs {
            let expected = input.product.clone();
            let key = input.query;
            match cx.read(key) {
                Poll::Pending => waiting.push(key),
                Poll::Ready(Err(error)) => {
                    self.terminal = Some(Err(error));
                    return finish(Err(error));
                }
                Poll::Ready(Ok(product)) if product == expected => (),
                Poll::Ready(Ok(_)) => {
                    self.terminal = Some(Err(self.owner.failure));
                    return finish(Err(self.owner.failure));
                }
            }
        }
        if !waiting.is_empty() {
            return QueryPoll::Waiting(waiting);
        }
        let result = self
            .owner
            .plan
            .execute(self.owner.store.as_ref())
            .and_then(|mut selection| {
                selection.allowances = self.owner._allowances.clone();
                let product = selection.product.clone();
                if let Some(previous) = self.owner.output.get() {
                    if previous.product != product {
                        return Err("seal-demand completion changed".into());
                    }
                } else {
                    self.owner
                        .output
                        .set(Arc::new(selection))
                        .map_err(|_| "seal-demand completed twice".to_string())?;
                }
                Ok(product)
            })
            .map_err(|error| {
                self.owner
                    .store
                    .put_content(OUTPUT, error.as_bytes())
                    .unwrap_or(self.owner.failure)
            });
        self.terminal = Some(result.clone());
        finish(result)
    }
}
