//! Retained allocation accounting, adapted from Buck2 allocative's Arc and Vec
//! visitors (third-party/NOTICES.md). This measures storage, never wire bytes.
use super::{DepthEvidence, Reads};
use crate::comptime::StagedRegistration;
use hirsel_syntax::DefPath;
use hirsel_tir::*;
use hirsel_types::{Destination, Origin, OriginRoot, SharedPath, SharedSlice};
use std::{alloc::Layout, collections::HashSet};

/// Bytes retained by a DefinitionRef, including its Arc allocation. Session
/// interner rows/DefIds and shared syntax belong to the session, not this hold.
/// Shared allocations are charged once within this artifact, conservatively
/// again if another hold also retains them. No allocator-specific size classes.
pub(crate) fn retained_tir_bytes(definition: &Definition) -> u64 {
    let mut weight = Weight::default();
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
    weight.add(
        reads
            .len()
            .saturating_mul(size_of::<(super::ReadKey, hirsel_syntax::ContentHash)>()),
    );
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
    weight.vec(dependencies, |w, dependency| {
        w.def_path(&dependency.definition)
    });
    weight.vec(diagnostics, |w, diagnostic| {
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
        w.add(rule.retained_bytes());
        w.add(message.capacity());
        w.vec(secondary, |w, label| w.add(label.message.capacity()));
        w.vec(notes, |w, note| w.add(note.capacity()));
        if let Some(fix) = fix {
            w.add(fix.replacement.capacity());
            w.add(fix.message.capacity());
        }
        w.vec(obligations, |w, obligation| match obligation {
            hirsel_diag::Obligation::GenericBound { arguments } => w.flat_vec(arguments),
            hirsel_diag::Obligation::Multiplicity
            | hirsel_diag::Obligation::CopyProof
            | hirsel_diag::Obligation::ParameterMode { .. } => (),
        });
    });
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
struct Weight {
    bytes: u64,
    shared: HashSet<usize>,
}
impl Weight {
    fn add(&mut self, bytes: usize) {
        self.bytes = self.bytes.saturating_add(bytes as u64);
    }
    fn flat_vec<T>(&mut self, value: &Vec<T>) {
        self.add(value.capacity().saturating_mul(size_of::<T>()));
    }
    fn vec<T>(&mut self, value: &Vec<T>, mut visit: impl FnMut(&mut Self, &T)) {
        self.flat_vec(value);
        for element in value {
            visit(self, element);
        }
    }
    fn arc_slice<T>(&mut self, value: &std::sync::Arc<[T]>) {
        if self.shared.insert(value.as_ptr() as usize) {
            self.add(arc_bytes(&**value));
        }
    }
    fn shared_slice<T>(&mut self, value: &SharedSlice<T>) {
        let identity = value.storage_identity();
        if identity != 0 && self.shared.insert(identity) {
            self.add(arc_bytes(value.as_slice()));
        }
    }
    fn shared_values<T>(&mut self, value: &SharedSlice<T>, mut visit: impl FnMut(&mut Self, &T)) {
        let identity = value.storage_identity();
        if identity != 0 && self.shared.insert(identity) {
            self.add(arc_bytes(value.as_slice()));
            for element in value {
                visit(self, element);
            }
        }
    }
    fn generic(&mut self, value: &hirsel_types::GenericParameter) {
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
    fn signature(&mut self, value: &hirsel_types::SignatureRef) {
        if !self.shared.insert(std::sync::Arc::as_ptr(value) as usize) {
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
            track_caller: _,
            unsafe_ffi: _,
            wrapper: _,
            roles,
            schema,
            recursive_group: _,
        } = metadata;
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
            hirsel_types::SignatureKind::Type(definition) => {
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
                if let Some(hirsel_types::LoopSourceDeclaration { open, next, option }) =
                    loop_source
                {
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
        }
    }
    fn path<T>(&mut self, mut value: &SharedPath<T>) {
        while let Some(identity) = value.node_identity() {
            if !self.shared.insert(identity) {
                break;
            }
            self.add(value.node_allocation_bytes());
            value = value.prefix().expect("nonempty paths have a prefix");
        }
    }
    fn def_path(&mut self, value: &DefPath) {
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
    fn constant(&mut self, value: &Constant) {
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
    fn constructor(&mut self, value: &Constructor) {
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
            | InstructionKind::Snapshot { .. } => (),
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
            matches,
        } = body;
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
            definition: DefPath::core(&"helper".repeat(1000)),
            fingerprint: Default::default(),
        });
        assert!(retained_lowering_output_bytes(&diagnostics, &dependencies) >= warned + 6000);
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
}
