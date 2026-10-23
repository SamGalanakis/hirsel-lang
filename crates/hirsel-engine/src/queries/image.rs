//! The engine's side of the std image (`crate::image`): writing one from a
//! checked package, adopting one, and parsing an image package's sources only
//! when something needs their syntax.
//!
//! An adopting engine parses none of the package's sources. Its declaration
//! rows, signatures and the inputs their readers observe come from the image,
//! and so do the records of the products a program runs with. A query that
//! needs a definition's syntax (checking or lowering it from source, a
//! diagnostic at it) parses the sources its items stand in, with each item at
//! the image's anchor for its place, so every span the image holds and every
//! span the parse makes name the same anchors. Indexing a source of the
//! package again (an edit) parses every source and ends the image.
use super::*;
use crate::image::*;
use hirsel_syntax::PackageId;
use std::io::Write as _;

/// An image one engine adopted, with the FileId and context of each of its
/// package's sources in manifest order.
pub(crate) struct AdoptedImage {
    pub(crate) image: Arc<StdImage>,
    files: Vec<(FileId, FileContext)>,
    /// The sources parsed on demand, by manifest ordinal.
    parsed: BTreeSet<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // An invalid hand-built TIR control: it names an entry body that is absent.
    // Skipping V would accept it, so these tests exercise evidence admission.
    fn invalid_lowering() -> ImageLowering {
        ImageLowering {
            verification: Default::default(),
            definition: Arc::new(hirsel_tir::Definition {
                caller_bodies: vec![],
                protocol_associations: vec![],
                types: Arc::from([]),
                path: DefId::intern(&DefPath::core("image_evidence_control")),
                signature_hash: ContentHash::of_bytes(b"control"),
                bodies: vec![],
                entry: hirsel_tir::BodyId(0),
                contracts: vec![],
            }),
            dependencies: vec![],
            execution: Reads::empty(),
            depth: crate::memo::DepthEvidence::leaf(32),
        }
    }

    #[test]
    fn repeated_image_admission_reuses_only_local_immutable_evidence() {
        crate::with_compiler_stack(|| {
            use hirsel_tir::*;
            let mut engine = QueryEngine::new(default_environment(), PhaseAdapters::default());
            let mut lowering = invalid_lowering();
            let ty = engine.types.intern(hirsel_types::Type::unit());
            let definition = Arc::make_mut(&mut lowering.definition);
            definition.types = vec![ty].into();
            definition.bodies = vec![Body {
                parameter_scope: hirsel_syntax::LexicalOwner::first(definition.path.path())
                    .site(hirsel_types::NameSite::Item),
                scoped_parameters: vec![],
                locals: vec![Local {
                    ty,
                    storage: Storage::ReturnSlot,
                    scope: ScopeId(0),
                    span: Span::default(),
                }],
                places: vec![Place {
                    loan: None,
                    binding: None,
                    root: LocalId(0),
                    projection: vec![].into(),
                    ty,
                }],
                scopes: vec![Scope {
                    parent: None,
                    span: Span::default(),
                }],
                blocks: vec![Block {
                    instructions: vec![Instruction {
                        kind: InstructionKind::Assign {
                            destination: PlaceId(0),
                            value: RvalueId(0),
                        },
                        span: Span::default(),
                        on_failure: CleanupId(0),
                    }],
                    terminator: Terminator::Return {
                        value: Operand {
                            place: PlaceId(0),
                            usage: hirsel_types::UseKind::Take,
                        },
                        cleanup: CleanupId(0),
                    },
                    span: Span::default(),
                }],
                rvalues: vec![Rvalue::Constant(Constant::Unit)],
                cleanups: vec![Cleanup::default()],
                cleanup_nodes: vec![],
                entry: BlockId(0),
                parameters: vec![],
                result: ty,
                borrow_cursors: vec![],
                tail_sites: vec![],
                matches: vec![],
            }];
            let id = lowering.definition.path;
            engine.admit_image_lowering(&lowering).unwrap();
            assert_eq!(engine.scalar_validation_runs[&id], 1);
            engine.admit_image_lowering(&lowering).unwrap();
            assert_eq!(
                engine.scalar_validation_runs[&id], 1,
                "same immutable artifact must not run V twice"
            );
            let bytes = hirsel_types::with_table_encoding(&engine.types, &[ty], || {
                bincode::serialize(&lowering).unwrap()
            });
            let decoded: ImageLowering =
                hirsel_types::with_table_decoding(&[ty], || bincode::deserialize(&bytes).unwrap());
            engine.admit_image_lowering(&decoded).unwrap();
            assert_eq!(
                engine.scalar_validation_runs[&id], 2,
                "serialized bytes never transfer local evidence"
            );
            let mut changed = lowering.clone();
            Arc::make_mut(&mut changed.definition).entry = BodyId(u32::MAX);
            assert!(
                engine.admit_image_lowering(&changed).is_err(),
                "cloned evidence cannot certify replaced bytes"
            );
        });
    }

