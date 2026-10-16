//! Explicit named integer Order values, constructed from their declared graphs.
use super::*;
use hirsel_tir::Constructor;
use hirsel_tir::{Materialized, ValueKind, ValueNode};

pub(super) const INTEGERS: &[&str] = &[
    "u8", "u16", "u32", "u64", "u128", "i8", "i16", "i32", "i64", "i128",
];
pub(super) fn signatures() -> Vec<Signature> {
    INTEGERS
        .iter()
        .map(|name| {
            let mut builder = build::Builder::new("");
            let function = builder.function(&format!("a:{name},b:{name}"), "i32 uses {}", true);
            let mut signature = builder.signature(
                &format!("$intrinsic.OrderCompare_{name}"),
                SignatureKind::Function(function),
                false,
            );
            signature.metadata.visibility = hirsel_syntax::Visibility::Private;
            signature
        })
        .collect()
}
impl BuiltinRegistry {
    pub(crate) fn dictionary_value(
        &self,
        path: &DefPath,
    ) -> Result<Option<Materialized>, QueryError> {
        let Some(name) = INTEGERS
            .iter()
            .find(|name| *path == core_path(&format!("{name}.order")))
        else {
            return Ok(None);
        };
        let signature = self.get(path).expect("declared dictionary");
        let SignatureKind::Const(root) = &signature.kind else {
            unreachable!()
        };
        let TypeKind::Named {
            definition,
            arguments,
        } = &signature.types.get(*root).unwrap().kind
        else {
            unreachable!()
        };
        let declaration = self.get(definition).expect("declared dictionary type");
        let SignatureKind::Type(order) = &declaration.kind else {
            unreachable!()
        };
        let mut types = signature.types.clone();
        let representation = substitute_type(
            &declaration.types,
            order.representation.unwrap(),
            &mut types,
            Substitution {
                binder: Some(hirsel_types::GenericBinder::Bound(0)),
                roles: &declaration.metadata.roles,
                arguments,
                authorities: &vec![AuthoritySummary::inert(); arguments.len()],
            },
        )
        .map_err(|e| {
            QueryError::Operational(OperationalFailure {
                operation: "builtin dictionary materialization".into(),
                message: format!("{e:?}"),
            })
        })?;
        let TypeKind::Record(fields) = &types.get(representation).unwrap().kind else {
            unreachable!()
        };
        let [compare] = fields.as_slice() else {
            unreachable!()
        };
        let compare = ValueNode {
            ty: compare.ty,
            kind: ValueKind::FunctionReference(core_path(&format!(
                "$intrinsic.OrderCompare_{name}"
            ))),
        };
        Ok(Some(Materialized {
            root: ValueNode {
                ty: *root,
                kind: ValueKind::Construct {
                    constructor: Constructor::Record(definition.clone()),
                    fields: vec![compare],
                },
            },
            types,
        }))
    }
}
