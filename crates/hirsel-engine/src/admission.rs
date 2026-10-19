//! Production entry selection and admission, before any evaluator starts.
use hirsel_run_config::{
    AdmittedRun, BoundRun, CodeReach, HostAuthority, LaunchKind, ProgramFacts, ProgramRevision,
    Root, RunConfig, RunRefusal, ValidatedRun,
};
use hirsel_syntax::Visibility;
use hirsel_types::{GenericArgument, Signature, SignatureKind, TypeKind};
use std::path::Path;

/// Read one explicit absolute record. Neither argv nor environment supplies grants.
pub fn read_run_record(path: &Path) -> Result<ValidatedRun, RunRefusal> {
    RunConfig::read_path(path)
}

/// The exact nominal roots, in parameter order, of an eligible entry.
pub fn entry_roots(signature: &Signature, launch: LaunchKind) -> Result<Vec<Root>, RunRefusal> {
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
