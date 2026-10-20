//! Production entry selection and admission, before any evaluator starts.
use crate::DefinitionQueries;
use hirsel_run_config::{
    AdmittedRun, BoundRun, HostAuthority, ProgramFacts, Root, RunConfig, ValidatedRun,
};
use hirsel_run_schema::{CalleeFacts, CodeReach, LaunchKind, ProgramRevision, RunRefusal};
use hirsel_syntax::Visibility;
use hirsel_types::{GenericArgument, Signature, SignatureKind, TypeKind};
use std::collections::BTreeMap;
use std::path::Path;

/// Read one explicit absolute record. Neither argv nor environment supplies grants.
pub fn read_run_record(path: &Path) -> Result<ValidatedRun, RunRefusal> {
    RunConfig::read_path(path)
}

/// The exact nominal roots, in parameter order, of an eligible entry.
pub fn entry_roots(signature: &Signature, launch: LaunchKind) -> Result<Vec<Root>, RunRefusal> {
    if launch == LaunchKind::Call {
        return callee_roots(signature);
    }
    let invalid = |detail: &str| RunRefusal::Invalid {
        field: "entry".into(),
        detail: detail.into(),
    };
    let SignatureKind::Function(function) = &signature.kind else {
        return Err(invalid("expected a function"));
    };
    if signature.metadata.visibility != Visibility::Public
        || !function.generics.is_empty()
        || function.incomplete
        || function.foreign.is_some()
    {
        return Err(invalid(
            "expected a public, complete, monomorphic Hirsel function",
        ));
    }
    let TypeKind::Named {
        definition,
        arguments,
    } = &signature
        .types
        .get(function.result)
        .ok_or_else(|| invalid("missing result type"))?
        .kind
    else {
        return Err(invalid("expected core@1/Result[(), E]"));
    };
    let nominal = |definition: &hirsel_syntax::DefPath, name: &str| {
        definition.package_is("core", "1")
            && definition.0.modules.is_empty()
            && definition.0.name == name
    };
    let [GenericArgument::Type(ok), GenericArgument::Type(_)] = arguments.as_ref() else {
        return Err(invalid("expected core@1/Result[(), E]"));
    };
    if !nominal(definition, "Result")
        || !matches!(signature.types.get(*ok).map(|t| &t.kind), Some(TypeKind::Tuple(fields)) if fields.is_empty())
    {
        return Err(invalid("expected core@1/Result[(), E]"));
    }
    let roots = function
        .parameters
        .iter()
        .map(|parameter| {
            let TypeKind::Named {
                definition,
                arguments,
            } = &signature
                .types
                .get(parameter.ty)
                .ok_or_else(|| invalid("missing parameter type"))?
                .kind
            else {
                return Err(invalid("expected nominal root capabilities"));
            };
            if !arguments.is_empty() || !nominal(definition, &definition.0.name) {
                return Err(invalid("expected nominal root capabilities"));
            }
            match definition.0.name.as_str() {
                "Io" => Ok(Root::Io),
                "Args" => Ok(Root::Args),
                "Env" => Ok(Root::Env),
                "Unsafe" => Ok(Root::Unsafe),
                "Test" => Ok(Root::Test),
                _ => Err(invalid("expected nominal root capabilities")),
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    if (launch == LaunchKind::Test && roots != [Root::Test])
        || (launch == LaunchKind::Main && roots.contains(&Root::Test))
    {
        return Err(invalid("parameters do not match the launch kind"));
    }
    Ok(roots)
}

/// The roots of a callee run's entry (RUN-3, RUN-10): a complete, monomorphic Hirsel
/// function of any visibility taking its argument, after the place's `Io` for the call
/// form, and returning a `Result`.
fn callee_roots(signature: &Signature) -> Result<Vec<Root>, RunRefusal> {
    let invalid = || RunRefusal::Invalid {
        field: "entry".into(),
        detail: "expected a runner entry: fn(a: A) or fn(rt: Io, a: A), returning a Result".into(),
    };
    let SignatureKind::Function(function) = &signature.kind else {
        return Err(invalid());
    };
    if !function.generics.is_empty() || function.incomplete || function.foreign.is_some() {
        return Err(invalid());
    }
    let named = |ty, name: &str| {
        matches!(
            signature.types.get(ty).map(|t| &t.kind),
            Some(TypeKind::Named { definition, .. })
                if definition.package_is("core", "1")
                    && definition.0.modules.is_empty()
                    && definition.0.name == name
        )
    };
    if !named(function.result, "Result") {
        return Err(invalid());
    }
    match &function.parameters[..] {
        [_] => Ok(vec![]),
        [io, _] if named(io.ty, "Io") => Ok(vec![Root::Io]),
        _ => Err(invalid()),
    }
}

/// What a runner call carries for each callee entry `entry`'s closure names (RUN-8,
/// RUN-11): every function reference to a definition a callee run can enter, with its
/// own program revision, definition hash, signature fingerprint and reach (CAP-6).
pub fn callee_facts(
    engine: &mut crate::QueryEngine,
    entry: &hirsel_syntax::DefPath,
) -> Result<BTreeMap<String, CalleeFacts>, hirsel_types::QueryError> {
    let hashes = engine.definition_hashes([entry.clone()])?;
    let mut facts = BTreeMap::new();
    for site in hashes.call_sites.values().flatten() {
        let path = site.callee.to_string();
        if !matches!(site.kind, hirsel_proof_export::CallSiteKind::Reference(_))
            || facts.contains_key(&path)
        {
            continue;
        }
        let signature = engine.item_signature(&site.callee)?.value;
        if callee_roots(&signature).is_err() {
            continue;
        }
        let program = engine.semantic_program_revision(&site.callee)?.digest();
        let reach = match engine.unsafe_reach(&site.callee)? {
            None => CodeReach::Safe,
            Some(reach) => CodeReach::Escapes(reach.spelled()),
        };
        facts.insert(
            path,
            CalleeFacts {
                program,
                code: site.hash.0,
                fingerprint: signature.fingerprint.0,
                reach,
            },
        );
    }
    Ok(facts)
}

/// The facts a runner host checks an arriving call against (RUN-8) for one definition it
/// holds, `path`, and the call form its signature takes: none when it is not a callee entry.
pub fn entry_facts(
    engine: &mut crate::QueryEngine,
    path: &hirsel_syntax::DefPath,
) -> Result<Option<(CalleeFacts, hirsel_run_schema::CallForm)>, hirsel_types::QueryError> {
    let signature = engine.item_signature(path)?.value;
    let Ok(roots) = callee_roots(&signature) else {
        return Ok(None);
    };
    let hashes = engine.definition_hashes([path.clone()])?;
    let Some(code) = hashes.hashes.get(path) else {
        return Ok(None);
    };
    let program = engine.semantic_program_revision(path)?.digest();
    let reach = match engine.unsafe_reach(path)? {
        None => CodeReach::Safe,
        Some(reach) => CodeReach::Escapes(reach.spelled()),
    };
    let form = if roots.is_empty() {
        hirsel_run_schema::CallForm::Compute
    } else {
        hirsel_run_schema::CallForm::Call
    };
    Ok(Some((
        CalleeFacts {
            program,
            code: code.0,
            fingerprint: signature.fingerprint.0,
            reach,
        },
        form,
    )))
}

/// The callee entries a runner host serves from its program: every callee entry the
/// package of `entry` defines, with the facts an arriving call is checked against.
pub fn served_entries(
    engine: &mut crate::QueryEngine,
    entry: &hirsel_syntax::DefPath,
) -> Result<BTreeMap<String, (CalleeFacts, hirsel_run_schema::CallForm)>, hirsel_types::QueryError>
{
    let mut served = BTreeMap::new();
    for path in engine.item_paths() {
        if path.0.package != entry.0.package {
            continue;
        }
        if let Some(facts) = entry_facts(engine, &path)? {
            served.insert(path.to_string(), facts);
        }
    }
    Ok(served)
}

/// Select the admission kind from the entry's checked nominal signature.
pub fn entry_launch_kind(signature: &Signature) -> Result<LaunchKind, RunRefusal> {
    if entry_roots(signature, LaunchKind::Test).is_ok() {
        Ok(LaunchKind::Test)
    } else {
        entry_roots(signature, LaunchKind::Main).map(|_| LaunchKind::Main)
    }
}

/// Code selection defaults to denial. Check every binding and root before host admission.
pub fn admit_run(
    signature: &Signature,
    revision: &ProgramRevision,
    launch: LaunchKind,
    requested: Option<&ValidatedRun>,
    code: CodeReach,
    host: &mut dyn HostAuthority,
    now: i64,
) -> Result<AdmittedRun, RunRefusal> {
    bind_run(signature, revision, launch, requested, code)?.admit(host, now)
}

/// Bind the requested run (or the program's default record) to the checked code it would
/// execute, with no host consulted: the entry's signature, binding, roots and code policy. A
/// launcher that admits the run elsewhere (a native executable) binds here only.
pub fn bind_run(
    signature: &Signature,
    revision: &ProgramRevision,
    launch: LaunchKind,
    requested: Option<&ValidatedRun>,
    code: CodeReach,
) -> Result<BoundRun, RunRefusal> {
    let roots = entry_roots(signature, launch)?;
    if signature.definition.to_string() != revision.entry() {
        return Err(RunRefusal::Binding {
            field: "entry.signature".into(),
        });
    }
    let run = match requested {
        Some(run) => run.clone(),
        None => RunConfig::for_program(revision, launch).validated()?,
    };
    run.bind(&ProgramFacts::of(revision, launch, roots, code))
}
