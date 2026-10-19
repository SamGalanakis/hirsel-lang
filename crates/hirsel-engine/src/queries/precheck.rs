//! Prechecked packages: a dependency package checked once, by the toolchain build,
//! instead of by every program that selects it (docs/building.md, "The std precheck").
//!
//! A precheck holds what running a program needs of the package: each definition's
//! semantic-revision node and, for executable definitions, its lowered and
//! validated TIR, completed source audit, compile-time substitutions and constants,
//! with the execution reads that computing them adds to enclosing frames.
//! It is keyed by the compiler identity and the package's complete checked input,
//! and written only for a package that checks complete with no diagnostics and
//! whose queries read nothing outside its closure. A program whose package
//! matches reuses these products; other queries still compute from source, so
//! a precheck changes only how long a run takes.
use super::*;
use hirsel_syntax::PackageId;
use hirsel_tir::Definition;
use hirsel_types::{DecodedTypes, EncodedTypes};
use serde::{Deserialize, Serialize};
use std::{io::Write, ops::Range, path::Path, sync::Mutex};

const MAGIC: &[u8; 8] = b"HIRPCHK3";

#[derive(Serialize, Deserialize)]
struct Header {
    key: ContentHash,
    package: PackageId,
    /// Where each anchor its spans name stands, by number from 1: the
    /// package source's manifest ordinal and the anchor's base in it.
    anchors: Vec<(u32, u32)>,
    /// Each definition's record, as a byte range after the header.
    definitions: Vec<(DefPath, u64, u64)>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct PrecheckedNode {
    pub content: Vec<Vec<u8>>,
    pub dependencies: BTreeSet<DefPath>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PrecheckedLowering {
    definition: Definition,
    dependencies: Vec<SignatureDependency>,
    execution: ExecutionReads,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct PrecheckedDefinition {
    node: PrecheckedNode,
    lowering: Option<PrecheckedLowering>,
    audit: Option<crate::native_audit::SourceAuditProducts>,
    constant: Option<crate::comptime::EvaluatedConstant>,
}

/// A decoded precheck file. Records decode on first use and are shared by every
/// engine in the process that adopts the precheck.
pub struct Precheck {
    key: ContentHash,
    package: PackageId,
    records: BTreeMap<DefPath, Range<usize>>,
    /// The anchors decoded spans name, each standing in a package source
    /// (manifest ordinal) at a base; an adopting engine places them.
    anchors: Vec<(hirsel_span::Anchor, u32, u32)>,
    bytes: Vec<u8>,
    decoded: Mutex<(DecodedTypes, BTreeMap<DefPath, Arc<PrecheckedDefinition>>)>,
}

impl std::fmt::Debug for Precheck {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Precheck({}@{})",
            self.package.name, self.package.version
        )
    }
}

fn corrupt(message: impl std::fmt::Display) -> QueryError {
    QueryError::Operational(hirsel_types::OperationalFailure {
        operation: "precheck".into(),
        message: message.to_string(),
    })
}

impl Precheck {
    /// The precheck key: the compiler identity, the package's checked-input
    /// revision (environment, closure manifests and source contents) and the
    /// compile-time limits its constants evaluated under.
    pub(crate) fn key(
        identity: ContentHash,
        revision: ContentHash,
        limits: hirsel_interp::Limits,
    ) -> ContentHash {
        ContentHash::of_parts(
            "hirsel.precheck.v1",
            [
                identity.0.as_slice(),
                revision.0.as_slice(),
                limits
                    .steps
                    .map(|steps| steps.to_le_bytes().to_vec())
                    .unwrap_or_default()
                    .as_slice(),
                (limits.call_depth as u64).to_le_bytes().as_slice(),
            ],
        )
    }
    pub fn package(&self) -> &PackageId {
        &self.package
    }
    /// The definitions this precheck holds products for.
    pub fn definitions(&self) -> impl Iterator<Item = &DefPath> {
        self.records.keys()
    }
    pub(crate) fn matches(&self, key: ContentHash) -> bool {
        self.key == key
    }
    pub fn read(path: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::decode(bytes).map_err(|e| format!("{}: {e}", path.display()))
    }
    pub fn decode(bytes: Vec<u8>) -> Result<Self, String> {
        let rest = bytes
            .strip_prefix(MAGIC.as_slice())
            .ok_or("not a Hirsel precheck")?;
        let mut reader = rest;
        let header: Header = bincode::deserialize_from(&mut reader).map_err(|e| e.to_string())?;
        let base = bytes.len() - reader.len();
        let mut records = BTreeMap::new();
        for (path, start, len) in header.definitions {
            let start = base + start as usize;
            let end = start
                .checked_add(len as usize)
                .filter(|end| *end <= bytes.len())
                .ok_or("truncated precheck")?;
            records.insert(path, start..end);
        }
        Ok(Self {
            key: header.key,
            package: header.package,
            records,
            anchors: header
                .anchors
                .into_iter()
                .map(|(ordinal, base)| (hirsel_span::Anchor::fresh(), ordinal, base))
                .collect(),
            bytes,
            decoded: Mutex::default(),
        })
    }
    fn definition(&self, path: &DefPath) -> Result<Option<Arc<PrecheckedDefinition>>, QueryError> {
        let Some(range) = self.records.get(path) else {
            return Ok(None);
        };
        let mut decoded = self.decoded.lock().unwrap();
        if let Some(record) = decoded.1.get(path) {
            return Ok(Some(record.clone()));
        }
        let (provenance, records) = &mut *decoded;
        let mut anchors =
            hirsel_span::AnchorCodec::Decode(self.anchors.iter().map(|a| a.0).collect());
        let record: PrecheckedDefinition = hirsel_types::decode_types(provenance, || {
            hirsel_span::with_anchor_codec(&mut anchors, || {
                bincode::deserialize(&self.bytes[range.clone()])
            })
        })
        .map_err(corrupt)?;
        let record = Arc::new(record);
        records.insert(path.clone(), record.clone());
        Ok(Some(record))
    }
}

/// A precheck one engine adopted, with the FileId of each of its package's sources.
pub(crate) struct AdoptedPrecheck {
    precheck: Arc<Precheck>,
    files: Vec<FileId>,
    lowered: BTreeMap<DefPath, hirsel_tir::DefinitionRef>,
}

impl QueryEngine {
    /// Use `precheck` for `package`, whose sources are `files` in manifest order.
    pub(crate) fn adopt_precheck(&mut self, precheck: Arc<Precheck>, files: Vec<FileId>) {
        self.comptime_runtime = None;
        for &(anchor, ordinal, base) in &precheck.anchors {
            if let Some(file) = files.get(ordinal as usize) {
                self.sources.move_anchor(anchor, *file, base);
            }
        }
        self.prechecks.insert(
            precheck.package.clone(),
            AdoptedPrecheck {
                precheck,
                files,
                lowered: BTreeMap::new(),
            },
        );
    }
    /// Whether `package`'s checked products come from a precheck.
    pub fn prechecked(&self, package: &PackageId) -> bool {
        self.prechecks.contains_key(package)
    }
    /// A source edit in a prechecked package ends its precheck for this engine.
    pub(super) fn drop_precheck(&mut self, package: &PackageId) {
        self.prechecks.remove(package);
    }
    fn prechecked_definition(
        &self,
        path: &DefPath,
    ) -> Result<Option<Arc<PrecheckedDefinition>>, QueryError> {
        match self.prechecks.get(&path.0.package) {
            Some(adopted) => adopted.precheck.definition(path),
            None => Ok(None),
        }
    }
    pub(super) fn prechecked_node(
        &self,
        path: &DefPath,
    ) -> Result<Option<PrecheckedNodeRef>, QueryError> {
        Ok(self.prechecked_definition(path)?.map(PrecheckedNodeRef))
    }
    pub(crate) fn prechecked_audit_products(
        &self,
        path: &DefPath,
    ) -> Result<Option<crate::native_audit::SourceAuditProducts>, QueryError> {
        Ok(self
            .prechecked_definition(path)?
            .and_then(|record| record.audit.clone()))
    }
    pub(crate) fn prechecked_constant(
        &mut self,
        path: &DefPath,
    ) -> Result<Option<crate::comptime::EvaluatedConstant>, QueryError> {
        let value = self
            .prechecked_definition(path)?
            .and_then(|record| record.constant.clone());
        if let Some(value) = &value {
            self.inherit_execution(&value.executed);
        }
        Ok(value)
    }
    /// The prechecked lowering of `path`, with the execution reads computing it
    /// would have recorded in every enclosing frame.
    pub(super) fn prechecked_lowering(
        &mut self,
        path: &DefPath,
    ) -> Result<Option<QueryOutput<hirsel_tir::DefinitionRef>>, QueryError> {
        let Some(record) = self.prechecked_definition(path)? else {
            return Ok(None);
        };
        let Some(lowering) = &record.lowering else {
            return Ok(None);
        };
        self.inherit_execution(&lowering.execution);
        let adopted = self.prechecks.get_mut(&path.0.package).unwrap();
        let value = match adopted.lowered.get(path) {
            Some(definition) => definition.clone(),
            None => {
                let definition = Arc::new(lowering.definition.clone());
                adopted.lowered.insert(path.clone(), definition.clone());
                definition
            }
        };
        Ok(Some(QueryOutput {
            value,
            diagnostics: vec![],
            dependencies: lowering.dependencies.clone(),
        }))
    }

