//! Retained allocation accounting, adapted from Buck2 allocative's Arc and Vec
//! visitors (third-party/NOTICES.md). This measures storage, never wire bytes.
mod signature;
mod source_entry;
pub(crate) use signature::retained_signature_payload_bytes;
pub(crate) use source_entry::retained_signature_entry_bytes;
mod syntax;
#[cfg(test)]
mod type_storage;
use super::{DepthEvidence, ReadKey, Reads};
use crate::comptime::StagedRegistration;
use hirsel_syntax::{ContentHash, DefPath};
use hirsel_tir::*;
use hirsel_types::{Destination, Origin, OriginRoot, SharedPath, SharedSlice};
use std::{alloc::Layout, collections::HashSet};
pub(crate) use syntax::retained_syntax_bytes;

/// Bytes retained by a DefinitionRef, including its Arc allocation. Session
/// interner rows/DefIds and shared syntax belong to the session, not this hold.
/// Shared allocations are charged once within this artifact, conservatively
/// again if another hold also retains them. No allocator-specific size classes.
pub(crate) fn retained_tir_bytes(definition: &Definition) -> u64 {
    tir_bytes(definition, Weight::default())
}

/// Archive decoding recreates shared rows at every serialized occurrence.
/// Count each occurrence independently instead of assuming source Arc sharing.
pub(crate) fn decoded_tir_bytes(definition: &Definition) -> u64 {
    tir_bytes(
        definition,
        Weight {
            count_shared_repeats: true,
            ..Weight::default()
        },
    )
}
fn tir_bytes(definition: &Definition, mut weight: Weight) -> u64 {
    weight.add(arc_bytes(definition));
    let Definition {
        caller_bodies,
        protocol_associations,
        types,
        path: _,
        signature_hash: _,
        bodies,
        entry: _,
        contracts,
    } = definition;
    weight.flat_vec(caller_bodies);
    weight.vec(protocol_associations, |w, r| {
        w.def_path(&r.association.source)
    });
    weight.arc_slice(types);
    weight.vec(bodies, Weight::body);
    weight.flat_vec(contracts);
    weight.bytes
}

/// Arc allocation including its inline value; payload allocations are separate.
pub(crate) fn retained_arc_bytes<T: ?Sized>(value: &T) -> u64 {
    arc_bytes(value) as u64
}

/// Backing allocations of a worker's Lowered wrapper, excluding the wrapper's
/// own Arc/inline layout and its DefinitionRef (charged by retained_tir_bytes).
/// Signature and other shared payloads are deduplicated within the registration
/// vector. Session interned rows are excluded, as with the TIR weight.
pub(crate) fn retained_lowering_metadata_bytes(
    reads: &Reads,
    depth: &DepthEvidence<hirsel_span::Span>,
    registrations: &Vec<StagedRegistration>,
) -> u64 {
    let mut weight = Weight::default();
    for (identity, bytes) in reads.allocations() {
        weight.allocation(identity, bytes);
    }
    weight.add(size_of_val(&*depth.first));
    weight.vec(registrations, |w, registration| {
        w.def_path(&registration.path);
        w.def_path(&registration.constant.owner);
        w.flat_vec(&registration.constant.inputs);
        w.signature(&registration.constant.signature);
    });
    weight.bytes
}

/// Owned buffers of lowering's returned warnings and ordered signature
/// dependencies, excluding the containing Vec/inline fields of its wrapper.
pub(crate) fn retained_lowering_output_bytes(
    diagnostics: &Vec<hirsel_diag::Diagnostic>,
    dependencies: &Vec<crate::SignatureDependency>,
) -> u64 {
    let mut weight = Weight::default();
    weight.flat_vec(dependencies);
    weight.vec(diagnostics, Weight::diagnostic);
    weight.bytes
}

/// Owned staged declaration payloads. Signature Arcs are deduplicated within
/// the set; interned type rows remain with the pinned source snapshot.
pub(crate) fn retained_staged_declaration_bytes(registrations: &Vec<StagedRegistration>) -> u64 {
    let mut weight = Weight::default();
    weight.vec(registrations, |w, registration| {
        w.def_path(&registration.path);
        w.def_path(&registration.constant.owner);
        w.flat_vec(&registration.constant.inputs);
        w.signature(&registration.constant.signature);
    });
    weight.bytes
}

/// Owned diagnostic/error buffers, excluding the containing inline enum.
pub(crate) fn retained_query_error_bytes(error: &hirsel_types::QueryError) -> u64 {
    let mut weight = Weight::default();
    weight.error(error);
    weight.bytes
}

/// Exact worker staged-use closure. DefIds belong to the session; argument
/// vector capacities are retained by the lower memo and charged in its hold.
pub(crate) fn retained_staged_request_bytes(requests: &Vec<crate::comptime::Selection>) -> u64 {
    let mut weight = Weight::default();
    weight.vec(requests, |w, selection| {
        if let crate::comptime::Selection::Staged(_, arguments) = selection {
            w.flat_vec(arguments);
        }
    });
    weight.bytes
}

