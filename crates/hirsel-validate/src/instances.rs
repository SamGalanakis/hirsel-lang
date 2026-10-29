//! CT-5 instance bodies. A generic generated declaration has no definition of
//! its own; each selected instance is a monomorphic definition whose header is
//! the declaration's header at the instance's arguments, recomputed here from
//! the declaration's signature rather than trusted from the producer.
use super::*;
use hirsel_types::{
    AuthoritySummary, CanonicalSpelling, GenericBinder, GenericParameter, SignatureKind,
    SignatureRef, Substitution, TypeMemo,
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
    t: &TypeInterner,
    bodies: &InstanceBodies,
    signatures: &BTreeMap<DefPath, SignatureRef>,
    definitions: &BTreeMap<DefPath, DefinitionRef>,
) -> Vec<InstanceViolation> {
    let definitions_set = definitions.keys().cloned().collect();
    let mut violations = validate_instance_catalog(t, bodies, signatures, &definitions_set);
    for (owner, definition) in definitions {
        violations.extend(validate_instance_uses(t, bodies, owner, definition));
    }
    violations
}

/// Validate selected CT-5 rows using compact independently observed executable
/// declarations. Body uses are validated separately when that body is demanded.
pub fn validate_instance_catalog(
    t: &TypeInterner,
    bodies: &InstanceBodies,
    signatures: &BTreeMap<DefPath, SignatureRef>,
    definitions: &std::collections::BTreeSet<DefPath>,
) -> Vec<InstanceViolation> {
    // V's own structural memo, as for a definition.
    let memo = TypeMemo::new();
    let mut violations = vec![];
    for declaration in &bodies.declarations {
        let path = declaration.path();
        let generic = signatures.get(path).is_some_and(
            |s| matches!(&s.kind, SignatureKind::Function(f) if !f.generics.is_empty()),
        );
        if !generic || definitions.contains(path) {
            violations.push(InstanceViolation::Declaration(path.clone()));
        }
    }
    for (declaration, rows) in &bodies.instances {
        let path = declaration.path();
        let Some(SignatureKind::Function(header)) = signatures
            .get(path)
            .filter(|_| bodies.declarations.contains(declaration))
            .map(|s| &s.kind)
        else {
            violations.push(InstanceViolation::Declaration(path.clone()));
            continue;
        };
        // Rows are reported in the order of their keys' spellings.
        let mut rows: Vec<_> = rows
            .iter()
            .map(|(key, row)| (spelled(*key), key, row))
            .collect();
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        for (spelled, key, row) in rows {
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
                && InstanceBodies::key(t, &row.arguments).as_ref() == Some(key);
            if !fits {
                violations.push(InstanceViolation::Arguments {
                    declaration: path.clone(),
                    key: spelled,
                });
                continue;
            }
            let instance = signatures
                .get(row.definition.path())
                .filter(|_| definitions.contains(row.definition.path()));
            let Some(instance) = instance else {
                violations.push(InstanceViolation::MissingInstance {
                    declaration: path.clone(),
                    key: spelled,
                });
                continue;
            };
            if !same_header(t, &memo, header, &row.arguments, instance) {
                violations.push(InstanceViolation::Header {
                    declaration: path.clone(),
                    key: spelled,
                });
            }
        }
    }
    violations
}

/// The exact existing CT-5 closed-call/reference check for one actual source
/// body. No other Definition is needed or retained by this operation.
pub fn validate_instance_uses(
    t: &TypeInterner,
    bodies: &InstanceBodies,
    owner: &DefPath,
    definition: &hirsel_tir::Definition,
) -> Vec<InstanceViolation> {
    instance_uses(t, bodies, owner, &ReadDefinition::live(definition))
}

/// The same CT-5 obligations over live or checked indexed rows. Missing source
/// or type bindings are a read failure, independent of the call verdict.
pub fn validate_instance_rows(
    t: &TypeInterner,
    bodies: &InstanceBodies,
    owner: &DefPath,
    definition: hirsel_product_codec::TirView<'_>,
) -> Option<Vec<InstanceViolation>> {
    Some(instance_uses(
        t,
        bodies,
        owner,
        &ReadDefinition::new(definition)?,
    ))
}
fn instance_uses(
    t: &TypeInterner,
    bodies: &InstanceBodies,
    owner: &DefPath,
    definition: &ReadDefinition<'_>,
) -> Vec<InstanceViolation> {
    let mut violations = vec![];
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
        let Some(declaration) = DefId::get(path) else {
            continue;
        };
        if bodies.declarations.contains(&declaration)
            && InstanceBodies::key(t, arguments).is_some()
            && !matches!(bodies.resolve(declaration, t, arguments), Ok(Some(_)))
        {
            violations.push(InstanceViolation::Unselected {
                owner: owner.clone(),
                declaration: path.clone(),
            });
        }
    }
    violations
}

fn spelled(key: hirsel_types::GeneratedActualKey) -> Vec<String> {
    key.spellings().into_iter().map(str::to_owned).collect()
}

/// Whether `instance` is a monomorphic function whose passing modes, parameter
/// and result types (by canonical spelling, CT-3, and root uniqueness) and
/// `uses` are `header`'s with its generics replaced by `arguments`.
fn same_header(
    t: &TypeInterner,
    memo: &TypeMemo,
    header: &hirsel_types::FunctionType,
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
    let authorities = vec![AuthoritySummary::Unknown; arguments.len()];
    let instantiated = |ty| {
        hirsel_types::substitute(
            t,
            memo,
            ty,
            &Substitution {
                binder: Some(GenericBinder::Bound(0)),
                arguments,
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
        CanonicalSpelling::closed(t, a) == CanonicalSpelling::closed(t, b)
            && t.get(a).map(|r| r.modes.uniqueness) == t.get(b).map(|r| r.modes.uniqueness)
    };
    expected
        .iter()
        .zip(f.parameters.iter())
        .all(|((passing, ty), p)| *passing == p.passing && same(*ty, p.ty))
        && same(result, f.result)
}