    #[test]
    fn serialized_empty_claimed_verdict_cannot_certify_invalid_tir() {
        crate::with_compiler_stack(|| {
            let mut engine = QueryEngine::new(default_environment(), PhaseAdapters::default());
            let bytes = bincode::serialize(&invalid_lowering()).unwrap();
            let decoded: ImageLowering = bincode::deserialize(&bytes).unwrap();
            assert!(
                engine.admit_image_lowering(&decoded).is_err(),
                "unauthenticated image bytes cannot certify invalid TIR"
            );
        });
    }

    #[test]
    fn absent_or_stale_image_verdict_requires_v() {
        crate::with_compiler_stack(|| {
            let mut engine = QueryEngine::new(default_environment(), PhaseAdapters::default());
            let lowering = invalid_lowering();
            assert!(engine.admit_image_lowering(&lowering).is_err());
        });
    }

    #[test]
    fn image_verification_off_does_not_create_acceptance_evidence() {
        crate::with_compiler_stack(|| {
            let mut engine = QueryEngine::new(default_environment(), PhaseAdapters::default());
            let lowering = invalid_lowering();
            engine.verify = Verify::Off;
            assert!(engine.admit_image_lowering(&lowering).is_ok());
            engine.verify = Verify::On;
            assert!(engine.admit_image_lowering(&lowering).is_err());
        });
    }
}

fn corrupt(message: impl std::fmt::Display) -> QueryError {
    QueryError::Operational(hirsel_types::OperationalFailure {
        operation: "std image".into(),
        message: message.to_string(),
    })
}