/// One scalar Lower cache entry and its exact-artifact V evidence. Compact
/// receipts outlive eviction independently; table buckets/session interners
/// belong to the session, as with worker holds.
pub(crate) fn retained_lower_entry_bytes(
    entry: &crate::queries::Entry<hirsel_tir::DefinitionRef>,
    verdict: Option<&Vec<hirsel_validate::VRead>>,
) -> u64 {
    let mut weight = Weight::default();
    weight.add(arc_bytes(entry));
    weight.add(arc_bytes(&*entry.memo.reads));
    for (identity, bytes) in entry.reads().allocations() {
        weight.allocation(identity, bytes);
    }
    weight.detail(&entry.detail);
    weight.flat_vec(&entry.covered);
    weight.vec(&entry.selections, |w, selection| {
        if let crate::comptime::Selection::Staged(_, arguments) = selection {
            w.flat_vec(arguments);
        }
    });
    if let Some(depth) = &entry.depth {
        weight.add(size_of_val(&*depth.first));
    }
    if let Some(receipt) = &entry.acceptance {
        weight.add(arc_bytes(receipt.contexts()));
        weight.add(arc_bytes(receipt.artifacts()));
    }
    // Lowering has no phase-products sink: these are the empty local products,
    // never a resolver syntax snapshot or an owner representation.
    debug_assert!(entry.products.resolution.is_none() && entry.products.representation.is_none());
    weight.add(arc_bytes(&*entry.products));
    weight.vec(
        &entry.products.mode_diagnostics,
        |w, (diagnostic, phase)| {
            w.diagnostic(diagnostic);
            w.add(phase.capacity());
        },
    );
    match &entry.memo.value {
        Ok(output) => {
            weight.add(arc_bytes(&**output));
            weight.bytes = weight
                .bytes
                .saturating_add(retained_tir_bytes(&output.value));
            weight.vec(&output.diagnostics, Weight::diagnostic);
            weight.flat_vec(&output.dependencies);
        }
        Err(error) => weight.error(error),
    }
    if let Some(reads) = verdict {
        weight.add(
            std::mem::size_of::<hirsel_validate::VerifiedDefinition>()
                + 2 * std::mem::size_of::<usize>(),
        );
        weight.flat_vec(reads);
    }
    weight.bytes
}

fn arc_bytes<T: ?Sized>(value: &T) -> usize {
    Layout::new::<[usize; 2]>()
        .extend(Layout::for_value(value))
        .expect("an allocated Arc has a valid layout")
        .0
        .pad_to_align()
        .size()
}

