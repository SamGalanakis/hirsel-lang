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
/// The exact selected type closure and its admitted inspection workspace.
/// This owns no decoded output beyond the ordinary complete input.
pub struct FrontPreparedProof {
    input: Rc<dyn AdmittedProofInput>,
    types: hirsel_proof_export::CapturedTypes,
    _allowance: MemoryReservation,
}
/// A complete canonical export with construction working admission retained.
/// It borrows no mutable compiler and cannot request another source product.
pub struct FrontBuiltProof {
    export: hirsel_proof_export::Export,
    input: BuiltProofInput,
    _allowance: MemoryReservation,
}
enum BuiltProofInput {
    Borrowed(Rc<FrontPreparedProof>),
    Owned {
        locator: Locator,
        source: OwnedProofSource,
        _input_allowance: MemoryReservation,
        _types: hirsel_proof_export::CapturedTypes,
        _capture_allowance: MemoryReservation,
    },
}
impl BuiltProofInput {
    fn locator(&self) -> &Locator {
        match self {
            Self::Borrowed(input) => &input.input.export_input().locator,
            Self::Owned { locator, .. } => locator,
        }
    }
    fn checked_receipt(&self) -> Option<(&FrontCheckScope, &FrontCheckedSignatures)> {
        match self {
            Self::Borrowed(input) => input.input.checked_receipt(),
            Self::Owned { source, .. } => source
                .checked
                .as_ref()
                .map(|(scope, headers)| (&**scope, &**headers)),
        }
    }
}
/// Portable canonical bytes keep only their actual output allocation charged,
/// including through detached clones. Neither source nor native authority is
/// supplied by these bytes.
#[derive(Clone)]
pub struct FrontProofText {
    text: Arc<str>,
    owner: Arc<MemoryReservation>,
}
impl FrontProofText {
    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn retained_bytes(&self) -> u64 {
        self.owner.bytes()
    }
    pub fn into_parts(self) -> (Arc<str>, Arc<dyn Send + Sync>) {
        (self.text, self.owner)
    }
}
fn arc_layout<T>() -> Option<usize> {
    Some(
        std::alloc::Layout::new::<[usize; 2]>()
            .extend(std::alloc::Layout::new::<T>())
            .ok()?
            .0
            .pad_to_align()
            .size(),
    )
}
impl FrontProofInput {
    pub fn input(&self) -> &ExportInput {
        &self.input
    }
    /// Metadata-only workspace size for the selected type closure.
    pub fn type_capture_bytes(&self) -> Result<u64, QueryError> {
        FrontPreparedProof::capture_bytes(self)
    }
    pub fn capture_types_with_allowance(
        self: Rc<Self>,
        allowance: MemoryReservation,
    ) -> Result<FrontPreparedProof, QueryError> {
        FrontPreparedProof::capture(self, allowance)
    }
}
impl AdmittedProofInput for FrontProofInput {
    fn export_input(&self) -> &ExportInput {
        &self.input
    }
    fn export_module_bytes(&self) -> usize {
        self._owner
            .modules
            .values()
            .map(String::len)
            .max()
            .unwrap_or(0)
    }
    fn export_clone_bytes(&self) -> u64 {
        self._allowance.bytes()
    }
    fn into_owned(self: Rc<Self>) -> Result<OwnedProofInput, QueryError> {
        let this = Rc::try_unwrap(self).map_err(|_| {
            operational("PS9 input must be uniquely owned before moving its payloads")
        })?;
        Ok(OwnedProofInput {
            input: this.input,
            source: OwnedProofSource {
                _proof: Some(this._owner),
                checked: None,
            },
            allowance: this._allowance,
        })
    }
}
impl FrontPreparedProof {
    pub(crate) fn capture_bytes(input: &dyn AdmittedProofInput) -> Result<u64, QueryError> {
        hirsel_proof_export::capture_types_bytes(input.export_input())
            .and_then(|n| {
                n.checked_add(
                    size_of::<FrontPreparedProof>() as u64 + 2 * size_of::<usize>() as u64,
                )
            })
            .ok_or(QueryError::NotImplemented)
    }
    pub(crate) fn capture(
        input: Rc<dyn AdmittedProofInput>,
        allowance: MemoryReservation,
    ) -> Result<Self, QueryError> {
        if Self::capture_bytes(&*input)? > allowance.bytes() {
            return Err(operational(
                "canonical type closure workspace exceeds admission",
            ));
        }
        let types = hirsel_proof_export::capture_types(input.export_input(), allowance.bytes())
            .ok_or(QueryError::NotImplemented)?;
        Ok(Self {
            input,
            types,
            _allowance: allowance,
        })
    }
    /// Table-free inspection of the actual canonical fields. Source producers
    /// have already materialized every type their headers/originals name.
    pub fn build_bytes(&self) -> Result<u64, QueryError> {
        let module = self.input.export_module_bytes();
        let tables = hirsel_proof_export::build_captured_tables_bytes(
            self.input.export_input(),
            module,
            &self.types,
        )
        .ok_or(QueryError::NotImplemented)?;
        // The input reservation covers all prospective owned input payloads;
        // cloning them for Export needs one additional independently held copy.
        tables
            .checked_add(self.input.export_clone_bytes())
            .and_then(|n| n.checked_add(size_of::<FrontBuiltProof>() as u64))
            .ok_or(QueryError::NotImplemented)
    }
    pub fn build_with_allowance(
        self: Rc<Self>,
        allowance: MemoryReservation,
    ) -> Result<FrontBuiltProof, QueryError> {
        if self.build_bytes()? > allowance.bytes() {
            return Err(operational("PS9 construction buffers exceed admission"));
        }
        let export = hirsel_proof_export::build(self.input.export_input());
        if !export.skipped.is_empty() {
            return Err(operational(
                "PS9 construction refused an incomplete definition section",
            ));
        }
        Ok(FrontBuiltProof {
            export,
            input: BuiltProofInput::Borrowed(self),
            _allowance: allowance,
        })
    }
    /// Construction tables only: unique input payloads move under their
    /// existing reservation instead of requiring a second payload grant.
    pub fn owned_build_bytes(&self) -> Result<u64, QueryError> {
        self.owned_build_layout()?
            .total()
            .ok_or(QueryError::NotImplemented)
    }
    /// Inspect the actual retained sections and independent construction
    /// workspaces without decoding originals or querying a provider.
    pub fn owned_build_layout(&self) -> Result<hirsel_proof_export::BuildTablesLayout, QueryError> {
        let mut layout = hirsel_proof_export::build_owned_tables_layout(
            self.input.export_input(),
            self.input.export_module_bytes(),
            &self.types,
        )
        .ok_or(QueryError::NotImplemented)?;
        layout.retained_sections = layout
            .retained_sections
            .checked_add(size_of::<FrontBuiltProof>() as u64)
            .ok_or(QueryError::NotImplemented)?;
        Ok(layout)
    }
    /// Consume this preparation and its sole input reference. Shared inputs
    /// refuse before export construction; successful exports retain all source
    /// and input owners until their last inspection/encoding consumer drops.
    pub fn build_owned_with_allowance(
        self,
        allowance: MemoryReservation,
    ) -> Result<FrontBuiltProof, QueryError> {
        if self.owned_build_bytes()? > allowance.bytes() {
            return Err(operational(
                "owned PS9 construction buffers exceed admission",
            ));
        }
        let owned = self.input.into_owned()?;
        let locator = owned.input.locator.clone();
        let export = hirsel_proof_export::build_owned(owned.input)
            .ok_or_else(|| operational("original body payloads must be uniquely owned"))?;
        if !export.skipped.is_empty() {
            return Err(operational(
                "PS9 construction refused an incomplete definition section",
            ));
        }
        Ok(FrontBuiltProof {
            export,
            input: BuiltProofInput::Owned {
                locator,
                source: owned.source,
                _input_allowance: owned.allowance,
                _types: self.types,
                _capture_allowance: self._allowance,
            },
            _allowance: allowance,
        })
    }
}
impl FrontBuiltProof {
    /// Complete checked-product read maps and digest scratch, independent of
    /// the canonical text output allowance. PS9-only inputs cannot mint this.
    pub fn checked_products_bytes(&self) -> Result<u64, QueryError> {
        let (scope, headers) = self
            .input
            .checked_receipt()
            .ok_or(QueryError::NotImplemented)?;
        scope.receipt_finish_bytes(headers, self.encoded_bytes()?)
    }
    pub fn checked_products_with_allowances(
        &self,
        output: MemoryReservation,
        mut receipt: MemoryReservation,
    ) -> Result<Arc<CheckedProducts>, QueryError> {
        let needed = self.checked_products_bytes()?;
        if needed > receipt.bytes() {
            return Err(operational(
                "checked receipt read/digest buffers exceed admission",
            ));
        }
        let (scope, headers) = self
            .input
            .checked_receipt()
            .ok_or(QueryError::NotImplemented)?;
        let encoded = self.encode_with_allowance(output)?;
        let (canonical, output_owner) = encoded.into_parts();
        let products = CheckedProductEncoder::from_canonical(canonical, self.export.types.clone())
            .finish(scope.reads().clone(), headers.registrations().to_vec());
        let retained = products
            .receipt_retained_bytes()
            .ok_or(QueryError::NotImplemented)?;
        if retained > receipt.bytes() {
            return Err(operational(
                "checked receipt storage exceeds its construction bound",
            ));
        }
        let owner = Arc::new(receipt.split(retained).ok_or(QueryError::NotImplemented)?);
        Ok(Arc::new(
            products.retain_owner(output_owner).retain_owner(owner),
        ))
    }