impl QueryEngine {
    /// Use `image` for its package, whose sources are `files` in manifest
    /// order. The session's interner is built over the image's rows, so only
    /// before its first query.
    pub(crate) fn adopt_image(&mut self, image: Arc<StdImage>, files: Vec<(FileId, FileContext)>) {
        assert!(
            self.graph.is_empty() && self.image.is_none(),
            "an engine adopts one image, before its first query"
        );
        self.types = hirsel_types::TypeInterner::new(Some(image.base()));
        self.memo = Arc::new(hirsel_types::TypeMemo::new());
        self.comptime_runtime = None;
        for &(anchor, ordinal, base) in image.anchors() {
            if let Some((file, _)) = files.get(ordinal as usize) {
                self.sources.move_anchor(anchor, *file, base);
            }
        }
        self.declarations.adopt_rows(image.declarations());
        let revision = self.manifest_revision();
        self.declarations.inputs_mut().manifest_revision = revision;
        self.published_manifest = Some(revision);
        self.image = Some(AdoptedImage {
            image,
            files,
            parsed: BTreeSet::new(),
        });
    }
    /// Whether `package`'s checked products come from an image.
    pub fn imaged(&self, package: &PackageId) -> bool {
        self.image
            .as_ref()
            .is_some_and(|adopted| adopted.image.package() == package)
    }
    /// Exact immutable image actually selected by this engine, if any.
    pub fn selected_image(&self) -> Option<(&PackageId, ContentHash)> {
        self.image
            .as_ref()
            .map(|a| (a.image.package(), a.image.key()))
    }
    /// The adopted image declaring `path`.
    pub(crate) fn image_of(&self, path: &DefPath) -> Option<&Arc<StdImage>> {
        self.image
            .as_ref()
            .filter(|adopted| adopted.image.package() == &path.0.package)
            .map(|adopted| &adopted.image)
            .filter(|image| image.declares(path))
    }
    /// Whether `path` is a source definition: indexed, or declared by the
    /// adopted image.
    pub(crate) fn declared(&self, path: &DefPath) -> bool {
        self.items.contains_key(path)
            || self.catalog.body(path).is_some()
            || self.image_of(path).is_some()
    }
    /// Every source definition, indexed or declared by the adopted image.
    pub(crate) fn source_paths(&self) -> Vec<DefPath> {
        let mut paths: BTreeSet<DefPath> = self.items.keys().cloned().collect();
        paths.extend(self.catalog.paths().cloned());
        if let Some(adopted) = &self.image {
            paths.extend(adopted.image.declarations().paths().cloned());
        }
        paths.into_iter().collect()
    }
    /// The image's signature of `path`.
    pub(crate) fn image_signature(&self, path: &DefPath) -> Option<Arc<ImageSignature>> {
        self.image_of(path)?.signature_entry(DefId::intern(path))
    }
    /// The image's observed inputs of `path`, while its sources are not indexed.
    pub(crate) fn image_inputs(&self, path: &DefPath) -> Option<ImageInputs> {
        self.image_of(path)?.inputs(DefId::intern(path))
    }
    /// The definitions standing in `file`, or None for a source of the
    /// adopted image's package not parsed here.
    pub(crate) fn image_file_definitions(&self, file: FileId) -> Option<Vec<DefPath>> {
        if let Some(paths) = self.catalog.file_definitions(file) {
            return Some(paths);
        }
        let adopted = self.image.as_ref()?;
        let ordinal = adopted.files.iter().position(|(f, _)| *f == file)? as u32;
        if adopted.parsed.contains(&ordinal) {
            return None;
        }
        Some(
            adopted
                .image
                .file_definitions(ordinal)
                .iter()
                .map(|d| d.path().clone())
                .collect(),
        )
    }