    /// The precheck record of `path`, or None for a definition whose node or
    /// lowering fails: a program reaching it computes it from source, with
    /// the same result.
    fn precheck_record(
        &mut self,
        path: &DefPath,
    ) -> Result<Option<PrecheckedDefinition>, QueryError> {
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
        let constant = if matches!(
            self.item_signature(path)?.value.kind,
            hirsel_types::SignatureKind::Const { ty: _, .. }
        ) {
            Some(self.evaluate_constant(path)?)
        } else {
            None
        };
        let lowering = if executable {
            self.execution_stack.push(ExecutionReads::default());
            let lowered = self.lower_to_tir(path);
            let execution = self
                .execution_stack
                .pop()
                .expect("precheck execution frame");
            let Ok(lowered) = lowered else {
                return Ok(None);
            };
            if !lowered.diagnostics.is_empty() {
                return Ok(None);
            }
            Some(PrecheckedLowering {
                definition: (*lowered.value).clone(),
                dependencies: lowered.dependencies,
                execution,
            })
        } else {
            None
        };
        Ok(Some(PrecheckedDefinition {
            node: PrecheckedNode {
                content,
                dependencies,
            },
            lowering,
            audit,
            constant,
        }))
    }

    /// Write the records of `paths` that belong to the writer's package and
    /// have none yet, then release their checked, mode and lowered bodies.
    pub(crate) fn precheck_definitions<W: Write>(
        &mut self,
        writer: &mut PrecheckWriter<W>,
        paths: Vec<DefPath>,
    ) -> Result<(), QueryError> {
        let paths: Vec<_> = paths
            .into_iter()
            .filter(|path| path.0.package == writer.package && writer.seen.insert(path.clone()))
            .collect();
        for path in &paths {
            if let Some(record) = self.precheck_record(path)? {
                writer.write(path, record, &self.sources)?;
            }
        }
        writer.closed &= self.release_bodies(&paths, &writer.closure);
        Ok(())
    }

