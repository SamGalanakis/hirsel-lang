//! PS9 source export from Run's complete settled package semantic closure.
//! Ports proof_export::QueryEngine::proof_export_input's selection/formatting;
//! construction captures source metadata, and Run performs all body demands.
use super::*;
use hirsel_proof_export::{Anchor, Annotation, ExportInput, Header, Location, Locator};
use hirsel_syntax::{AnalysisSite, ItemKind, TypeBody};
use std::rc::Rc;

/// An immutable package export selection, retaining the exact ordinary source
/// producers. A returned ExportInput is data, with no native/V authority.
pub struct FrontProofExport {
    run: Arc<FrontRunQueries>,
    headers: Arc<FrontCheckedSignatures>,
    entry: DefPath,
    roots: BTreeSet<DefPath>,
    executable: BTreeSet<DefPath>,
    annotations: BTreeMap<DefPath, Vec<Annotation>>,
    modules: BTreeMap<FileId, String>,
    files: BTreeMap<DefPath, Vec<FileId>>,
    sources: SourceMap,
    signature_edges: BTreeMap<DefId, Vec<DefId>>,
    input_metadata_bytes: u64,
    _allowance: MemoryReservation,
}
/// A complete input and its decode/clone working admission. Keep this owner
/// alive while its input is borrowed by an export builder. Final export tables
/// and encoded text need their own admission.
pub struct FrontProofInput {
    input: ExportInput,
    _owner: Arc<FrontProofExport>,
    _allowance: MemoryReservation,
}
impl FrontProofInput {
    pub fn input(&self) -> &ExportInput {
        &self.input
    }
}
impl FrontProofExport {
    /// Captured layout metadata plus exactly this completed Run's typed body
    /// and value decode bounds. Does not open a provider or decode any body.
    pub fn input_bytes(&self, product: ProductRef) -> Result<u64, QueryError> {
        if product.address != self.query().address {
            return Err(QueryError::NotImplemented);
        }
        let output = self
            .run
            .outcome(product)
            .ok_or(QueryError::NotImplemented)?;
        let closure = output.semantic.definitions();
        if !self.roots.is_subset(closure) {
            return Err(QueryError::NotImplemented);
        }
        let mut required = self.input_metadata_bytes;
        let mut maps = crate::NativeMetadataWeight::default();
        maps.tree::<DefPath, hirsel_tir::DefinitionRef>(closure.len());
        maps.tree::<DefPath, hirsel_tir::Materialized>(closure.len());
        for path in closure {
            maps.path(path);
            maps.path(path);
            let signature = self
                .headers
                .signature(path)
                .ok_or(QueryError::NotImplemented)?;
            if matches!(signature.kind, SignatureKind::Const { .. }) {
                required = required
                    .checked_add(self.run.lower.settled_constant_clone_bytes(path)?)
                    .ok_or(QueryError::NotImplemented)?;
            }
            if self.executable.contains(path) {
                required = required
                    .checked_add(self.run.lower.settled_definition_decode_bytes(path)?)
                    .ok_or(QueryError::NotImplemented)?;
            }
        }
        required
            .checked_add(maps.bytes())
            .ok_or(QueryError::NotImplemented)
    }
    /// Refuse N-1 before body/provider inspection. The returned owner pins both
    /// the selected source and the caller's physical decode/clone reservation.
    pub fn input_with_allowance(
        self: &Arc<Self>,
        product: ProductRef,
        allowance: MemoryReservation,
    ) -> Result<FrontProofInput, QueryError> {
        if self.input_bytes(product)? > allowance.bytes() {
            return Err(operational(
                "PS9 input decode/clone buffers exceed admission",
            ));
        }
        Ok(FrontProofInput {
            input: self.input(product)?,
            _owner: self.clone(),
            _allowance: allowance,
        })
    }

