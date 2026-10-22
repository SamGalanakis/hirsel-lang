//! Materialize the declared integer dictionaries.
use super::*;
use declarations::dictionaries::INTEGERS;
use hirsel_tir::{Constructor, Materialized, ValueKind, ValueNode};
impl BuiltinRegistry {
    pub(crate) fn dictionary_value(
        &self,
        t: &TypeInterner,
        m: &TypeMemo,
        path: &DefPath,
    ) -> Result<Option<Materialized>, QueryError> {
        let Some(name) = INTEGERS
            .iter()
            .find(|name| *path == hirsel_syntax::DefPath::core(&format!("{name}.order")))
        else {
            return Ok(None);
        };
        let signature = self.get(DefId::intern(path)).expect("declared dictionary");
        let SignatureKind::Const { ty: root, .. } = &signature.kind else {
            unreachable!()
        };
        let TypeKind::Named {
            definition,
            arguments,
        } = &t.row(*root).kind
        else {
            unreachable!()
        };
        let declaration = self.get(*definition).expect("declared dictionary type");
        let SignatureKind::Type(order) = &declaration.kind else {
            unreachable!()
        };
        let representation = substitute(
            t,
            m,
            order.representation.unwrap(),
            &Substitution {
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
        let TypeKind::Record(fields) = &t.row(representation).kind else {
            unreachable!()
        };
        let [compare] = fields.as_slice() else {
            unreachable!()
        };
        let compare = ValueNode {
            ty: compare.ty,
            kind: ValueKind::FunctionReference {
                definition: hirsel_syntax::DefPath::core(&format!(
                    "$intrinsic.OrderCompare_{name}"
                )),
                arguments: vec![],
            },
        };
        Ok(Some(Materialized {
            root: ValueNode {
                ty: *root,
                kind: ValueKind::Construct {
                    constructor: Constructor::Record(definition.path().clone()),
                    fields: vec![compare],
                },
            },
        }))
    }
}
