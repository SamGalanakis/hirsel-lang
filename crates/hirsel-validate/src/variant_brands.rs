//! Immutable producer inputs for parameter authority and inactive payload contracts.
use super::*;

#[derive(Clone)]
pub(super) enum Producer {
    Transfer(PlaceId),
    Closure(BodyId),
    Call {
        callable: PlaceId,
    },
    Variant {
        definition: DefPath,
        ordinal: u32,
        fields: usize,
    },
    Unknown,
}
pub(super) struct Inputs {
    pub roots: Vec<Vec<Producer>>,
}
impl Inputs {
    pub fn new(body: &ReadBody<'_>) -> Self {
        let mut roots = vec![Vec::new(); body.locals.len()];
        for (index, local) in body.locals.iter().enumerate() {
            if matches!(local.storage, Storage::Parameter | Storage::Capture) {
                roots[index].push(Producer::Unknown);
            }
        }
        for block in &body.blocks {
            if let Terminator::Call {
                callee,
                returns,
                arguments,
                ..
            } = &block.terminator
            {
                if let Some(CallReturn { destination, .. }) = returns {
                    let producer = match callee {
                        Callee::Indirect { value, .. }
                            if body.places[destination.0 as usize].projection.is_empty() =>
                        {
                            Producer::Call {
                                callable: value.place,
                            }
                        }
                        _ => Producer::Unknown,
                    };
                    roots[body.places[destination.0 as usize].root.0 as usize].push(producer);
                }
                for argument in arguments.iter().filter(|a| a.value.usage == UseKind::Mut) {
                    roots[body.places[argument.value.place.0 as usize].root.0 as usize]
                        .push(Producer::Unknown);
                }
            }
            for instruction in &block.instructions {
                work::count(Counter::ValidateDefinitionInputRows);
                let (place, producer) = match &instruction.kind {
                    InstructionKind::Assign { destination, value } => {
                        let producer = if body.places[destination.0 as usize].projection.is_empty()
                        {
                            match &body.rvalues[value.0 as usize] {
                                Rvalue::Use(value) => Producer::Transfer(value.place),
                                Rvalue::Closure { body, .. } => Producer::Closure(*body),
                                Rvalue::Construct {
                                    constructor:
                                        Constructor::Variant {
                                            definition,
                                            variant,
                                        },
                                    fields,
                                    ..
                                } => Producer::Variant {
                                    definition: definition.clone(),
                                    ordinal: *variant,
                                    fields: fields.len(),
                                },
                                _ => Producer::Unknown,
                            }
                        } else {
                            Producer::Unknown
                        };
                        (*destination, producer)
                    }
                    InstructionKind::Replace { destination, .. } => {
                        (*destination, Producer::Unknown)
                    }
                    InstructionKind::BeginLoan {
                        place,
                        mutable: true,
                        ..
                    } => (*place, Producer::Unknown),
                    _ => continue,
                };
                roots[body.places[place.0 as usize].root.0 as usize].push(producer);
            }
        }
        Self { roots }
    }
}