    /// Register this exact Run view's factory; its root includes every source
    /// definition in the entry package and their ordinary semantic closure.
    pub fn run(&self) -> &Arc<FrontRunQueries> {
        &self.run
    }
    pub fn query(&self) -> QueryId {
        self.run.query(&self.entry).expect("captured export entry")
    }
    /// Export the exact Run behind this completed native source root. The
    /// producer identity check precedes body decoding and never reselects Run.
    pub fn input_for_native(
        &self,
        native: &FrontNativeQueries,
        product: ProductRef,
    ) -> Result<ExportInput, QueryError> {
        let run = native
            .export_run(&self.entry)
            .ok_or(QueryError::NotImplemented)?;
        if !Arc::ptr_eq(&run, &self.run) {
            return Err(QueryError::NotImplemented);
        }
        self.input(
            native
                .completed_run_product(product)
                .ok_or(QueryError::NotImplemented)?,
        )
    }
    /// Complete observed source reads for the selected closure. Exact frozen
    /// header fingerprint coverage is available through headers().
    pub fn reads(&self, product: ProductRef) -> Result<BTreeMap<DefPath, Arc<Reads>>, QueryError> {
        let output = self
            .run
            .outcome(product)
            .filter(|_| product.address == self.query().address)
            .ok_or(QueryError::NotImplemented)?;
        let closure = output.semantic.definitions();
        let mut reads: BTreeMap<DefPath, Arc<Reads>> = BTreeMap::new();
        for (key, observed) in self.run.front.memo_snapshot() {
            let path = key.def.path();
            if closure.contains(path) {
                let previous = reads.entry(path.clone()).or_default();
                *previous = Arc::new(
                    previous
                        .union(&observed)
                        .map_err(|_| QueryError::NotImplemented)?,
                );
            }
        }
        for path in closure {
            for phase in [QueryKind::Lower, QueryKind::Evaluate] {
                if let Some(observed) = self.run.lower.settled_reads(path, phase) {
                    let previous = reads.entry(path.clone()).or_default();
                    *previous = Arc::new(
                        previous
                            .union(&observed)
                            .map_err(|_| QueryError::NotImplemented)?,
                    );
                }
            }
        }
        Ok(reads)
    }
    pub fn headers(&self) -> &Arc<FrontCheckedSignatures> {
        &self.headers
    }
    /// Format only the exact completed package selection. Missing signatures,
    /// TIR or values refuse instead of silently making a partial export.
    pub fn input(&self, product: ProductRef) -> Result<ExportInput, QueryError> {
        if product.address != self.query().address {
            return Err(QueryError::NotImplemented);
        }
        let output = self
            .run
            .outcome(product)
            .ok_or(QueryError::NotImplemented)?;
        let closure = output.semantic.definitions();
        if !self.roots.is_subset(closure) {
            return Err(QueryError::NotImplemented);
        }
        let mut signatures = BTreeMap::new();
        let mut definitions = BTreeMap::new();
        let mut constants = BTreeMap::new();
        let mut annotations = Vec::new();
        for path in closure {
            let section = self
                .headers
                .section(path)
                .ok_or(QueryError::NotImplemented)?;
            if matches!(section.public.kind, SignatureKind::Const { .. }) {
                constants.insert(
                    path.clone(),
                    (*self.run.lower.settled_constant(path, &[])?).clone(),
                );
            }
            signatures.insert(path.clone(), section);
            if self.executable.contains(path) {
                definitions.insert(path.clone(), self.run.lower.settled_definition(path)?);
            }
            if let Some(rows) = self.annotations.get(path) {
                annotations.extend(rows.iter().cloned());
            }
        }
        // The fingerprint closure also reaches declarations named only by
        // public/private type views and registry operation signatures.
        let types = self.run.lower.signatures().types().clone();
        let mut pending = Vec::new();
        for path in signatures.keys() {
            pending.extend(
                self.signature_edges
                    .get(&DefId::intern(path))
                    .ok_or(QueryError::NotImplemented)?
                    .iter()
                    .copied(),
            );
        }
        while let Some(id) = pending.pop() {
            let path = id.path();
            if signatures.contains_key(path) {
                continue;
            }
            let section = self
                .headers
                .section(path)
                .ok_or(QueryError::NotImplemented)?;
            pending.extend(
                self.signature_edges
                    .get(&id)
                    .ok_or(QueryError::NotImplemented)?
                    .iter()
                    .copied(),
            );
            signatures.insert(path.clone(), section);
        }
        let sources = Rc::new(self.sources.clone());
        let selected_files: BTreeSet<_> = closure
            .iter()
            .filter_map(|path| self.files.get(path))
            .flat_map(|files| files.iter().copied())
            .collect();
        let modules = self
            .modules
            .iter()
            .filter(|(file, _)| selected_files.contains(file))
            .map(|(file, module)| (*file, module.clone()))
            .collect();
        let locator: Locator = Rc::new(move |span| location(&sources, &modules, span));
        Ok(ExportInput {
            types,
            header: Header {
                format: hirsel_proof_export::FORMAT.into(),
                tir_version: hirsel_tir::TIR_VERSION,
                environment: self.headers.environment().clone(),
                entry: self.entry.clone(),
                program_revision: Some(ContentHash(output.revision.digest())),
            },
            definitions,
            signatures,
            constants,
            skipped: vec![],
            annotations,
            locator,
        })
    }
}
fn location(sources: &SourceMap, modules: &BTreeMap<FileId, String>, span: Span) -> Location {
    let span = sources.resolve(span);
    let file = sources.file(span.file);
    match (
        modules.get(&span.file),
        file.and_then(|f| f.location(span.start)),
        file.and_then(|f| f.location(span.end)),
    ) {
        (Some(module), Some(start), Some(end)) => Location {
            module: module.clone(),
            line: start.line,
            column: start.column,
            end_line: end.line,
            end_column: end.column,
        },
        _ => Location::unknown(),
    }
}
impl QueryEngine {
    /// Capture PS9 metadata and select an unshared ordinary Run view. No
    /// check, Lower, Evaluate or semantic query is executed during capture.
    pub fn front_proof_export(
        &mut self,
        entry: &DefPath,
        run: Arc<FrontRunQueries>,
        headers: Arc<FrontCheckedSignatures>,
        allowance: MemoryReservation,
    ) -> Result<Arc<FrontProofExport>, QueryError> {
        let roots: BTreeSet<_> = self
            .item_paths()
            .into_iter()
            .filter(|p| p.0.package == entry.0.package)
            .collect();
        let run = run.with_source_roots(roots.iter().cloned().collect())?;
        self.capture_proof_export(entry, run, headers, roots, allowance)
    }
    /// Attach to native's already selected Run. Its selected roots must cover
    /// every PS9 entry-package obligation; a subset remains a refusal.
    pub fn front_proof_export_from_native(
        &mut self,
        entry: &DefPath,
        native: &Arc<FrontNativeQueries>,
        headers: Arc<FrontCheckedSignatures>,
        allowance: MemoryReservation,
    ) -> Result<Arc<FrontProofExport>, QueryError> {
        let run = native.export_run(entry).ok_or(QueryError::NotImplemented)?;
        let roots: BTreeSet<_> = self
            .item_paths()
            .into_iter()
            .filter(|p| p.0.package == entry.0.package)
            .collect();
        if !roots.iter().all(|path| run.native_roots.contains(path)) {
            return Err(QueryError::NotImplemented);
        }
        self.capture_proof_export(entry, run, headers, roots, allowance)
    }
    fn capture_proof_export(
        &mut self,
        entry: &DefPath,
        run: Arc<FrontRunQueries>,
        headers: Arc<FrontCheckedSignatures>,
        roots: BTreeSet<DefPath>,
        allowance: MemoryReservation,
    ) -> Result<Arc<FrontProofExport>, QueryError> {
        if !headers.matches(&run.front)
            || !Arc::ptr_eq(&run.lower.signatures(), &self.signature_inputs())
            || run.query(entry).is_none()
        {
            return Err(QueryError::NotImplemented);
        }
        let mut executable = BTreeSet::new();
        let mut annotations = BTreeMap::new();
        let mut modules = BTreeMap::new();
        let mut files = BTreeMap::new();
        for path in self.source_paths() {
            self.materialize(&path)?;
            let Ok(items) = self.definition_items(&path) else {
                continue;
            };
            for syntax in items {
                let mut module = format!("{}@{}", path.0.package.name, path.0.package.version);
                for part in &path.0.modules {
                    module.push('/');
                    module.push_str(part);
                }
                let file = self.item_file(&syntax);
                modules.insert(file, module);
                files
                    .entry(path.clone())
                    .or_insert_with(Vec::new)
                    .push(file);
                let own = hirsel_check_protocol::fact::fact_owner(&path).is_none();
                let rows = annotations.entry(path.clone()).or_insert_with(Vec::new);
                for a in syntax.tree.analyses.iter().filter(|_| own) {
                    let anchor = match a.site {
                        AnalysisSite::Item => Anchor::Item,
                        AnalysisSite::Parameter(i) => Anchor::Parameter(i),
                        AnalysisSite::Field(i) => Anchor::Field(i),
                        AnalysisSite::Payload { variant, field } => {
                            Anchor::Payload { variant, field }
                        }
                    };
                    rows.push((anchor, a.clone()));
                }
                match &syntax.item.kind {
                    _ if generation::executable(&syntax) => {
                        executable.insert(path.clone());
                    }
                    ItemKind::Type {
                        body: TypeBody::Distinct { fact: Some(_), .. },
                        ..
                    } => {
                        executable.insert(hirsel_check_protocol::fact::fact_path(&path));
                    }
                    _ => {}
                }
            }
        }
        let sources = self.sources.clone();
        let annotations: BTreeMap<_, Vec<_>> = annotations
            .into_iter()
            .map(|(path, rows)| {
                let rows = rows
                    .into_iter()
                    .map(|(anchor, a)| Annotation {
                        path: path.clone(),
                        anchor,
                        name: a.name,
                        payload: a.payload,
                        location: location(&sources, &modules, a.origin.span),
                    })
                    .collect();
                (path, rows)
            })
            .collect();
        // Freeze signature-only closure edges while S0 owns normalization.
        // DefIds retain no copied path payload; later input sizing/formatting
        // never traverses or reconstructs the signature's type graph.
        let types = run.lower.signatures().types().clone();
        let mut signature_edges = BTreeMap::new();
        for (path, _) in headers.signatures() {
            let section = headers.section(path).ok_or(QueryError::NotImplemented)?;
            let edges = crate::proof_export::signature_references(&types, &section)
                .into_iter()
                .map(|path| DefId::intern(&path))
                .collect::<Vec<_>>();
            signature_edges.insert(DefId::intern(path), edges);
        }
        let mut input_weight = crate::NativeMetadataWeight::default();
        input_weight
            .add(size_of::<FrontProofInput>() + size_of::<SourceMap>() + 8 * size_of::<usize>());
        input_weight.add(hirsel_proof_export::FORMAT.len());
        input_weight.add(headers.environment().compiler_version.capacity());
        input_weight.path(entry);
        input_weight.source_snapshot(&sources);
        input_weight.tree::<FileId, ()>(modules.len());
        input_weight.map(&modules, |w, _, module| w.add(module.capacity()));
        let annotation_count: usize = annotations.values().map(Vec::len).sum();
        input_weight.add(
            annotation_count
                .saturating_mul(2)
                .max(4)
                .saturating_mul(size_of::<Annotation>()),
        );
        for rows in annotations.values() {
            for a in rows {
                input_weight.path(&a.path);
                input_weight.add(a.name.capacity());
                input_weight.add(a.location.module.capacity());
                input_weight.vec(&a.payload, |w, token| w.add(token.capacity()));
            }
        }
        let edge_count: usize = signature_edges.values().map(Vec::len).sum();
        input_weight.add(
            edge_count
                .saturating_mul(2)
                .max(4)
                .saturating_mul(size_of::<DefId>()),
        );
        let input_metadata_bytes = input_weight
            .bytes()
            .checked_add(
                headers
                    .sections_clone_bytes()
                    .ok_or(QueryError::NotImplemented)?,
            )
            .ok_or(QueryError::NotImplemented)?;
        let mut weight = crate::NativeMetadataWeight::default();
        weight.map(&signature_edges, |w, _, edges| w.flat_vec(edges));
        weight.add(size_of::<FrontProofExport>() + 2 * size_of::<usize>());
        weight.path(entry);
        weight.source_snapshot(&sources);
        weight.tree::<DefPath, ()>(roots.len() + executable.len());
        for path in roots.iter().chain(&executable) {
            weight.path(path);
        }
        weight.map(&modules, |w, _, module| w.add(module.capacity()));
        weight.map(&files, |w, path, files| {
            w.path(path);
            w.flat_vec(files);
        });
        weight.map(&annotations, |w, path, rows| {
            w.path(path);
            w.vec(rows, |w, a| {
                w.path(&a.path);
                w.add(a.name.capacity());
                w.add(a.location.module.capacity());
                w.vec(&a.payload, |w, token| w.add(token.capacity()));
            });
        });
        if weight.bytes() > allowance.bytes() {
            return Err(operational("PS9 source metadata exceeds admission"));
        }
        Ok(Arc::new(FrontProofExport {
            run,
            headers,
            entry: entry.clone(),
            roots,
            executable,
            annotations,
            modules,
            files,
            sources,
            signature_edges,
            input_metadata_bytes,
            _allowance: allowance,
        }))
    }
}