    /// Parse the sources `path`'s items stand in, if the adopted image
    /// declares it and they are not parsed yet.
    pub(crate) fn materialize(&mut self, path: &DefPath) -> Result<(), QueryError> {
        self.materialize_catalog(path)?;
        if self.items.contains_key(path) {
            return Ok(());
        }
        let Some(image) = self.image_of(path).cloned() else {
            return Ok(());
        };
        let Some(row) = image.declarations().row_image(path) else {
            return Ok(());
        };
        let ordinals: BTreeSet<u32> = row
            .site_spans()
            .iter()
            .filter_map(|site| image.place(site.anchor).map(|(ordinal, _)| ordinal))
            .collect();
        for ordinal in ordinals {
            self.materialize_file(ordinal)?;
        }
        Ok(())
    }
    /// Parse `file` as the image's source it is, if it is one, keeping the image.
    pub(super) fn materialize_source(&mut self, file: FileId) -> Result<(), QueryError> {
        self.materialize_catalog_file(file)?;
        let ordinal = self
            .image
            .as_ref()
            .and_then(|adopted| adopted.files.iter().position(|(source, _)| *source == file));
        match ordinal {
            Some(ordinal) => self.materialize_file(ordinal as u32),
            None => Ok(()),
        }
    }
    /// Parse every source of the adopted image's package not parsed yet.
    pub(crate) fn materialize_package(&mut self) -> Result<(), QueryError> {
        let count = self.image.as_ref().map_or(0, |a| a.image.file_count());
        for ordinal in 0..count as u32 {
            self.materialize_file(ordinal)?;
        }
        Ok(())
    }
    /// Parse the image package's source at manifest `ordinal` and list its
    /// items, each at the image's anchor for its place with the occurrence its
    /// row records. The rows already describe them: nothing is indexed again.
    fn materialize_file(&mut self, ordinal: u32) -> Result<(), QueryError> {
        let Some(adopted) = self.image.as_mut() else {
            return Ok(());
        };
        if !adopted.parsed.insert(ordinal) {
            return Ok(());
        }
        let image = adopted.image.clone();
        let Some((file, context)) = adopted.files.get(ordinal as usize).cloned() else {
            return Err(corrupt(format!("no source {ordinal}")));
        };
        let mut output = self.parse_syntax(file, &context)?;
        if has_errors(&output.diagnostics) {
            return Err(corrupt("an image source does not parse"));
        }
        let mut listed = vec![];
        for new in output.value.items.iter_mut() {
            let (_, base) = self
                .sources
                .anchor(new.anchor)
                .expect("a parsed item's anchor is placed");
            let anchor = image
                .anchor_at(ordinal, base)
                .ok_or_else(|| corrupt("an image source item stands at no anchor"))?;
            let occurrence = self
                .declarations
                .sites(&new.item.definition)
                .iter()
                .position(|site| site.name_span.anchor == anchor)
                .ok_or_else(|| corrupt("an image source item has no row"))?;
            let syntax = Arc::make_mut(new);
            syntax.occurrence = occurrence as u32;
            syntax.reanchor(anchor);
            self.sources.move_anchor(anchor, file, base);
            let mut add = |syntax: Arc<ItemSyntax>| {
                let candidates = self
                    .items
                    .entry(syntax.item.definition.clone())
                    .or_default();
                candidates.push(syntax.clone());
                candidates.sort_by_key(|candidate| candidate.occurrence);
                listed.push(syntax);
            };
            if let Some(helper) = crate::facts::snapshot(new) {
                add(helper);
            }
            for helper in crate::dictionaries::literal_snapshots(new) {
                add(helper);
            }
            add(new.clone());
        }
        self.declarations.place_file(file, listed, &self.sources);
        let environment = self.package_environment(&context.package);
        self.parsed.insert(file, (context, environment, Ok(output)));
        Ok(())
    }
    /// Before a source of the image's package is indexed again: parse every
    /// source, record each definition's inputs as indexing would have, and
    /// end the image, whose products no longer describe the package.
    pub(super) fn release_image(&mut self, package: &PackageId) -> Result<(), QueryError> {
        if !self.imaged(package) {
            return Ok(());
        }
        self.materialize_package()?;
        let paths: Vec<DefPath> = self
            .items
            .keys()
            .filter(|path| &path.0.package == package)
            .cloned()
            .collect();
        for path in paths {
            let id = DefId::intern(&path);
            if !self.inputs.contains_key(&id)
                && let Some(inputs) = self.definition_inputs(&path)
            {
                self.inputs.insert(id, inputs);
            }
        }
        self.image = None;
        self.comptime_runtime = None;
        self.retire_snapshot();
        Ok(())
    }

