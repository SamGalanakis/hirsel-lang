//! Match actual descriptor completion positions to static declaration paths.
//! The existing rt_program derivation proves ordered field contributions; this
//! diagnostic projection never interprets payload bytes or changes that program.
use super::{NativeProjection, Selection};
use crate::layout_types::{FieldLayout, LayoutId, LayoutKind, LayoutTable, TagForm};
use crate::rt_program::{self, Descriptor, StepKind};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CleanupCompletion {
    pub step: u32,
    pub layout: LayoutId,
    pub path: Vec<NativeProjection>,
}

/// Resolve each inline Resource's completed finalizer against the independently
/// derived descriptor. Heap, dynamic and ambiguous ownership refuse. A runtime
/// completion can select a checked step; it cannot supply a declaration path.
pub fn resource_cleanup_program(
    layouts: &LayoutTable,
    root: LayoutId,
) -> Result<Vec<CleanupCompletion>, String> {
    let mut programs = Programs {
        layouts,
        active: BTreeSet::new(),
        complete: BTreeMap::new(),
    };
    programs.get(root).map(|program| program.1.clone())
}

type Program = (Descriptor, Vec<CleanupCompletion>);
struct Programs<'a> {
    layouts: &'a LayoutTable,
    active: BTreeSet<LayoutId>,
    complete: BTreeMap<LayoutId, Arc<Program>>,
}
impl Programs<'_> {
    fn get(&mut self, layout: LayoutId) -> Result<Arc<Program>, String> {
        if let Some(program) = self.complete.get(&layout) {
            return Ok(program.clone());
        }
        if !self.active.insert(layout) {
            return Err("ownership cleanup has recursive inline storage".into());
        }
        let program = Arc::new(self.derive(layout)?);
        self.active.remove(&layout);
        self.complete.insert(layout, program.clone());
        Ok(program)
    }

    fn derive(&mut self, layout: LayoutId) -> Result<Program, String> {
        let layouts = self.layouts;
        let record = layouts
            .layout(layout)
            .ok_or("ownership cleanup layout is absent")?;
        let descriptor = rt_program::descriptor(self.layouts, layout)
            .map_err(|error| format!("ownership cleanup descriptor: {error:?}"))?;
        descriptor
            .check_finalizers(self.layouts)
            .map_err(|error| format!("ownership cleanup finalizers: {error:?}"))?;
        if !Selection::Resources.tracks_layout(self.layouts, layout) {
            return Ok((descriptor, Vec::new()));
        }
        if !matches!(
            record.kind,
            LayoutKind::Resource
                | LayoutKind::Aggregate
                | LayoutKind::Variant
                | LayoutKind::Environment { .. }
        ) {
            return Err("ownership cleanup requires static inline Resource storage".into());
        }
        if matches!(record.kind, LayoutKind::Resource) && record.finalizer.is_none() {
            return Err("ownership cleanup Resource has no finalizer completion".into());
        }
        let mut seen = BTreeSet::new();
        let mut completions = Vec::new();
        for (pc, step) in descriptor.steps.iter().enumerate() {
            if step.kind == StepKind::EndFinalize
                && step.field == crate::runtime_abi::WHOLE_VALUE
                && step.layout == Some(layout)
                && step.offset == 0
            {
                seen.insert(pc);
                if matches!(record.kind, LayoutKind::Resource) {
                    completions.push(CleanupCompletion {
                        step: u32::try_from(pc).map_err(|_| "ownership cleanup step overflow")?,
                        layout,
                        path: Vec::new(),
                    });
                }
            }
        }
        if let Some(tag) = &record.tag {
            let mut variants = BTreeSet::new();
            let mut pc = 0;
            while pc < descriptor.steps.len() {
                let step = &descriptor.steps[pc];
                if seen.contains(&pc) {
                    pc += 1;
                    continue;
                }
                let ordinal = match (step.kind, tag.form) {
                    (StepKind::Tag, TagForm::Ordinal) => step.variant,
                    (StepKind::NonNull, TagForm::Niche { dataful, .. }) => dataful,
                    (StepKind::LiveBit, TagForm::LiveBit { bit }) if bit == step.variant => 1,
                    _ => return Err("ownership cleanup has no exact root variant scope".into()),
                };
                if !variants.insert(ordinal) {
                    return Err("ownership cleanup repeats a root variant scope".into());
                }
                if step.offset != tag.offset {
                    return Err("ownership cleanup root tag differs".into());
                }
                let end = pc
                    .checked_add(1 + step.span as usize)
                    .filter(|end| *end <= descriptor.steps.len())
                    .ok_or("ownership cleanup variant span leaves its descriptor")?;
                let variant = tag
                    .variants
                    .iter()
                    .find(|variant| variant.ordinal == ordinal)
                    .ok_or("ownership cleanup variant is absent")?;
                for field in &variant.fields {
                    self.field(
                        &descriptor,
                        pc + 1..end,
                        field,
                        NativeProjection::VariantField {
                            variant: ordinal,
                            field: field.ordinal,
                        },
                        &mut seen,
                        &mut completions,
                    )?;
                }
                pc = end;
            }
            if tag.variants.iter().any(|variant| {
                !variants.contains(&variant.ordinal)
                    && variant
                        .fields
                        .iter()
                        .any(|field| Selection::Resources.tracks_layout(layouts, field.layout))
            }) {
                return Err("ownership cleanup omits a Resource variant scope".into());
            }
        } else {
            for field in &record.fields {
                self.field(
                    &descriptor,
                    0..descriptor.steps.len(),
                    field,
                    NativeProjection::Field(field.ordinal),
                    &mut seen,
                    &mut completions,
                )?;
            }
        }
        if descriptor
            .steps
            .iter()
            .enumerate()
            .any(|(pc, step)| step.kind == StepKind::EndFinalize && !seen.contains(&pc))
        {
            return Err("ownership cleanup leaves an unbound finalizer completion".into());
        }
        completions.sort_by_key(|completion| completion.step);
        Ok((descriptor, completions))
    }

    fn field(
        &mut self,
        parent: &Descriptor,
        range: std::ops::Range<usize>,
        field: &FieldLayout,
        projection: NativeProjection,
        seen: &mut BTreeSet<usize>,
        completions: &mut Vec<CleanupCompletion>,
    ) -> Result<(), String> {
        let actual = range
            .filter(|pc| {
                let step = &parent.steps[*pc];
                step.kind == StepKind::EndFinalize && step.field == field.ordinal
            })
            .collect::<Vec<_>>();
        if actual.is_empty() {
            if Selection::Resources.tracks_layout(self.layouts, field.layout) {
                return Err("ownership cleanup Resource field has no inline completions".into());
            }
            return Ok(());
        }
        let program = self.get(field.layout)?;
        let (child, bindings) = &*program;
        let expected = child
            .steps
            .iter()
            .enumerate()
            .filter(|(_, step)| step.kind == StepKind::EndFinalize)
            .collect::<Vec<_>>();
        if actual.len() != expected.len() {
            return Err("ownership cleanup field completion count differs".into());
        }
        for (pc, (child_pc, step)) in actual.into_iter().zip(expected) {
            let physical = &parent.steps[pc];
            if physical.layout != step.layout
                || physical.offset
                    != field
                        .offset
                        .checked_add(step.offset)
                        .ok_or("ownership cleanup offset overflow")?
                || !seen.insert(pc)
            {
                return Err("ownership cleanup field completion differs".into());
            }
            if let Some(binding) = bindings
                .iter()
                .find(|binding| binding.step as usize == child_pc)
            {
                let mut path = vec![projection];
                path.extend_from_slice(&binding.path);
                completions.push(CleanupCompletion {
                    step: u32::try_from(pc).map_err(|_| "ownership cleanup step overflow")?,
                    layout: binding.layout,
                    path,
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout_types::*;

    fn record(kind: LayoutKind, size: u64, drop: Vec<DropStep>) -> LayoutRecord {
        LayoutRecord {
            key: LayoutIdentity {
                owner_revision: None,
                instance: None,
                target: TargetSpec::x86_64_unknown_linux_gnu().digest(),
                layout_version: crate::LAYOUT_VERSION,
            },
            kind,
            size: SizeSpec::Fixed(size),
            align: 8,
            fields: Vec::new(),
            tag: None,
            initialized: InitializedBytes::Fields,
            scan: Vec::new(),
            drop,
            copy: CopyRecipe::Bits,
            finalizer: None,
            storage: Vec::new(),
            abi: AbiInputs {
                scalars: Vec::new(),
                unaligned: false,
            },
        }
    }

    fn table() -> LayoutTable {
        // Reuse rt_program's zero_sized_siblings vector. The physical offset
        // and type cannot distinguish these two declaration-owned Resources.
        let mut child = record(LayoutKind::Resource, 0, vec![DropStep::Finalize]);
        child.finalizer = Some(FinalizerRef {
            definition: hirsel_syntax::DefPath::core("Token.drop"),
            body: 0,
            unsafe_ffi_package: false,
        });
        let mut pair = record(
            LayoutKind::Aggregate,
            0,
            vec![
                DropStep::Child {
                    offset: 0,
                    layout: LayoutId(0),
                },
                DropStep::Child {
                    offset: 0,
                    layout: LayoutId(0),
                },
            ],
        );
        pair.fields = (0..2)
            .map(|ordinal| FieldLayout {
                ordinal,
                offset: 0,
                layout: LayoutId(0),
                unaligned: false,
            })
            .collect();
        let mut table = LayoutTable::new(&TargetSpec::x86_64_unknown_linux_gnu());
        table.layouts = vec![child, pair];
        table
    }

    fn optional_pair(table: &mut LayoutTable) -> LayoutId {
        let mut option = record(
            LayoutKind::Variant,
            8,
            vec![DropStep::ActiveVariant {
                tag_offset: 0,
                form: TagForm::Ordinal,
                variants: vec![
                    (0, vec![]),
                    (
                        1,
                        vec![
                            DropStep::Child {
                                offset: 8,
                                layout: LayoutId(0),
                            },
                            DropStep::Child {
                                offset: 8,
                                layout: LayoutId(0),
                            },
                        ],
                    ),
                ],
            }],
        );
        option.tag = Some(TagEncoding {
            offset: 0,
            form: TagForm::Ordinal,
            variants: vec![
                VariantLayout {
                    ordinal: 0,
                    fields: vec![],
                },
                VariantLayout {
                    ordinal: 1,
                    fields: vec![FieldLayout {
                        ordinal: 0,
                        offset: 8,
                        layout: LayoutId(1),
                        unaligned: false,
                    }],
                },
            ],
        });
        let id = LayoutId(table.layouts.len() as u32);
        table.layouts.push(option);
        id
    }

    #[test]
    fn zero_sized_equal_resource_fields_bind_distinct_actual_completion_steps() {
        let table = table();
        let program = resource_cleanup_program(&table, LayoutId(1)).unwrap();
        assert_eq!(
            program,
            [
                CleanupCompletion {
                    step: 1,
                    layout: LayoutId(0),
                    path: vec![NativeProjection::Field(1)],
                },
                CleanupCompletion {
                    step: 3,
                    layout: LayoutId(0),
                    path: vec![NativeProjection::Field(0)],
                },
            ]
        );
        let descriptor = rt_program::descriptor(&table, LayoutId(1)).unwrap();
        assert!(program.iter().all(|binding| {
            let step = &descriptor.steps[binding.step as usize];
            step.kind == StepKind::EndFinalize && step.offset == 0
        }));
    }

    #[test]
    fn nested_variant_paths_survive_flattened_equal_resource_offsets() {
        let mut table = table();
        let root = optional_pair(&mut table);
        let program = resource_cleanup_program(&table, root).unwrap();
        assert_eq!(
            program,
            [
                CleanupCompletion {
                    step: 3,
                    layout: LayoutId(0),
                    path: vec![
                        NativeProjection::VariantField {
                            variant: 1,
                            field: 0
                        },
                        NativeProjection::Field(1),
                    ],
                },
                CleanupCompletion {
                    step: 5,
                    layout: LayoutId(0),
                    path: vec![
                        NativeProjection::VariantField {
                            variant: 1,
                            field: 0
                        },
                        NativeProjection::Field(0),
                    ],
                },
            ]
        );
    }

    #[test]
    fn a_balanced_empty_descriptor_cannot_hide_a_declared_resource_variant() {
        let mut table = table();
        let root = optional_pair(&mut table);
        table.layouts[root.0 as usize].drop.clear();
        rt_program::descriptor(&table, root)
            .unwrap()
            .check_finalizers(&table)
            .unwrap();
        assert!(resource_cleanup_program(&table, root).is_err());
    }

    #[test]
    fn heap_resource_storage_and_missing_finalizers_have_no_inline_certificate() {
        let mut table = table();
        let root = LayoutId(table.layouts.len() as u32);
        table.layouts.push(record(
            LayoutKind::Block {
                payload: LayoutId(0),
            },
            8,
            vec![DropStep::Release { offset: 0 }],
        ));
        rt_program::descriptor(&table, root).unwrap();
        assert!(resource_cleanup_program(&table, root).is_err());
        table.layouts[0].finalizer = None;
        table.layouts[0].drop.clear();
        rt_program::descriptor(&table, LayoutId(0)).unwrap();
        assert!(resource_cleanup_program(&table, LayoutId(0)).is_err());
    }
}
