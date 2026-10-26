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
    pub product: ProductRef,
    pub equations: LocalDemandEquations,
}

pub struct Plan {
    query: QueryId,
    action: Action,
    inputs: Vec<Input>,
    entries: Vec<usize>,
}

pub struct Selection {
    pub product: ProductRef,
    units: Vec<UnitDemand>,
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
    pub fn dependencies(&self) -> impl Iterator<Item = ProductRef> + '_ {
        self.inputs.iter().map(|input| input.product)
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