    fn image_record(&self, path: &DefPath) -> Option<Arc<ImageRecord>> {
        self.image_of(path)?.record(DefId::intern(path))
    }
    pub(crate) fn image_node(&self, path: &DefPath) -> Option<Arc<ImageRecord>> {
        self.image_record(path)
    }
    pub(super) fn image_summary(&self, path: &DefPath) -> Option<Arc<PublishedSummary>> {
        self.image_record(path)?.summary.clone()
    }
    pub(crate) fn image_audit_products(
        &self,
        path: &DefPath,
    ) -> Option<crate::native_audit::SourceAuditProducts> {
        self.image_record(path)?.audit.clone()
    }
    pub(crate) fn image_constant(
        &self,
        path: &DefPath,
    ) -> Option<crate::comptime::EvaluatedConstant> {
        self.image_record(path)?.constant.clone()
    }
    /// The image's lowering of `path`, with the execution reads computing it
    /// would have recorded in every enclosing frame.
    pub(super) fn image_lowering(
        &mut self,
        path: &DefPath,
    ) -> Result<Option<QueryOutput<hirsel_tir::DefinitionRef>>, QueryError> {
        let Some(record) = self.image_record(path) else {
            return Ok(None);
        };
        let Some(lowering) = record.lowering.as_ref() else {
            return Ok(None);
        };
        if lowering.depth.limit() != self.depth_limit() {
            return Ok(None);
        }
        self.admit_image_lowering(lowering).map(Some)
    }
    fn admit_image_lowering(
        &mut self,
        lowering: &ImageLowering,
    ) -> Result<QueryOutput<hirsel_tir::DefinitionRef>, QueryError> {
        if self.verify == Verify::On {
            // The image key binds its producer and sources, but acceptance also
            // depends on the importing snapshot's independently checked views.
            let evidence = lowering.verification.lock().unwrap().clone();
            let snapshot = self.signature_snapshot();
            if !evidence.as_ref().is_some_and(|evidence| {
                evidence.holds(
                    &lowering.definition,
                    &self.types,
                    self.source_verification_policy(),
                    snapshot.validation(&lowering.definition.path.path().0.package),
                )
            }) {
                *lowering.verification.lock().unwrap() =
                    self.ensure_lowered_verdict(&lowering.definition)?;
            }
        }
        self.depth_replay_requests(&lowering.depth)?;
        self.merge_execution(&lowering.execution);
        Ok(QueryOutput {
            value: lowering.definition.clone(),
            diagnostics: vec![],
            dependencies: lowering.dependencies.clone(),
        })
    }

    /// The record of `path`, or None for a definition whose node or lowering
    /// fails: a program reaching it computes it from source, with the same
    /// result.
    fn image_record_of(&mut self, path: &DefPath) -> Result<Option<ImageRecord>, QueryError> {
        let Ok((content, dependencies)) = self.semantic_node(path) else {
            return Ok(None);
        };
        let executable = self.items[path]
            .first()
            .is_some_and(|item| generation::executable(item));
        let audit = if executable {
            Some(self.source_audit_products(path)?)
        } else {
            None
        };
        let mut constant = if matches!(
            self.item_signature(path)?.value.kind,
            hirsel_types::SignatureKind::Const { .. }
        ) {
            Some(self.evaluate_constant(path)?)
        } else {
            None
        };
        let mut lowering = if executable {
            self.execution_stack.push(ReadsBuilder::new());
            self.s1.begin_requests();
            let lowered = self.lower_to_tir(path);
            let depth = self.s1.finish_requests(self.depth_limit());
            let execution = self
                .execution_stack
                .pop()
                .expect("image execution frame")
                .finish()
                .map_err(|_| QueryError::NotImplemented)?;
            let Ok(lowered) = lowered else {
                return Ok(None);
            };
            if !lowered.diagnostics.is_empty() {
                return Ok(None);
            }
            Some(ImageLowering {
                verification: Default::default(),
                definition: lowered.value,
                dependencies: lowered.dependencies,
                execution,
                depth,
            })
        } else {
            None
        };
        // A whole-universe worker receipt can depend on the imager's root.
        // These optional products cannot become a package-local proof. Drop
        // their resident counterparts too, so release checks describe the
        // products actually written; other closure checks stay authoritative.
        if constant
            .as_ref()
            .is_some_and(|value| has_universe_input(&value.executed))
        {
            constant = None;
            self.constant_values.remove(path);
        }
        if lowering
            .as_ref()
            .is_some_and(|value| has_universe_input(&value.execution))
        {
            lowering = None;
            self.forget(&MemoKey::new(QueryKind::Lower, DefId::intern(path)));
        }
        Ok(Some(ImageRecord {
            node: ImageNode {
                content,
                dependencies,
            },
            summary: self.definition_summary(DefId::intern(path)).ok(),
            lowering,
            audit,
            constant,
        }))
    }