    /// Release the bodies of the package's definitions that have no record yet;
    /// each is computed again when its source checks.
    pub(crate) fn release_unwritten<W>(&mut self, writer: &mut PrecheckWriter<W>) {
        let paths: Vec<_> = self
            .items
            .keys()
            .filter(|p| p.0.package == writer.package && !writer.seen.contains(*p))
            .cloned()
            .collect();
        writer.closed &= self.release_bodies(&paths, &writer.closure);
    }

    /// Write the records of the package's remaining definitions and return
    /// the precheck's head, its magic and header, after a complete check of
    /// this program found no diagnostics. Its body is what `writer` wrote.
    pub(crate) fn finish_precheck<W: Write>(
        &mut self,
        mut writer: PrecheckWriter<W>,
        key: ContentHash,
    ) -> Result<Vec<u8>, QueryError> {
        let rest = self
            .items
            .keys()
            .filter(|p| p.0.package == writer.package)
            .cloned()
            .collect();
        self.precheck_definitions(&mut writer, rest)?;
        if !(writer.closed && self.package_closed(&writer.package, &writer.closure)) {
            return Err(corrupt(format!(
                "{}@{} reads outside its dependency closure",
                writer.package.name, writer.package.version
            )));
        }
        writer.body.flush().map_err(corrupt)?;
        let mut head = MAGIC.to_vec();
        bincode::serialize_into(
            &mut head,
            &Header {
                key,
                package: writer.package,
                anchors: writer
                    .anchors
                    .iter()
                    .map(|&(_, ordinal, base)| (ordinal, base))
                    .collect(),
                definitions: writer.definitions,
            },
        )
        .map_err(corrupt)?;
        Ok(head)
    }
}

/// A precheck being written (`QueryEngine::precheck_definitions`). Each
/// definition's record goes to the body as soon as it is computed, so the
/// precheck never holds more than one record or the encoded package.
pub(crate) struct PrecheckWriter<W> {
    package: PackageId,
    closure: BTreeSet<PackageId>,
    /// Each package source's manifest ordinal.
    ordinal: BTreeMap<FileId, u32>,
    /// The anchors spans name, numbered from 1 in order, each with its
    /// source's ordinal and its base there; the header's table.
    anchors: Vec<(hirsel_span::Anchor, u32, u32)>,
    codec: hirsel_span::AnchorCodec,
    body: W,
    len: u64,
    types: EncodedTypes,
    /// Each written record's byte range in the body.
    definitions: Vec<(DefPath, u64, u64)>,
    /// The definitions written or left out.
    seen: BTreeSet<DefPath>,
    /// Every released query read only inside the closure.
    closed: bool,
}

impl<W: Write> PrecheckWriter<W> {
    /// A precheck of `package`, whose sources are `files` in manifest order,
    /// writing its body to `body`.
    pub(crate) fn new(
        package: PackageId,
        closure: BTreeSet<PackageId>,
        files: &[FileId],
        body: W,
    ) -> Self {
        Self {
            package,
            closure,
            ordinal: files
                .iter()
                .enumerate()
                .map(|(index, file)| (*file, index as u32))
                .collect(),
            anchors: vec![],
            codec: hirsel_span::AnchorCodec::Encode(BTreeMap::new()),
            body,
            len: 0,
            types: EncodedTypes::default(),
            definitions: vec![],
            seen: BTreeSet::new(),
            closed: true,
        }
    }
    /// Whether `file` is one of the package's sources.
    pub(crate) fn has_file(&self, file: FileId) -> bool {
        self.ordinal.contains_key(&file)
    }
    /// Number the anchors `sources` places in the package's sources that have
    /// no number yet. Parsing places them as the program checks.
    fn number_anchors(&mut self, sources: &hirsel_span::SourceMap) {
        let hirsel_span::AnchorCodec::Encode(numbers) = &mut self.codec else {
            unreachable!("a precheck writer encodes");
        };
        for (anchor, file, base) in sources.anchors() {
            if let Some(ordinal) = self.ordinal.get(&file)
                && !numbers.contains_key(&anchor)
            {
                self.anchors.push((anchor, *ordinal, base));
                numbers.insert(anchor, self.anchors.len() as u32);
            }
        }
    }
    fn encode(&mut self, record: &PrecheckedDefinition) -> Option<Vec<u8>> {
        let mut bytes = vec![];
        let codec = &mut self.codec;
        hirsel_types::encode_types(&mut self.types, || {
            hirsel_span::with_anchor_codec(codec, || bincode::serialize_into(&mut bytes, record))
        })
        .ok()
        .map(|()| bytes)
    }
    /// Write `record`. TIR with a span outside the package's sources (a
    /// generated source) is left out, as is a record that still has one.
    fn write(
        &mut self,
        path: &DefPath,
        mut record: PrecheckedDefinition,
        sources: &hirsel_span::SourceMap,
    ) -> Result<(), QueryError> {
        let bytes = match self.encode(&record) {
            Some(bytes) => bytes,
            None => {
                self.number_anchors(sources);
                match self.encode(&record) {
                    Some(bytes) => bytes,
                    None if record.lowering.is_some() => {
                        record.lowering = None;
                        match self.encode(&record) {
                            Some(bytes) => bytes,
                            None => return Ok(()),
                        }
                    }
                    None => return Ok(()),
                }
            }
        };
        self.body.write_all(&bytes).map_err(corrupt)?;
        self.definitions
            .push((path.clone(), self.len, bytes.len() as u64));
        self.len += bytes.len() as u64;
        Ok(())
    }
}

/// A prechecked definition's revision node.
pub(crate) struct PrecheckedNodeRef(Arc<PrecheckedDefinition>);
impl PrecheckedNodeRef {
    /// Whether the precheck holds the definition's TIR.
    pub(super) fn has_lowering(&self) -> bool {
        self.0.lowering.is_some()
    }
    pub(super) fn has_staged_reads(&self) -> bool {
        self.0
            .lowering
            .as_ref()
            .is_some_and(|lowering| crate::comptime::has_staged_reads(&lowering.definition))
    }
}
impl std::ops::Deref for PrecheckedNodeRef {
    type Target = PrecheckedNode;
    fn deref(&self) -> &PrecheckedNode {
        &self.0.node
    }
}
