//! Re-span reused item owners without changing their arena identities or query keys.
use super::*;
use hirsel_span::Span;
use hirsel_syntax::TypeBody;

pub(super) struct Movement {
    pub old: Span,
    pub new_file: FileId,
    pub delta: i64,
    pub whole: (Span, Span),
}
struct Rebaser<'a> {
    moves: BTreeMap<FileId, Vec<&'a Movement>>,
    owners: BTreeMap<usize, Arc<SyntaxFile>>,
}
pub(crate) fn rebase_package_diagnostics(
    ds: &[Diagnostic],
    moves: &[(FileId, FileId, u32)],
) -> Vec<Diagnostic> {
    let movements: Vec<_> = moves
        .iter()
        .map(|&(old, new, len)| {
            let before = Span::new(old, 0, len).unwrap();
            Movement {
                old: before,
                new_file: new,
                delta: 0,
                whole: (before, Span::new(new, 0, len).unwrap()),
            }
        })
        .collect();
    let r = Rebaser {
        moves: movements.iter().map(|m| (m.old.file, vec![m])).collect(),
        owners: BTreeMap::new(),
    };
    let mut result = ds.to_vec();
    for d in &mut result {
        r.diagnostic(d);
    }
    result
}
impl Rebaser<'_> {
    fn attributes(&self, attributes: &mut [hirsel_syntax::Attribute]) {
        for attribute in attributes {
            self.span(&mut attribute.origin.span);
            for argument in &mut attribute.arguments {
                match argument {
                    hirsel_syntax::AttributeArgument::LoopSource(s) => {
                        self.span(&mut s.origin.span)
                    }
                    hirsel_syntax::AttributeArgument::StatePromise(s) => {
                        self.span(&mut s.origin.span)
                    }
                    _ => (),
                }
            }
        }
    }
    fn span(&self, span: &mut Span) {
        let Some(moves) = self.moves.get(&span.file) else {
            return;
        };
        let end = moves.partition_point(|m| m.old.start <= span.start);
        let Some(m) = end.checked_sub(1).and_then(|i| moves.get(i)) else {
            return;
        };
        if *span == m.whole.0 {
            *span = m.whole.1;
            return;
        }
        if span.end <= m.old.end {
            span.file = m.new_file;
            span.start =
                u32::try_from(i64::from(span.start) + m.delta).expect("reused source offset");
            span.end = u32::try_from(i64::from(span.end) + m.delta).expect("reused source offset");
        }
    }
    fn tir_span(&self, span: &mut Span) {
        // Zero is the existing sentinel for synthetic storage/control sites.
        if *span != Span::default() {
            self.span(span);
        }
    }
    fn diagnostic(&self, d: &mut Diagnostic) {
        self.span(&mut d.primary);
        for obligation in &mut d.obligations {
            match obligation {
                hirsel_diag::Obligation::GenericBound { arguments } => {
                    for span in arguments {
                        self.span(span);
                    }
                }
                hirsel_diag::Obligation::ParameterMode { argument } => self.span(argument),
                _ => (),
            }
        }
        for label in &mut d.secondary {
            self.span(&mut label.span);
        }
        if let Some(fix) = &mut d.fix {
            self.span(&mut fix.span);
        }
    }
    fn error(&self, error: &mut QueryError) {
        if let QueryError::Diagnostics(ds) = error {
            for d in ds {
                self.diagnostic(d);
            }
        }
    }
    fn read(&self, read: &mut hirsel_types::DependencyRead) {
        use hirsel_types::DependencyRead::*;
        match read {
            ProviderLookup { output, .. } => {
                if let Err(e) = &mut output.result {
                    self.error(e);
                }
            }
            Lookup {
                result: Err(hirsel_types::LookupError::Provider(ds)),
                ..
            } => {
                for d in ds {
                    self.diagnostic(d);
                }
            }
            _ => {}
        }
    }
    fn resolution(&self, resolution: &mut Resolution) {
        for deferred in &mut resolution.deferred {
            self.span(&mut deferred.span);
        }
        for read in &mut resolution.reads {
            self.read(read);
        }
    }
    fn file(&mut self, owner: &mut Arc<SyntaxFile>) {
        let address = Arc::as_ptr(owner) as usize;
        if let Some(file) = self.owners.get(&address) {
            *owner = file.clone();
            return;
        }
        let Some(m) = self.moves.get(&owner.file).and_then(|moves| moves.first()) else {
            return;
        };
        let mut file = (**owner).clone();
        file.file = m.new_file;
        for token in &mut file.tokens {
            self.span(&mut token.span);
        }
        for item in &mut file.tree.items {
            self.span(&mut item.origin.span);
            self.span(&mut item.name_span);
            self.attributes(&mut item.attributes);
            if let ItemKind::Function { signature, .. } = &mut item.kind {
                self.attributes(&mut signature.attributes);
            }
            if let ItemKind::Type { body, .. } = &mut item.kind {
                match body {
                    TypeBody::Record(fields) => {
                        for field in fields {
                            self.span(&mut field.origin.span);
                        }
                    }
                    TypeBody::Distinct { field, .. } => self.span(&mut field.origin.span),
                    _ => {}
                }
            }
        }
        for node in &mut file.tree.expressions {
            self.span(&mut node.origin.span);
            if let hirsel_syntax::ExprKind::Closure { signature, .. } = &mut node.kind {
                self.attributes(&mut signature.attributes);
            }
        }
        for node in &mut file.tree.statements {
            self.span(&mut node.origin.span);
            if let hirsel_syntax::StmtKind::Function { signature, .. } = &mut node.kind {
                self.attributes(&mut signature.attributes);
            }
        }
        for node in &mut file.tree.patterns {
            self.span(&mut node.origin.span);
        }
        for node in &mut file.tree.types {
            self.span(&mut node.origin.span);
            if let hirsel_syntax::TypeExprKind::Function(signature) = &mut node.kind {
                self.attributes(&mut signature.attributes);
            }
        }
        let file = Arc::new(file);
        self.owners.insert(address, file.clone());
        *owner = file;
    }
    fn cached<T: Clone>(&mut self, cached: &mut Cached<T>, value: impl FnOnce(&mut Self, &mut T)) {
        for read in &mut cached.reads.manifest_lookups {
            self.read(read);
        }
        for read in &mut cached.reads.resolver {
            self.read(read);
        }
        match &mut cached.result {
            Ok(output) => {
                let output = Arc::make_mut(output);
                for d in &mut output.diagnostics {
                    self.diagnostic(d);
                }
                value(self, &mut output.value);
            }
            Err(e) => self.error(e),
        }
    }
}
impl QueryEngine {
    pub(super) fn rebase_positions(&mut self, moves: &[Movement]) {
        if moves.is_empty() {
            return;
        }
        let mut grouped: BTreeMap<FileId, Vec<&Movement>> = BTreeMap::new();
        for m in moves {
            grouped.entry(m.old.file).or_default().push(m);
        }
        for group in grouped.values_mut() {
            group.sort_by_key(|m| (m.old.start, m.old.end));
        }
        let mut r = Rebaser {
            moves: grouped,
            owners: BTreeMap::new(),
        };
        for snapshots in self.items.values_mut() {
            for s in snapshots {
                r.file(&mut s.file);
            }
        }
        for c in self.signatures.values_mut() {
            r.cached(Arc::make_mut(c), |_, _| {});
        }
        for c in self.resolutions.values_mut() {
            r.cached(Arc::make_mut(c), |r, v| r.resolution(v));
        }
        for c in self.checked.values_mut() {
            r.cached(Arc::make_mut(c), |r, v| {
                r.file(&mut v.file);
                r.resolution(&mut v.checked.resolution);
            });
        }
        for c in self.modes.values_mut() {
            r.cached(Arc::make_mut(c), |r, v| {
                r.file(&mut v.typed.file);
                r.resolution(&mut v.typed.checked.resolution);
            });
        }
        for c in self.lowered.values_mut() {
            r.cached(Arc::make_mut(c), |r, v| {
                for body in &mut std::sync::Arc::make_mut(v).bodies {
                    for local in &mut body.locals {
                        r.tir_span(&mut local.span);
                    }
                    for scope in &mut body.scopes {
                        r.tir_span(&mut scope.span);
                    }
                    for block in &mut body.blocks {
                        r.tir_span(&mut block.span);
                        for instruction in &mut block.instructions {
                            r.tir_span(&mut instruction.span);
                        }
                    }
                }
            });
        }
        for reads in self.reads.values_mut() {
            for read in &mut reads.manifest_lookups {
                r.read(read);
            }
            for read in &mut reads.resolver {
                r.read(read);
            }
        }
        if let Some(limit) = &mut self.last_comptime_limit {
            if let Some(site) = &mut limit.site {
                r.span(site);
            }
        }
        if let Some(state) = &self.adapters.compiler_state {
            let mut state = state.borrow_mut();
            for (d, _) in &mut state.mode_diagnostics {
                r.diagnostic(d);
            }
            for site in &mut state.signature_metadata.declarations {
                r.span(&mut site.name_span);
            }
            for s in state.resolver_outputs.values_mut() {
                r.file(&mut s.file);
                r.resolution(&mut s.output.value);
                for d in &mut s.output.diagnostics {
                    r.diagnostic(d);
                }
            }
            for s in state.signature_resolutions.values_mut() {
                r.file(&mut s.file);
                r.resolution(&mut s.output.value);
                for d in &mut s.output.diagnostics {
                    r.diagnostic(d);
                }
            }
        }
    }
}