    /// Compute the records of `paths` that belong to the build's package and
    /// have none yet, then release their resolved, checked, mode and lowered bodies.
    pub(crate) fn image_definitions(
        &mut self,
        build: &mut ImageBuild,
        paths: Vec<DefPath>,
    ) -> Result<(), QueryError> {
        let paths: Vec<_> = paths
            .into_iter()
            .filter(|path| path.0.package == build.package && build.seen.insert(path.clone()))
            .collect();
        for path in &paths {
            if let Some(record) = self.image_record_of(path)? {
                build.spill(DefId::intern(path), &record)?;
            }
        }
        build.closed &= self.release_bodies(&paths, &build.closure);
        Ok(())
    }
    /// Release the bodies of the package's definitions that have no record
    /// yet; each is computed again when its source checks.
    pub(crate) fn release_unrecorded(&mut self, build: &mut ImageBuild) {
        let paths: Vec<_> = self
            .items
            .keys()
            .filter(|p| p.0.package == build.package && !build.seen.contains(*p))
            .cloned()
            .collect();
        // P/S1 has checked all files already, but these paths have not had
        // records written yet. Whole-universe optional products are omitted
        // just as image_record_of does for recorded paths; they cannot become
        // closure-local image proofs merely by surviving in resident tables.
        for path in &paths {
            let id = DefId::intern(path);
            if self
                .lowered
                .get(&id)
                .is_some_and(|entry| has_universe_input(entry.reads()))
            {
                self.forget(&MemoKey::new(QueryKind::Lower, id));
            }
            if self
                .constant_values
                .get(path)
                .is_some_and(|value| has_universe_input(&value.executed))
            {
                self.constant_values.remove(path);
            }
        }
        build.closed &= self.release_bodies(&paths, &build.closure);
    }

    /// The image of the build's package, keyed by `key`, after a complete
    /// check of this program found no diagnostics: the records of its
    /// remaining definitions, then every definition's row, signature and
    /// inputs.
    pub(crate) fn finish_image(
        &mut self,
        mut build: ImageBuild,
        key: ContentHash,
    ) -> Result<Vec<u8>, QueryError> {
        let rest = self
            .items
            .keys()
            .filter(|p| p.0.package == build.package)
            .cloned()
            .collect();
        self.image_definitions(&mut build, rest)?;
        if !(build.closed && self.package_closed(&build.package, &build.closure)) {
            return Err(corrupt(format!(
                "{}@{} reads outside its dependency closure",
                build.package.name, build.package.version
            )));
        }
        let mut writer = hirsel_image::ImageWriter::new(key, self.types.clone());
        writer.anchors(
            self.sources.anchors().filter_map(|(anchor, file, base)| {
                Some((anchor, *build.ordinal.get(&file)?, base))
            }),
        );
        let paths: Vec<DefPath> = self
            .items
            .keys()
            .filter(|p| p.0.package == build.package)
            .cloned()
            .collect();
        let mut files: BTreeMap<u32, Vec<DefId>> = BTreeMap::new();
        for path in &paths {
            let d = DefId::intern(path);
            let Some(row) = self.declarations.row_image(path) else {
                continue;
            };
            let mut ordinals: Vec<u32> = self.items[path]
                .iter()
                .filter_map(|item| build.ordinal.get(&self.item_file(item)).copied())
                .collect();
            ordinals.dedup();
            for ordinal in ordinals {
                files.entry(ordinal).or_default().push(d);
            }
            writer.entry(crate::image::ROWS, d, Box::new(Whole(row)));
            let signature = self.signature_query(path)?;
            let covered = self.signature_coverage(path)?;
            let owner = self
                .owner_record(path)
                .map(|record| (record.representation.declaration, record.dependencies));
            writer.entry(
                crate::image::SIGNATURES,
                d,
                Box::new(Whole(SignatureEntry {
                    signature: (*signature.value).clone(),
                    dependencies: signature.dependencies.clone(),
                    covered,
                    owner,
                })),
            );
            let inputs = self
                .observed_inputs(path)
                .ok_or_else(|| corrupt(format!("{path} has no inputs")))?;
            writer.entry(crate::image::INPUTS, d, Box::new(Whole(inputs)));
        }
        for (d, record) in build.records()? {
            writer.entry(crate::image::RECORDS, d, Box::new(record));
        }
        let generated = paths
            .iter()
            .filter(|path| self.instance_bodied(path))
            .map(DefId::intern)
            .collect();
        writer.blob(
            crate::image::META,
            Box::new(Whole(Meta {
                package: build.package.clone(),
                files: meta_files(&files, build.ordinal.len()),
                generated,
            })),
        );
        let written = writer.finish().map_err(corrupt)?;
        if let Some((kind, d, error)) = written
            .dropped
            .iter()
            .find(|(kind, _, _)| *kind != crate::image::RECORDS)
        {
            return Err(corrupt(format!("section {kind} cannot hold {d}: {error}")));
        }
        Ok(written.bytes)
    }
}

