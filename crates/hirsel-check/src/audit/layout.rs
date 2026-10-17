use super::*;

impl Audit<'_, '_> {
    pub(super) fn inline_layout(&mut self) -> Result<(), QueryError> {
        let item = &self.input.tree.items[self.input.item.0 as usize];
        let SignatureKind::Type(definition) = &self.input.signature.kind else {
            return Ok(());
        };
        let Some(representation) = definition.representation else {
            return Ok(());
        };
        let root = import_type(&self.input.signature.types, representation, &mut self.types)
            .map_err(|error| {
                QueryError::Operational(OperationalFailure {
                    operation: "inline layout audit".into(),
                    message: format!("{error:?}"),
                })
            })?;
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
                            (self.input.tree.types[syntax.0 as usize].origin.span, ty)
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
            if self.inline_cycle(ty, &mut active)? {
                self.report(
                    const { RuleId::of("MEM-7") },
                    span,
                    "recursive inline layout requires an explicit block boundary such as box",
                );
            }
        }
        Ok(())
    }

    fn inline_cycle(
        &mut self,
        root: TypeId,
        active: &mut BTreeSet<ast::DefPath>,
    ) -> Result<bool, QueryError> {
        let children = match self.types.get(root).unwrap().kind.clone() {
            TypeKind::Named { definition, .. } => {
                if !active.insert(definition.clone()) {
                    return Ok(true);
                }
                let (representation, _) = self.representation(root)?;
                let cycle = representation != root && self.inline_cycle(representation, active)?;
                active.remove(&definition);
                return Ok(cycle);
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
            // Generic parameters are inline but have no nominal recursive edge until
            // instantiated; named arguments are substituted at representation import.
            _ => vec![].into(),
        };
        for child in children {
            if self.inline_cycle(child, active)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
}