#[derive(Default)]
pub(crate) struct Weight {
    pub(crate) bytes: u64,
    shared: HashSet<usize>,
    count_shared_repeats: bool,
    decode_vectors: bool,
}
impl Weight {
    /// Serde recreates every serialized Arc occurrence. Sequence decoding may
    /// grow from its cautious initial capacity; doubling the producing Vec's
    /// capacity covers that geometric growth without relying on wire length.
    pub(crate) fn decoded() -> Self {
        Self {
            count_shared_repeats: true,
            decode_vectors: true,
            ..Self::default()
        }
    }
    fn first(&mut self, identity: usize) -> bool {
        self.count_shared_repeats || self.shared.insert(identity)
    }
    pub(crate) fn allocation(&mut self, identity: usize, bytes: usize) -> bool {
        if identity != 0 && !self.first(identity) {
            return false;
        }
        self.add(bytes);
        true
    }
    pub(crate) fn arc<T: ?Sized>(
        &mut self,
        value: &std::sync::Arc<T>,
        visit: impl FnOnce(&mut Self, &T),
    ) {
        if self.allocation(
            std::sync::Arc::as_ptr(value).cast::<()>() as usize,
            arc_bytes(&**value),
        ) {
            visit(self, value);
        }
    }
    pub(crate) fn reads(&mut self, value: &std::sync::Arc<Reads>) {
        self.arc(value, |w, reads| {
            for (identity, bytes) in reads.allocations() {
                w.allocation(identity, bytes);
            }
        });
        if self.decode_vectors && !value.is_empty() {
            // Reads::decode builds a cautious Vec, then Reads::from_vec
            // deduplicates >=32 rows in a HashMap and collects a second Vec
            // before moving the result into its final Arc slice. Their
            // working buffers are distinct from the retained read allocation.
            let slots = value.len().saturating_mul(2).max(4);
            self.add(
                slots
                    .saturating_mul(2)
                    .saturating_mul(size_of::<(ReadKey, ContentHash)>()),
            );
            if value.len() >= 32 {
                // hashbrown raw capacity_to_buckets / calculate_layout_for:
                // 7/8 occupancy, power-of-two buckets, one control byte per
                // bucket plus a SIMD control group and alignment. Both old
                // and new tables may coexist while a growing table rehashes.
                let buckets = value.len().saturating_mul(8).saturating_add(6) / 7;
                let buckets = buckets.checked_next_power_of_two().unwrap_or(usize::MAX);
                let rows = Layout::new::<(ReadKey, ContentHash)>();
                let table = buckets
                    .saturating_mul(rows.size().saturating_add(1))
                    .saturating_add(16)
                    .saturating_add(rows.align());
                self.add(table.saturating_mul(2));
            }
        }
    }
    pub(crate) fn diagnostic(&mut self, diagnostic: &hirsel_diag::Diagnostic) {
        let hirsel_diag::Diagnostic {
            rule,
            severity: _,
            message,
            primary: _,
            secondary,
            notes,
            fix,
            obligations,
            root_cause: _,
        } = diagnostic;
        self.add(rule.retained_bytes());
        self.add(message.capacity());
        self.vec(secondary, |w, label| w.add(label.message.capacity()));
        self.vec(notes, |w, note| w.add(note.capacity()));
        if let Some(fix) = fix {
            self.add(fix.replacement.capacity());
            self.add(fix.message.capacity());
        }
        self.vec(obligations, |w, obligation| match obligation {
            hirsel_diag::Obligation::GenericBound { arguments } => w.flat_vec(arguments),
            hirsel_diag::Obligation::Multiplicity
            | hirsel_diag::Obligation::CopyProof
            | hirsel_diag::Obligation::ParameterMode { .. } => (),
        });
    }
    fn package(&mut self, package: &hirsel_syntax::PackageId) {
        self.add(package.name.capacity());
        self.add(package.version.capacity());
    }
    fn context(&mut self, context: &hirsel_syntax::FileContext) {
        self.package(&context.package);
        self.vec(&context.modules, |w, part| w.add(part.capacity()));
    }
    pub(crate) fn error(&mut self, error: &hirsel_types::QueryError) {
        match error {
            hirsel_types::QueryError::Operational(failure) => self.failure(failure),
            hirsel_types::QueryError::Diagnostics(diagnostics) => {
                self.vec(diagnostics, Weight::diagnostic)
            }
            hirsel_types::QueryError::NotImplemented => (),
        }
    }
    fn failure(&mut self, failure: &hirsel_types::OperationalFailure) {
        self.add(failure.operation.capacity());
        self.add(failure.message.capacity());
    }
    fn dependency(&mut self, read: &hirsel_types::DependencyRead) {
        use hirsel_types::{DependencyRead, LookupError};
        match read {
            DependencyRead::ProviderLookup {
                context,
                package,
                segments,
                output,
            } => {
                self.context(context);
                if let Some(package) = package {
                    self.package(package);
                }
                self.vec(segments, |w, segment| w.add(segment.capacity()));
                match &output.result {
                    Ok(path) => self.def_path(path),
                    Err(error) => self.error(error),
                }
            }
            DependencyRead::Lookup {
                context,
                package,
                segments,
                result,
                manifest: _,
            } => {
                self.context(context);
                if let Some(package) = package {
                    self.package(package);
                }
                self.vec(segments, |w, segment| w.add(segment.capacity()));
                match result {
                    Ok(path) | Err(LookupError::Invisible(path)) => self.def_path(path),
                    Err(LookupError::MissingMember(paths) | LookupError::Ambiguous(paths)) => {
                        self.vec(paths, Weight::def_path)
                    }
                    Err(LookupError::Provider(diagnostics)) => {
                        self.vec(diagnostics, Weight::diagnostic)
                    }
                    Err(LookupError::Missing) => (),
                }
            }
            DependencyRead::ModuleHead {
                context,
                head,
                manifest: _,
                exposed: _,
            } => {
                self.context(context);
                self.add(head.capacity());
            }
            // Definition identities are owned by the session interner.
            DependencyRead::Signature { .. } => (),
        }
    }
    fn read_log(&mut self, value: &std::sync::Arc<SharedPath<hirsel_types::DependencyRead>>) {
        if !self.first(std::sync::Arc::as_ptr(value) as usize) {
            return;
        }
        self.add(arc_bytes(&**value));
        let mut path = &**value;
        while let Some(identity) = path.node_identity() {
            if !self.first(identity) {
                break;
            }
            self.add(path.node_allocation_bytes());
            self.dependency(path.last().unwrap());
            path = path.prefix().unwrap();
        }
    }
    fn detail(&mut self, detail: &crate::DependencyReads) {
        let crate::DependencyReads {
            operational_failure,
            signatures,
            failed_signatures,
            failed_signature_keys,
            lookups,
            manifest_lookups,
            resolver,
            citations,
        } = detail;
        if let Some(failure) = operational_failure {
            self.failure(failure);
        }
        self.flat_vec(signatures);
        self.vec(failed_signatures, Weight::def_path);
        self.vec(failed_signature_keys, |w, (path, _)| w.def_path(path));
        self.vec(lookups, |w, (path, _)| w.def_path(path));
        self.vec(manifest_lookups, Weight::dependency);
        self.read_log(resolver);
        self.vec(citations, |w, (path, _)| w.def_path(path));
    }
    pub(crate) fn add(&mut self, bytes: usize) {
        self.bytes = self.bytes.saturating_add(bytes as u64);
    }
    pub(crate) fn flat_vec<T>(&mut self, value: &Vec<T>) {
        self.add(
            value
                .capacity()
                .saturating_mul(if self.decode_vectors { 2 } else { 1 })
                .saturating_mul(size_of::<T>()),
        );
    }
    pub(crate) fn vec<T>(&mut self, value: &Vec<T>, mut visit: impl FnMut(&mut Self, &T)) {
        self.flat_vec(value);
        for element in value {
            visit(self, element);
        }
    }
    fn arc_slice<T>(&mut self, value: &std::sync::Arc<[T]>) {
        if self.first(value.as_ptr() as usize) {
            self.add(arc_bytes(&**value));
        }
    }
    fn shared_slice<T>(&mut self, value: &SharedSlice<T>) {
        let identity = value.storage_identity();
        if identity != 0 && self.first(identity) {
            self.add(arc_bytes(value.as_slice()));
        }
    }
    fn shared_values<T>(&mut self, value: &SharedSlice<T>, mut visit: impl FnMut(&mut Self, &T)) {
        let identity = value.storage_identity();
        if identity != 0 && self.first(identity) {
            self.add(arc_bytes(value.as_slice()));
            for element in value {
                visit(self, element);
            }
        }
    }
    pub(crate) fn generic(&mut self, value: &hirsel_types::GenericParameter) {
        match value {
            hirsel_types::GenericParameter::Type { name, bounds } => {
                self.add(name.capacity());
                self.shared_slice(bounds);
            }
            hirsel_types::GenericParameter::Size { name } => self.add(name.capacity()),
        }
    }
    fn authority(&mut self, value: &hirsel_types::AuthoritySummary) {
        match value {
            hirsel_types::AuthoritySummary::Known(terms) => {
                self.shared_values(terms, |w, term| match term {
                    hirsel_types::AuthorityTerm::Capability(path) => w.def_path(path),
                    hirsel_types::AuthorityTerm::Parameter(_)
                    | hirsel_types::AuthorityTerm::TypeParameter(_) => (),
                })
            }
            hirsel_types::AuthoritySummary::Unknown => (),
        }
    }
    pub(crate) fn signature(&mut self, value: &hirsel_types::SignatureRef) {
        if !self.first(std::sync::Arc::as_ptr(value) as usize) {
            return;
        }
        self.add(arc_bytes(&**value));
        let hirsel_types::Signature {
            metadata,
            definition: _,
            kind,
            fingerprint: _,
        } = &**value;
        let hirsel_types::SignatureMetadata {
            visibility: _,
            alias: _,
            constant_alias,
            track_caller: _,
            unsafe_ffi: _,
            wrapper: _,
            roles,
            schema,
            recursive_group: _,
            implementations,
        } = metadata;
        for binder in implementations {
            self.def_path(binder.family.path());
        }
        if let Some(path) = constant_alias {
            self.def_path(path);
        }
        let hirsel_types::NominalRoles {
            unsafe_capability,
            result_type,
            tasks_capability,
            scope_capability,
            heap,
        } = roles;
        for path in [
            unsafe_capability,
            result_type,
            tasks_capability,
            scope_capability,
        ]
        .into_iter()
        .flatten()
        {
            self.def_path(path);
        }
        if let Some(hirsel_types::HeapRole { arena, operation }) = heap {
            self.def_path(arena);
            self.def_path(operation);
        }
        self.flat_vec(schema);
        match kind {
            hirsel_types::SignatureKind::Const { generics, ty: _ } => {
                self.shared_values(generics, Weight::generic)
            }
            hirsel_types::SignatureKind::Family(family) => {
                for member in &family.members {
                    self.add(member.name.capacity());
                }
            }
            hirsel_types::SignatureKind::Implementation(implementation) => {
                self.def_path(implementation.family.path());
                for (name, actual) in &implementation.members {
                    self.add(name.capacity());
                    if let hirsel_types::MemberActual::Operation(target) = actual {
                        self.def_path(target);
                    }
                }
            }
            hirsel_types::SignatureKind::Function(function) => {
                let hirsel_types::FunctionType {
                    state_promises,
                    generics,
                    parameters,
                    result: _,
                    error: _,
                    allocation: _,
                    authority,
                    uses,
                    contracts,
                    incomplete: _,
                    foreign,
                } = function;
                self.shared_values(state_promises, |w, promise| {
                    w.path(&promise.selector.fields);
                    w.shared_slice(&promise.unretained);
                });
                self.shared_values(generics, Weight::generic);
                self.shared_values(parameters, |w, parameter| {
                    for name in [&parameter.name, &parameter.label].into_iter().flatten() {
                        w.add(name.capacity());
                    }
                });
                self.authority(authority);
                match uses {
                    hirsel_types::Uses::Only(terms) => self.authority(terms),
                    hirsel_types::Uses::Unrestricted => (),
                }
                self.shared_slice(contracts);
                if let Some(foreign) = foreign {
                    self.add(foreign.abi.capacity());
                    self.add(foreign.aliasing_contract.capacity());
                }
            }
            hirsel_types::SignatureKind::Type(definition) => self.type_definition(definition),
        }
    }
    fn type_definition(&mut self, definition: &hirsel_types::TypeDefinition) {
        let hirsel_types::TypeDefinition {
            loop_source,
            format,
            destructor_allocation: _,
            generics,
            representation: _,
            bounds,
            authority,
            capability: _,
            implementation_authority,
            serializable: _,
            serializable_check,
            layout: _,
            distinct: _,
            resource: _,
            drop,
            fact: _,
            captures,
        } = definition;
        if let Some(hirsel_types::LoopSourceDeclaration { open, next, option }) = loop_source {
            self.def_path(open);
            self.def_path(next);
            self.def_path(option);
        }
        for path in [format, drop].into_iter().flatten() {
            self.def_path(path);
        }
        self.vec(generics, Weight::generic);
        self.flat_vec(bounds);
        self.authority(authority);
        if let Some(authority) = implementation_authority {
            self.authority(authority);
        }
        if let Some(check) = serializable_check {
            self.def_path(&check.function);
        }
        if let Some(captures) = captures {
            self.shared_slice(captures);
        }
    }
    fn path<T>(&mut self, mut value: &SharedPath<T>) {
        while let Some(identity) = value.node_identity() {
            if !self.first(identity) {
                break;
            }
            self.add(value.node_allocation_bytes());
            value = value.prefix().expect("nonempty paths have a prefix");
        }
    }
    pub(crate) fn def_path(&mut self, value: &DefPath) {
        let hirsel_syntax::ItemPath {
            package,
            modules,
            name,
        } = &value.0;
        self.add(package.name.capacity());
        self.add(package.version.capacity());
        self.add(name.capacity());
        self.vec(modules, |w, name| w.add(name.capacity()));
    }
    fn origin(&mut self, value: &Origin) {
        match &value.root {
            OriginRoot::Definition(path) => self.def_path(path),
            OriginRoot::ArgumentBrand(_)
            | OriginRoot::Expression(..)
            | OriginRoot::Parameter(_)
            | OriginRoot::Local(..)
            | OriginRoot::Sibling(_) => (),
        }
        self.path(&value.fields);
    }
    fn destination(&mut self, value: &Destination) {
        match value {
            Destination::Arena(origin) => self.origin(origin),
            Destination::Caller(parameters) => self.shared_slice(parameters),
            Destination::Heap | Destination::Region(_) | Destination::Inferred => (),
        }
    }
    pub(crate) fn constant(&mut self, value: &Constant) {
        match value {
            Constant::String(value) => self.add(value.capacity()),
            Constant::Bytes(value) => self.flat_vec(value),
            Constant::Unit
            | Constant::Bool(_)
            | Constant::Integer(_)
            | Constant::F32(_)
            | Constant::F64(_) => (),
        }
    }
    pub(crate) fn constructor(&mut self, value: &Constructor) {
        match value {
            Constructor::Record(path)
            | Constructor::Variant {
                definition: path, ..
            } => {
                self.def_path(path);
            }
            Constructor::Tuple
            | Constructor::Sequence
            | Constructor::Map
            | Constructor::Set
            | Constructor::SortedMap { .. }
            | Constructor::Box => (),
        }
    }
    fn rvalue(&mut self, value: &Rvalue) {
        match value {
            Rvalue::Constant(value) => self.constant(value),
            Rvalue::ReadConstant {
                definition,
                arguments,
            } => {
                self.def_path(definition);
                self.flat_vec(arguments);
            }
            Rvalue::FunctionReference {
                definition,
                arguments,
            } => {
                self.def_path(definition);
                self.flat_vec(arguments);
            }
            Rvalue::Construct {
                constructor,
                fields,
                destination,
            } => {
                self.constructor(constructor);
                self.flat_vec(fields);
                self.destination(destination);
            }
            Rvalue::Closure {
                captures,
                destination,
                ..
            } => {
                self.flat_vec(captures);
                self.destination(destination);
            }
            Rvalue::Size(_)
            | Rvalue::Use(_)
            | Rvalue::Unary { .. }
            | Rvalue::Binary { .. }
            | Rvalue::Shape(_) => (),
        }
    }
    fn instruction(&mut self, instruction: &Instruction) {
        match &instruction.kind {
            InstructionKind::BindFields { fields, .. } => self.flat_vec(fields),
            InstructionKind::Defuse { .. }
            | InstructionKind::BindOrigin { .. }
            | InstructionKind::EndOrigin(_)
            | InstructionKind::StorageLive(_)
            | InstructionKind::StorageDead { .. }
            | InstructionKind::EndScope(_)
            | InstructionKind::Assign { .. }
            | InstructionKind::Replace { .. }
            | InstructionKind::BeginLoan { .. }
            | InstructionKind::EndLoan { .. }
            | InstructionKind::Drop { .. }
            | InstructionKind::RunCleanup(_)
            | InstructionKind::Check { .. }
            | InstructionKind::Snapshot { .. }
            | InstructionKind::Charge(_) => (),
        }
    }
    fn terminator(&mut self, value: &Terminator) {
        match value {
            Terminator::Switch { arms, .. } => self.flat_vec(arms),
            Terminator::Call {
                callee, arguments, ..
            } => {
                self.flat_vec(arguments);
                match callee {
                    Callee::Definition { path, arguments } => {
                        self.def_path(path);
                        self.flat_vec(arguments);
                    }
                    Callee::Indirect { arguments, .. } => self.flat_vec(arguments),
                }
            }
            Terminator::Goto(_)
            | Terminator::Match { .. }
            | Terminator::Branch { .. }
            | Terminator::Return { .. }
            | Terminator::Fail { .. }
            | Terminator::Unreachable => (),
        }
    }
    fn pattern(&mut self, value: &MatchPattern) {
        match &value.kind {
            MatchPatternKind::Literal(value) => self.constant(value),
            MatchPatternKind::Range { start, end } => {
                self.constant(start);
                self.constant(end);
            }
            MatchPatternKind::Tuple(fields) => self.flat_vec(fields),
            MatchPatternKind::Record { definition, fields }
            | MatchPatternKind::Variant {
                definition, fields, ..
            } => {
                self.def_path(definition);
                self.flat_vec(fields);
            }
            MatchPatternKind::Wildcard | MatchPatternKind::Binding { .. } => (),
        }
    }
    fn body(&mut self, body: &Body) {
        let Body {
            arena_scope_grant,
            scoped_parameters,
            parameter_scope: _,
            locals,
            places,
            scopes,
            blocks,
            rvalues,
            cleanups,
            cleanup_nodes,
            entry: _,
            parameters,
            result: _,
            tail_sites,
            borrow_cursors,
            matches,
        } = body;
        if let Some(grant) = arena_scope_grant {
            self.def_path(&grant.intrinsic);
        }
        self.flat_vec(locals);
        self.vec(places, |w, place| {
            w.path(&place.projection);
            if let Some(origin) = &place.binding {
                w.origin(origin);
            }
        });
        self.flat_vec(scopes);
        self.vec(blocks, |w, block| {
            w.vec(&block.instructions, Weight::instruction);
            w.terminator(&block.terminator);
        });
        self.vec(rvalues, Weight::rvalue);
        self.flat_vec(cleanups);
        self.flat_vec(cleanup_nodes);
        self.flat_vec(parameters);
        self.vec(scoped_parameters, |w, p| w.flat_vec(&p.fields));
        self.vec(borrow_cursors, |w, cursor| {
            if let Some(cover) = &cursor.cover {
                w.flat_vec(cover);
            }
        });
        self.vec(tail_sites, |w, site| {
            w.flat_vec(&site.owned_loop_arguments);
            match &site.kind {
                TailKind::ConstructorHole(path) => w.flat_vec(path),
                TailKind::SelfTail => (),
            }
        });
        self.vec(matches, |w, table| {
            w.vec(&table.patterns, Weight::pattern);
            w.flat_vec(&table.arms);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_types::{Type, TypeInterner};

    fn definition() -> Definition {
        let types = TypeInterner::new(None);
        let unit = types.intern(Type::unit());
        Definition {
            caller_bodies: vec![],
            protocol_associations: vec![],
            types: vec![unit].into(),
            path: hirsel_syntax::DefId::intern(&DefPath::core("held")),
            signature_hash: Default::default(),
            bodies: vec![Body {
                arena_scope_grant: None,
                scoped_parameters: Vec::new(),
                parameter_scope: hirsel_types::LexicalOwner::first(&DefPath::core("held"))
                    .site(hirsel_types::NameSite::Item),
                locals: vec![],
                places: vec![],
                scopes: vec![],
                blocks: vec![],
                rvalues: Vec::with_capacity(8),
                cleanups: vec![],
                cleanup_nodes: vec![],
                entry: BlockId(0),
                parameters: vec![],
                result: unit,
                borrow_cursors: vec![],
                tail_sites: vec![],
                matches: vec![],
            }],
            entry: BodyId(0),
            contracts: vec![],
        }
    }

    #[test]
    fn retained_storage_includes_spare_capacity_and_owned_literal_buffers() {
        let mut definition = definition();
        let empty = retained_tir_bytes(&definition);
        definition.bodies[0]
            .rvalues
            .push(Rvalue::Constant(Constant::Unit));
        assert_eq!(
            retained_tir_bytes(&definition),
            empty,
            "inline value uses spare capacity"
        );
        let mut text = String::with_capacity(32_768);
        text.push('x');
        let text_bytes = text.capacity() as u64;
        definition.bodies[0].rvalues[0] = Rvalue::Constant(Constant::String(text));
        assert_eq!(retained_tir_bytes(&definition) - empty, text_bytes);
        let bytes = Vec::with_capacity(65_536);
        let bytes_capacity = bytes.capacity() as u64;
        definition.bodies[0]
            .rvalues
            .push(Rvalue::Constant(Constant::Bytes(bytes)));
        assert_eq!(
            retained_tir_bytes(&definition) - empty,
            text_bytes + bytes_capacity
        );
        definition.bodies[0].rvalues.reserve(256);
        assert!(retained_tir_bytes(&definition) > empty + text_bytes + bytes_capacity);
        definition.bodies[0].rvalues.clear();
        assert!(
            retained_tir_bytes(&definition) > empty,
            "clearing leaves the arena allocated"
        );
        definition.bodies[0].rvalues.shrink_to_fit();
        assert!(retained_tir_bytes(&definition) < empty);
    }

    #[test]
    fn lower_entry_weight_includes_lookup_failures_and_spare_receipt_buffers() {
        use std::sync::Arc;
        let mut graph = crate::memo::MemoGraph::default();
        let output = crate::QueryOutput {
            value: Arc::new(definition()),
            diagnostics: vec![],
            dependencies: vec![],
        };
        let key = crate::memo::MemoKey::new(crate::QueryKind::Lower, output.value.path);
        let mut entry = crate::queries::Entry {
            memo: graph.insert(
                key,
                Default::default(),
                Ok(Arc::new(output)),
                Arc::new(Reads::empty()),
                super::super::Durability::Low,
                false,
            ),
            detail: Default::default(),
            products: Default::default(),
            covered: vec![],
            depth: None,
            acceptance: None,
            verification: Default::default(),
            selections: vec![],
            consistent: true,
        };
        let base = retained_lower_entry_bytes(&entry, None);
        entry
            .detail
            .manifest_lookups
            .push(hirsel_types::DependencyRead::ProviderLookup {
                context: crate::default_context(),
                package: None,
                segments: vec!["missing".repeat(4096)],
                output: hirsel_types::LookupOutput {
                    manifest: Default::default(),
                    result: Err(hirsel_types::QueryError::Operational(
                        hirsel_types::OperationalFailure {
                            operation: "lookup".into(),
                            message: "failure".repeat(4096),
                        },
                    )),
                },
            });
        let failed = retained_lower_entry_bytes(&entry, None);
        assert!(
            failed > base + 50_000,
            "negative lookup owns both request and failure text"
        );
        std::sync::Arc::make_mut(&mut entry.detail.resolver).extend((0..256).map(|_| {
            hirsel_types::DependencyRead::Signature {
                definition: hirsel_syntax::DefId::intern(&DefPath::core("dependency")),
                fingerprint: None,
            }
        }));
        assert!(retained_lower_entry_bytes(&entry, None) > failed);
        let no_depth = retained_lower_entry_bytes(&entry, None);
        entry.depth = Some(DepthEvidence::leaf(32));
        assert!(retained_lower_entry_bytes(&entry, None) > no_depth);
        let verdict = Vec::with_capacity(512);
        assert!(
            retained_lower_entry_bytes(&entry, Some(&verdict))
                > retained_lower_entry_bytes(&entry, None)
        );
    }

    #[test]
    fn read_log_weight_counts_shared_prefix_payloads_once_and_new_nodes_in_full() {
        use hirsel_types::DependencyRead;
        use std::sync::Arc;
        let original: SharedPath<_> = vec![DependencyRead::ProviderLookup {
            context: crate::default_context(),
            package: None,
            segments: vec!["missing".repeat(4096)],
            output: hirsel_types::LookupOutput {
                manifest: Default::default(),
                result: Err(hirsel_types::QueryError::NotImplemented),
            },
        }]
        .into();
        let original = Arc::new(original);
        let mut branch = (*original).clone();
        branch.push(DependencyRead::Signature {
            definition: hirsel_syntax::DefId::intern(&DefPath::core("callee")),
            fingerprint: None,
        });
        let branch = Arc::new(branch);
        let mut weight = Weight::default();
        weight.read_log(&original);
        let first = weight.bytes;
        assert!(first > 28_000);
        weight.read_log(&branch);
        let expected_delta = arc_bytes(&*branch) + branch.node_allocation_bytes();
        assert_eq!(weight.bytes, first + expected_delta as u64);
        weight.read_log(&original);
        weight.read_log(&branch);
        assert_eq!(weight.bytes, first + expected_delta as u64);
        let mut independent = Weight::default();
        independent.read_log(&branch);
        assert_eq!(
            independent.bytes,
            first + branch.node_allocation_bytes() as u64
        );
    }

    #[test]
    fn shared_prefixes_and_destinations_are_charged_once_per_artifact() {
        let mut weight = Weight::default();
        let mut path: SharedPath<_> = [Projection::Field(0), Projection::Field(1)]
            .into_iter()
            .collect();
        let prefix = path.prefix().unwrap().clone();
        weight.path(&path);
        let two = weight.bytes;
        weight.path(&prefix);
        weight.path(&path.clone());
        assert_eq!(weight.bytes, two, "shared prefixes are retained once");
        path.push(Projection::Field(2));
        weight.path(&path);
        assert!(weight.bytes > two);
        let callers: SharedSlice<_> =
            vec![hirsel_types::ParameterRef::bound(hirsel_types::ParamId(0))].into();
        weight.destination(&Destination::Caller(callers.clone()));
        let one_destination = weight.bytes;
        weight.destination(&Destination::Caller(callers));
        assert_eq!(weight.bytes, one_destination);
    }

    #[test]
    fn signature_read_weight_excludes_session_owned_path_payload() {
        use hirsel_types::DependencyRead;
        let path = DefPath::core(&"helper".repeat(1000));
        let read = DependencyRead::Signature {
            definition: hirsel_syntax::DefId::intern(&path),
            fingerprint: None,
        };
        let mut weight = Weight::default();
        weight.dependency(&read);
        assert_eq!(weight.bytes, 0);
        // The vector still owns and charges its reserved inline enum rows.
        let mut reads = Vec::with_capacity(8);
        let capacity = reads.capacity();
        reads.push(read);
        weight.vec(&reads, Weight::dependency);
        assert_eq!(
            weight.bytes,
            (capacity * size_of::<DependencyRead>()) as u64
        );
        // A lookup's owned result path remains a separate allocation.
        weight.def_path(&path);
        assert!(weight.bytes >= 6000);
    }

    #[test]
    fn lowering_output_weight_includes_owned_diagnostic_and_dependency_buffers() {
        let mut diagnostics = Vec::with_capacity(4);
        let mut dependencies = Vec::with_capacity(8);
        let base = retained_lowering_output_bytes(&diagnostics, &dependencies);
        let mut diagnostic = hirsel_diag::Diagnostic::error(
            hirsel_diag::RuleId::new("LANG-1").unwrap(),
            hirsel_span::Span::default(),
            "warning",
        );
        diagnostic.message.reserve(8192);
        diagnostic.secondary.push(hirsel_diag::Label {
            span: hirsel_span::Span::default(),
            message: "label".repeat(200),
        });
        diagnostic.notes.push("note".repeat(400));
        diagnostic.fix = Some(hirsel_diag::Fix {
            span: hirsel_span::Span::default(),
            replacement: "x".repeat(1000),
            message: "fix".into(),
        });
        diagnostic
            .obligations
            .push(hirsel_diag::Obligation::GenericBound {
                arguments: Vec::with_capacity(128),
            });
        diagnostics.push(diagnostic);
        let warned = retained_lowering_output_bytes(&diagnostics, &dependencies);
        assert!(warned > base + 10_000);
        dependencies.push(crate::SignatureDependency {
            definition: hirsel_syntax::DefId::intern(&DefPath::core(&"helper".repeat(1000))),
            fingerprint: Default::default(),
        });
        // Interned paths belong to the session. The already reserved vector
        // owns the same bytes regardless of a dependency's path length.
        assert_eq!(
            retained_lowering_output_bytes(&diagnostics, &dependencies),
            warned
        );
    }

    #[test]
    fn lowering_metadata_accounts_for_receipts_depth_and_shared_registration_signatures() {
        use std::sync::Arc;
        let types = TypeInterner::new(None);
        let signature = Arc::new(hirsel_types::Signature {
            metadata: hirsel_types::SignatureMetadata::default(),
            definition: hirsel_syntax::DefId::intern(&DefPath::core("staged")),
            kind: hirsel_types::SignatureKind::Const {
                ty: types.intern(Type::unit()),
                generics: vec![hirsel_types::GenericParameter::Size {
                    name: "n".repeat(65_536),
                }]
                .into(),
            },
            fingerprint: Default::default(),
        });
        let reads = Reads::from_vec(vec![(
            super::super::ReadKey::Environment,
            Default::default(),
        )])
        .unwrap();
        let mut depth = DepthEvidence::leaf(3);
        let mut registrations = Vec::with_capacity(2);
        let no_registration = retained_lowering_metadata_bytes(&reads, &depth, &registrations);
        depth.first = vec![None; 128].into_boxed_slice();
        assert!(retained_lowering_metadata_bytes(&reads, &depth, &registrations) > no_registration);
        let registration = StagedRegistration {
            path: DefPath::core("staged"),
            constant: crate::comptime::StagedConstant {
                owner: DefPath::core("owner"),
                expression: hirsel_syntax::ExprId(0),
                inputs: Vec::with_capacity(512),
                signature,
            },
        };
        registrations.push(registration.clone());
        let one = retained_lowering_metadata_bytes(&reads, &depth, &registrations);
        registrations.push(registration);
        let shared = retained_lowering_metadata_bytes(&reads, &depth, &registrations);
        assert!(one > 65_536);
        assert!(
            shared - one < 65_536,
            "same signature allocation charged once"
        );
        let mut independent = (*registrations[1].constant.signature).clone();
        // Cloning a signature shares its generic list. Detach that list so the
        // second registration now owns a separate large name allocation.
        let hirsel_types::SignatureKind::Const { generics, .. } = &mut independent.kind else {
            unreachable!()
        };
        *generics = generics.iter().cloned().collect();
        registrations[1].constant.signature = Arc::new(independent);
        assert!(
            retained_lowering_metadata_bytes(&reads, &depth, &registrations) >= shared + 65_536
        );
        let with_reads = retained_lowering_metadata_bytes(&reads, &depth, &registrations);
        assert!(
            with_reads > retained_lowering_metadata_bytes(&Reads::empty(), &depth, &registrations)
        );
    }

    #[test]
    fn nested_calls_matches_bindings_and_origins_retain_their_buffers() {
        let mut definition = definition();
        let body = &mut definition.bodies[0];
        body.blocks.push(Block {
            instructions: vec![Instruction {
                kind: InstructionKind::BindFields {
                    source: PlaceId(0),
                    fields: Vec::with_capacity(2048),
                },
                span: Default::default(),
                on_failure: CleanupId(0),
            }],
            terminator: Terminator::Call {
                callee: Callee::Definition {
                    path: DefPath::core("callee"),
                    arguments: Vec::with_capacity(2048),
                },
                arguments: Vec::with_capacity(2048),
                returns: None,
                on_failure: CleanupId(0),
            },
            span: Default::default(),
        });
        body.matches.push(MatchTable {
            scrutinee: PlaceId(0),
            patterns: vec![MatchPattern {
                place: PlaceId(0),
                kind: MatchPatternKind::Literal(Constant::Bytes(Vec::with_capacity(32_768))),
            }],
            arms: Vec::with_capacity(2048),
        });
        body.places.push(Place {
            root: LocalId(0),
            loan: None,
            ty: body.result,
            projection: vec![Projection::Field(0)].into(),
            binding: Some(Origin {
                root: OriginRoot::Definition(DefPath::core(&"x".repeat(8192))),
                fields: Default::default(),
            }),
        });
        let nested = retained_tir_bytes(&definition);
        assert!(nested > 32_768 + 8192);
        let body = &mut definition.bodies[0];
        let InstructionKind::BindFields { fields, .. } = &mut body.blocks[0].instructions[0].kind
        else {
            unreachable!()
        };
        fields.shrink_to_fit();
        assert!(retained_tir_bytes(&definition) < nested);
        let before = retained_tir_bytes(&definition);
        definition.bodies[0].matches[0].patterns.clear();
        assert_eq!(before - retained_tir_bytes(&definition), 32_768);
    }
    #[test]
    fn decoded_tir_counts_each_serialized_shared_path_occurrence() {
        let mut value = definition();
        let mut path = SharedPath::default();
        path.push(Projection::Field(0));
        path.push(Projection::Field(1));
        let row = Place {
            root: LocalId(0),
            loan: None,
            ty: value.bodies[0].result,
            projection: path,
            binding: None,
        };
        value.bodies[0].places = vec![row.clone(), row];
        let retained = retained_tir_bytes(&value);
        let decoded = decoded_tir_bytes(&value);
        assert!(
            decoded > retained,
            "a decoder cannot assume the original persistent sharing"
        );
        let copied = value.clone();
        // Vec::clone discards the fixture's deliberately reserved spare rows.
        // A decoded body fits within the original producer's captured bound.
        assert!(decoded_tir_bytes(&copied) <= decoded);
        assert!(decoded_tir_bytes(&copied) > retained_tir_bytes(&copied));
    }
}