/// An image being built (`QueryEngine::image_definitions`). Each record goes
/// to a spill file as soon as it is computed, its ids as this session's, so
/// the build never holds more than one record; `finish_image` reads them back
/// in DefPath order into the image.
pub(crate) struct ImageBuild {
    package: PackageId,
    closure: BTreeSet<PackageId>,
    /// Each package source's manifest ordinal.
    ordinal: BTreeMap<FileId, u32>,
    spill: std::io::BufWriter<std::fs::File>,
    /// Each spilled record's definition and byte range.
    spilled: Vec<(DefId, u64, u64)>,
    len: u64,
    /// The definitions recorded or left out.
    seen: BTreeSet<DefPath>,
    /// Every released query read only inside the closure.
    closed: bool,
}

impl ImageBuild {
    /// An image of `package`, whose sources are `files` in manifest order,
    /// spilling records to `spill`.
    pub(crate) fn new(
        package: PackageId,
        closure: BTreeSet<PackageId>,
        files: &[FileId],
        spill: std::fs::File,
    ) -> Self {
        Self {
            package,
            closure,
            ordinal: files
                .iter()
                .enumerate()
                .map(|(index, file)| (*file, index as u32))
                .collect(),
            spill: std::io::BufWriter::new(spill),
            spilled: vec![],
            len: 0,
            seen: BTreeSet::new(),
            closed: true,
        }
    }
    /// Whether `file` is one of the package's sources.
    pub(crate) fn has_file(&self, file: FileId) -> bool {
        self.ordinal.contains_key(&file)
    }
    fn spill(&mut self, d: DefId, record: &ImageRecord) -> Result<(), QueryError> {
        let bytes =
            hirsel_types::with_session_ids(|| bincode::serialize(record)).map_err(corrupt)?;
        self.spill.write_all(&bytes).map_err(corrupt)?;
        self.spilled.push((d, self.len, bytes.len() as u64));
        self.len += bytes.len() as u64;
        Ok(())
    }
    /// The spilled records, each read back when the image encodes it.
    fn records(&mut self) -> Result<Vec<(DefId, Spilled)>, QueryError> {
        self.spill.flush().map_err(corrupt)?;
        let file = Arc::new(std::sync::Mutex::new(
            self.spill.get_ref().try_clone().map_err(corrupt)?,
        ));
        Ok(self
            .spilled
            .iter()
            .map(|&(d, start, len)| {
                (
                    d,
                    Spilled {
                        file: file.clone(),
                        start,
                        len,
                    },
                )
            })
            .collect())
    }
}

