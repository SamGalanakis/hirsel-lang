//! CT-5 instance bodies. A generic generated declaration has no definition of
//! its own; each selected instance is a monomorphic definition whose header is
//! the declaration's header at the instance's arguments, recomputed here from
//! the declaration's signature rather than trusted from the producer.
use super::*;
use hirsel_types::{
    AuthoritySummary, CanonicalSpelling, GenericBinder, GenericParameter, SignatureKind,
    SignatureRef, Substitution, TypeStore,
};

/// An instance table that does not describe the program's definitions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InstanceViolation {
    /// A declaration in the table is not a generic function, or has a body.
    Declaration(DefPath),
    /// A row's arguments do not fit the declaration's generics, name a
    /// parameter, or are not the arguments its key spells.
    Arguments {
        declaration: DefPath,
        key: Vec<String>,
    },
    /// A row names no monomorphic definition with a signature.
    MissingInstance {
        declaration: DefPath,
        key: Vec<String>,
    },
    /// A row's definition header is not the declaration's at its arguments.
    Header {
        declaration: DefPath,
        key: Vec<String>,
    },
    /// A call or reference of a declaration at closed arguments has no row.
    Unselected {
        owner: DefPath,
        declaration: DefPath,
    },
}

pub fn validate_instance_bodies(
    bodies: &InstanceBodies,
    signatures: &BTreeMap<DefPath, SignatureRef>,
    definitions: &BTreeMap<DefPath, DefinitionRef>,
) -> Vec<InstanceViolation> {
    let mut violations = vec![];
    for declaration in &bodies.declarations {
        let generic = signatures.get(declaration).is_some_and(
            |s| matches!(&s.kind, SignatureKind::Function(f) if !f.generics.is_empty()),
        );
        if !generic || definitions.contains_key(declaration) {
            violations.push(InstanceViolation::Declaration(declaration.clone()));
        }
    }
    for (declaration, rows) in &bodies.instances {
        let Some(SignatureKind::Function(header)) = signatures
            .get(declaration)
            .filter(|_| bodies.declarations.contains(declaration))
            .map(|s| &s.kind)
        else {
            violations.push(InstanceViolation::Declaration(declaration.clone()));
            continue;
        };
        let declared = &signatures[declaration].types;
        for (key, row) in rows {
            let fits = row.arguments.len() == header.generics.len()
                && row
                    .arguments
                    .iter()
                    .zip(header.generics.iter())
                    .all(|(a, g)| {
                        matches!(
                            (a, g),
                            (GenericArgument::Type(_), GenericParameter::Type { .. })
                                | (GenericArgument::Size(_), GenericParameter::Size { .. })
                        )
                    })
                && InstanceBodies::key(&row.types, &row.arguments).as_ref() == Some(key);
            if !fits {
                violations.push(InstanceViolation::Arguments {
                    declaration: declaration.clone(),
                    key: key.clone(),
                });
                continue;
            }
            let instance = signatures
                .get(&row.definition)
                .filter(|_| definitions.contains_key(&row.definition));
            let Some(instance) = instance else {
                violations.push(InstanceViolation::MissingInstance {
                    declaration: declaration.clone(),
                    key: key.clone(),
                });
                continue;
            };
            if !same_header(declared, header, &row.types, &row.arguments, instance) {
                violations.push(InstanceViolation::Header {
                    declaration: declaration.clone(),
                    key: key.clone(),
                });
            }
        }
    }
    for (owner, definition) in definitions {
        let mut uses = vec![];
        for body in &definition.bodies {
            for block in &body.blocks {
                if let Terminator::Call {
                    callee: Callee::Definition { path, arguments },
                    ..
                } = &block.terminator
                {
                    uses.push((path, arguments));
                }
            }
            for rvalue in &body.rvalues {
                if let Rvalue::FunctionReference {
                    definition,
                    arguments,
                } = rvalue
                {
                    uses.push((definition, arguments));
                }
            }
        }
        for (path, arguments) in uses {
            if bodies.declarations.contains(path)
                && InstanceBodies::key(&definition.types, arguments).is_some()
                && !matches!(
                    bodies.resolve(path, &definition.types, arguments),
                    Ok(Some(_))
                )
            {
                violations.push(InstanceViolation::Unselected {
                    owner: owner.clone(),
                    declaration: path.clone(),
                });
            }
        }
    }
    violations
}

/// Whether `instance` is a monomorphic function whose passing modes, parameter
/// and result types (by canonical spelling, CT-3, and root uniqueness) and
/// `uses` are `header`'s with its generics replaced by `arguments`, rows of
/// `types`.
fn same_header(
    declared: &TypeStore,
    header: &hirsel_types::FunctionType,
    types: &TypeStore,
    arguments: &[GenericArgument],
    instance: &Signature,
) -> bool {
    let SignatureKind::Function(f) = &instance.kind else {
        return false;
    };
    if !f.generics.is_empty()
        || f.parameters.len() != header.parameters.len()
        || f.uses != header.uses
    {
        return false;
    }
    let mut scratch = TypeStore::default();
    let Ok(imported) = arguments
        .iter()
        .map(|a| match a {
            GenericArgument::Type(t) => {
                hirsel_types::import_type(types, *t, &mut scratch).map(GenericArgument::Type)
            }
            size => Ok(size.clone()),
        })
        .collect::<Result<Vec<_>, _>>()
    else {
        return false;
    };
    let authorities = vec![AuthoritySummary::Unknown; imported.len()];
    let mut instantiated = |ty| {
        hirsel_types::substitute_type(
            declared,
            ty,
            &mut scratch,
            Substitution {
                binder: Some(GenericBinder::Bound(0)),
                arguments: &imported,
                authorities: &authorities,
                ..Default::default()
            },
        )
    };
    let mut expected = vec![];
    for p in header.parameters.iter() {
        let Ok(ty) = instantiated(p.ty) else {
            return false;
        };
        expected.push((p.passing, ty));
    }
    let Ok(result) = instantiated(header.result) else {
        return false;
    };
    // Relation, multiplicity and sendability of a written type default by its
    // position and content (MODE-7), so the instance's rows make the defaults
    // of the closed type; identity and written uniqueness must agree.
    let same = |a: TypeId, b: TypeId| {
        CanonicalSpelling::closed(&scratch, a) == CanonicalSpelling::closed(&instance.types, b)
            && scratch.get(a).map(|r| r.modes.uniqueness)
                == instance.types.get(b).map(|r| r.modes.uniqueness)
    };
    expected
        .iter()
        .zip(f.parameters.iter())
        .all(|((passing, ty), p)| *passing == p.passing && same(*ty, p.ty))
        && same(result, f.result)
}