    pub fn export(&self) -> &hirsel_proof_export::Export {
        &self.export
    }
    pub fn encoded_bytes(&self) -> Result<usize, QueryError> {
        hirsel_proof_export::encoded_bytes(&self.export, self.input.locator())
            .ok_or(QueryError::NotImplemented)
    }
    /// One final exact-length Arc; emitter context/digest framing scratch is
    /// covered by the separately retained construction workspace.
    pub fn output_bytes(&self) -> Result<u64, QueryError> {
        let bytes = self.encoded_bytes()?;
        let metadata = 2 * size_of::<usize>()
            + arc_layout::<MemoryReservation>().ok_or(QueryError::NotImplemented)?
            + size_of::<FrontProofText>();
        bytes
            .checked_add(metadata)
            .and_then(|n| n.try_into().ok())
            .ok_or(QueryError::NotImplemented)
    }
    pub fn encode_with_allowance(
        &self,
        mut allowance: MemoryReservation,
    ) -> Result<FrontProofText, QueryError> {
        let needed = self.output_bytes()?;
        if needed > allowance.bytes() {
            return Err(operational("PS9 canonical output buffers exceed admission"));
        }
        let length = self.encoded_bytes()?;
        let canonical = hirsel_proof_export::encode_arc_with_output_limit(
            &self.export,
            self.input.locator(),
            length,
        )
        .ok_or(QueryError::NotImplemented)?;
        let retained = needed;
        let owner = Arc::new(
            allowance
                .split(retained)
                .ok_or(QueryError::NotImplemented)?,
        );
        Ok(FrontProofText {
            text: canonical,
            owner,
        })
    }
}
impl FrontProofExport {
    /// Captured layout metadata plus exactly this completed Run's typed body
    /// and value decode bounds. Does not open a provider or decode any body.
    pub fn input_bytes(&self, product: ProductRef) -> Result<u64, QueryError> {
        self.input_layout(product)?
            .total()
            .ok_or(QueryError::NotImplemented)
    }
    fn input_layout(&self, product: ProductRef) -> Result<DefinitionDecodeLayout, QueryError> {
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
        let mut transient = 0u64;
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
                let layout = self.run.lower.settled_definition_decode_layout(path)?;
                required = required
                    .checked_add(layout.retained)
                    .ok_or(QueryError::NotImplemented)?;
                transient = transient.max(layout.transient);
            }
        }
        Ok(DefinitionDecodeLayout {
            retained: required
                .checked_add(maps.bytes())
                .ok_or(QueryError::NotImplemented)?,
            transient,
        })
    }
    /// Refuse N-1 before body/provider inspection. The returned owner pins both
    /// the selected source and the caller's physical decode/clone reservation.
    pub fn input_with_allowance(
        self: &Arc<Self>,
        product: ProductRef,
        mut allowance: MemoryReservation,
    ) -> Result<FrontProofInput, QueryError> {
        let layout = self.input_layout(product.clone())?;
        if layout.total().ok_or(QueryError::NotImplemented)? > allowance.bytes() {
            return Err(operational(
                "PS9 input decode/clone buffers exceed admission",
            ));
        }
        Ok(FrontProofInput {
            input: self.input(product)?,
            _owner: self.clone(),
            _allowance: allowance
                .split(layout.retained)
                .ok_or(QueryError::NotImplemented)?,
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
            .outcome(product.clone())
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