/// A spilled record, read back as the image encodes it.
struct Spilled {
    file: Arc<std::sync::Mutex<std::fs::File>>,
    start: u64,
    len: u64,
}
impl Spilled {
    fn read(&self) -> Result<ImageRecord, String> {
        use std::io::{Read, Seek};
        let mut bytes = vec![0; self.len as usize];
        {
            let mut file = self.file.lock().map_err(|e| e.to_string())?;
            file.seek(std::io::SeekFrom::Start(self.start))
                .map_err(|e| e.to_string())?;
            file.read_exact(&mut bytes).map_err(|e| e.to_string())?;
        }
        hirsel_types::with_session_ids(|| bincode::deserialize(&bytes)).map_err(|e| e.to_string())
    }
}
impl hirsel_image::Encode for Spilled {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), String> {
        self.read()?.encode(out)
    }
    fn reduced(&self) -> Option<Box<dyn hirsel_image::Encode>> {
        self.read().ok()?.reduced()
    }
}

/// Membership in a package's closure, where core is always inside.
fn inside(closure: &BTreeSet<PackageId>) -> impl Fn(&PackageId) -> bool {
    let core = hirsel_syntax::DefPath::core("Result").0.package;
    move |p: &PackageId| closure.contains(p) || *p == core
}

fn has_universe_input(reads: &Reads) -> bool {
    reads
        .iter()
        .any(|(key, _)| matches!(key, ReadKey::ValidationUniverse(_)))
}

/// Every read names only packages `inside` accepts.
fn reads_closed(reads: &crate::memo::Reads, inside: &impl Fn(&PackageId) -> bool) -> bool {
    reads.iter().all(|(read, _)| match read {
        // The base universe token includes every currently selected package.
        // A dependency image cannot claim this input is closure-local merely
        // because the originating evaluator belongs to the dependency.
        ReadKey::ValidationUniverse(_) => false,
        ReadKey::Lookup(l) => {
            inside(&l.package.id()) && l.qualifier.is_none_or(|q| inside(&q.id()))
        }
        read => read.package().is_none_or(|p| inside(&p.id())),
    })
}

impl QueryEngine {
    /// No retained query of `package` reads outside its closure (or core).
    fn package_closed(&self, package: &PackageId, closure: &BTreeSet<PackageId>) -> bool {
        let inside = inside(closure);
        macro_rules! closed {
            ($field:ident) => {
                self.$field
                    .values()
                    .filter(|e| &e.memo.key.def.path().0.package == package)
                    .all(|e| reads_closed(e.reads(), &inside))
            };
        }
        closed!(signatures)
            && closed!(resolutions)
            && closed!(checked)
            && closed!(modes)
            && closed!(lowered)
            && closed!(summaries)
            && self
                .constant_values
                .iter()
                .filter(|(p, _)| &p.0.package == package)
                .all(|(_, value)| reads_closed(&value.executed, &inside))
    }
    /// Release the resolved, checked, mode and lowered bodies of `paths`; a later read
    /// computes them again. Whether
    /// each released query read only inside `closure` (or core).
    fn release_bodies(&mut self, paths: &[DefPath], closure: &BTreeSet<PackageId>) -> bool {
        let inside = inside(closure);
        let mut closed = true;
        for path in paths {
            let id = DefId::intern(path);
            // Resolution owns body-sized lookup/read logs, including the
            // phase snapshot's copy. A spilled record needs neither; keep
            // their closure evidence before forgetting them like H and TIR.
            closed &= self
                .resolutions
                .get(&id)
                .is_none_or(|e| reads_closed(e.reads(), &inside))
                && self
                    .checked
                    .get(&id)
                    .is_none_or(|e| reads_closed(e.reads(), &inside))
                && self
                    .modes
                    .get(&id)
                    .is_none_or(|e| reads_closed(e.reads(), &inside))
                && self
                    .lowered
                    .get(&id)
                    .is_none_or(|e| reads_closed(e.reads(), &inside))
                && self
                    .summaries
                    .get(&id)
                    .is_none_or(|e| reads_closed(e.reads(), &inside));
            for kind in [
                QueryKind::Resolve,
                QueryKind::Check,
                QueryKind::Modes,
                QueryKind::Lower,
                QueryKind::Summary,
            ] {
                self.forget(&MemoKey::new(kind, id));
            }
        }
        closed
    }
}
