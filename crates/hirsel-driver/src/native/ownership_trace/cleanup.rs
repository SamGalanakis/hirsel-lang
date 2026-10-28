//! Queue cleanup authority comes from checked layout programs and source-bound
//! finalizer bodies, independently of the native queue log.
use super::{NativeError, ReadyProgram, Sites, StageName};
use hirsel_lir::{
    ir::SiteRange,
    layout_types::LayoutId,
    ownership_trace::native::{NativeProjection, resource_cleanup_program},
    stage::{Ready, ValidatedUnit},
};
use hirsel_run_schema::{
    Site,
    ownership_trace::{
        Projection,
        native::{CleanupBinding, CleanupLayouts},
    },
};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn derive(
    program: &ReadyProgram,
    units: &[&ValidatedUnit<Ready>],
    payloads: &BTreeMap<u32, BTreeSet<LayoutId>>,
    sites: &Sites<'_>,
) -> Result<CleanupLayouts, NativeError> {
    let mut result = BTreeMap::new();
    for (&descriptor, layouts) in payloads {
        let mut binding = None;
        let mut unsupported = false;
        for &layout in layouts {
            let Ok(completions) = resource_cleanup_program(&program.table, layout) else {
                // Transport can still succeed when the queue drains. An
                // unwitnessed retained Resource continues to refuse at detach.
                unsupported = true;
                break;
            };
            let candidate = completions
                .into_iter()
                .map(|completion| {
                    let finalizer = program
                        .table
                        .layout(completion.layout)
                        .and_then(|record| record.finalizer.as_ref())
                        .ok_or_else(|| {
                            NativeError::new(
                                StageName::Emission,
                                "cleanup completion lost its finalizer",
                            )
                        })?;
                    let instance = program.finalizers.get(&completion.layout);
                    let body = units
                        .iter()
                        .map(|unit| unit.unit())
                        .find(|body| Some(&body.header.instance) == instance);
                    let definition = body
                        .map(|body| {
                            program
                                .semantic_owners
                                .get(&body.header.definition)
                                .and_then(Option::as_ref)
                                .unwrap_or(&body.header.definition)
                        })
                        .unwrap_or(&finalizer.definition);
                    let mut source = body
                        .filter(|body| body.header.source_count != 0)
                        .and_then(|_| {
                            sites
                                .failure(
                                    "ownership",
                                    definition,
                                    SiteRange {
                                        source: 0,
                                        start: 0,
                                        end: 0,
                                    },
                                )
                                .ok()
                        })
                        .and_then(|failure| sites.failure_site(&failure))
                        .unwrap_or_else(|| Site {
                            definition: super::name(definition),
                            file: format!("<native-finalizer:{}>", super::name(definition)),
                            line: 0,
                            column: 0,
                        });
                    source.definition =
                        format!("{} [queued image cleanup]", super::name(definition));
                    Ok((
                        completion.step,
                        Box::new(CleanupBinding {
                            path: completion
                                .path
                                .into_iter()
                                .map(|part| match part {
                                    NativeProjection::Field(field) => Projection::Field { field },
                                    NativeProjection::VariantField { variant, field } => {
                                        Projection::VariantField { variant, field }
                                    }
                                })
                                .collect(),
                            source,
                        }),
                    ))
                })
                .collect::<Result<BTreeMap<_, _>, NativeError>>()?;
            if binding
                .as_ref()
                .is_some_and(|previous| previous != &candidate)
            {
                return Err(NativeError::new(
                    StageName::Emission,
                    "equivalent linked queue descriptors have different cleanup bindings",
                ));
            }
            binding = Some(candidate);
        }
        if !unsupported && let Some(binding) = binding {
            result.insert(descriptor, binding);
        }
    }
    // Seal both private levels in the same bulk-built form their retained
    // allocation visitor charges. Consumers receive immutable references only.
    Ok(result.into_iter().collect())
}
