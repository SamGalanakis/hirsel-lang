use super::*;

impl Audit<'_, '_> {
    pub(super) fn inline_layout(&mut self) -> Result<(), QueryError> {
        let item = &self.input.syntax.item;
        let SignatureKind::Type(definition) = &self.input.signature.kind else {
            return Ok(());
        };
        let Some(representation) = definition.representation else {
            return Ok(());
        };
        let root = representation;
        let annotations: Vec<_> = match (&item.kind, self.types.get(root).unwrap().kind.clone()) {
            (
                ast::ItemKind::Type {
                    body: ast::TypeBody::Record(source),
                    ..
                },
                TypeKind::Record(fields),
            ) => source
                .iter()
                .zip(fields)
                .map(|(source, field)| (source.origin.span, field.ty))
                .collect(),
            (
                ast::ItemKind::Type {
                    body: ast::TypeBody::Variants(source),
                    ..
                },
                TypeKind::Variant(variants),
            ) => source
                .iter()
                .zip(variants)
                .flat_map(|(source, variant)| {
                    source
                        .fields
                        .iter()
                        .zip(variant.fields)
                        .map(|((_, syntax), (_, ty))| {
                            (
                                self.input.syntax.tree.types[syntax.0 as usize].origin.span,
                                ty,
                            )
                        })
                })
                .collect(),
            (
                ast::ItemKind::Type {
                    body: ast::TypeBody::Distinct { field, .. },
                    ..
                },
                TypeKind::Record(fields),
            ) => fields
                .first()
                .map(|f| vec![(field.origin.span, f.ty)])
                .unwrap_or_default(),
            _ => vec![(item.origin.span, root)],
        };
        for (span, ty) in annotations {
            let mut active = BTreeSet::from([item.definition.clone()]);
            if self.inline_cycle(ty, &mut active, &mut BTreeSet::new())? {
                self.report(
                    const { RuleId::of("MEM-7") },
                    span,
                    "recursive inline layout requires an explicit block boundary such as box",
                );
            }
        }
        Ok(())
    }

    /// Whether `root` reaches an `active` definition inline. Each named type is
    /// walked once through its generic representation, with its definition
    /// active, and then through the arguments that representation holds inline,
    /// without it: `Option[Option[u64]]` nests an argument, not `Option` itself.
    /// Parameters of the representation being walked that `root` holds inline
    /// are added to `inline`.
    fn inline_cycle(
        &mut self,
        root: TypeId,
        active: &mut BTreeSet<ast::DefPath>,
        inline: &mut BTreeSet<u32>,
    ) -> Result<bool, QueryError> {
        let children: SharedSlice<TypeId> = match self.types.get(root).unwrap().kind.clone() {
            TypeKind::Parameter(BinderRef::Bound {
                depth: 0,
                parameter,
            }) => {
                inline.insert(parameter.0);
                return Ok(false);
            }
            TypeKind::Named {
                definition,
                arguments,
            } => {
                if !active.insert(definition.path().clone()) {
                    return Ok(true);
                }
                let mut held = BTreeSet::new();
                let cycle = match self.generic_representation(&definition)? {
                    Some(representation) => self.inline_cycle(representation, active, &mut held)?,
                    None => false,
                };
                active.remove(definition.path());
                if cycle {
                    return Ok(true);
                }
                held.into_iter()
                    .filter_map(|i| match arguments.get(i as usize) {
                        Some(GenericArgument::Type(t)) => Some(*t),
                        _ => None,
                    })
                    .collect()
            }
            TypeKind::Tuple(fields) => fields,
            TypeKind::Record(fields) => fields.iter().map(|f| f.ty).collect(),
            TypeKind::Variant(variants) => variants
                .iter()
                .flat_map(|v| v.fields.iter().map(|(_, t)| *t))
                .collect(),
            TypeKind::Builtin {
                kind: BuiltinType::Inline | BuiltinType::Simd | BuiltinType::Packed,
                arguments,
            } => {
                if arguments
                    .iter()
                    .any(|a| matches!(a, GenericArgument::Size(Size::Constant(0))))
                {
                    return Ok(false);
                }
                arguments
                    .iter()
                    .filter_map(|a| match a {
                        GenericArgument::Type(t) => Some(*t),
                        _ => None,
                    })
                    .collect()
            }
            // Box, List/Array/Map and callable environments provide block boundaries.
            _ => vec![].into(),
        };
        for child in children {
            if self.inline_cycle(child, active, inline)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// A named type's representation over its own parameters, if it has one.
    fn generic_representation(
        &mut self,
        definition: &hirsel_syntax::DefId,
    ) -> Result<Option<TypeId>, QueryError> {
        let s = self.snapshot(definition)?;
        let SignatureKind::Type(d) = &s.kind else {
            return Ok(None);
        };
        let Some(representation) = d.representation else {
            return Ok(None);
        };
        Ok(Some(representation))
    }
}
