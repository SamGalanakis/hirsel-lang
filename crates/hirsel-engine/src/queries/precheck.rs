//! Prechecked packages: a dependency package checked once, by the toolchain build,
//! instead of by every program that selects it (docs/building.md, "The std precheck").
//!
//! A precheck holds what running a program needs of the package: each definition's
//! semantic-revision node and, for executable definitions, its lowered and
//! validated TIR with the execution reads that lowering adds to enclosing frames.
//! It is keyed by the compiler identity and the package's complete checked input,
//! and written only for a package that checks complete with no diagnostics and
//! whose queries read nothing outside its closure. A program whose package
//! matches skips checking it; every other query on its definitions still
//! computes from source, so a precheck changes only how long a run takes.
use super::*;
use hirsel_syntax::PackageId;
use hirsel_tir::Definition;
use hirsel_types::{DecodedTypes, EncodedTypes};
use serde::{Deserialize, Serialize};
use std::{ops::Range, path::Path, sync::Mutex};

const MAGIC: &[u8; 8] = b"HIRPCHK1";

#[derive(Serialize, Deserialize)]
struct Header {
    key: ContentHash,
    package: PackageId,
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
    /// Spans name the package's sources by manifest ordinal plus one; zero
    /// stays the synthetic-site sentinel.
    definition: Definition,
    dependencies: Vec<SignatureDependency>,
    execution: ExecutionReads,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct PrecheckedDefinition {
    node: PrecheckedNode,
    lowering: Option<PrecheckedLowering>,
}

/// A decoded precheck file. Records decode on first use and are shared by every
/// engine in the process that adopts the precheck.
pub struct Precheck {
    key: ContentHash,
    package: PackageId,
    records: BTreeMap<DefPath, Range<usize>>,
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
        let record: PrecheckedDefinition = hirsel_types::decode_types(provenance, || {
            bincode::deserialize(&self.bytes[range.clone()])
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

/// Each TIR span that names a source, in the fields source rebasing moves.
pub(super) fn tir_spans(definition: &mut Definition, mut span: impl FnMut(&mut Span)) {
    for body in &mut definition.bodies {
        for local in &mut body.locals {
            span(&mut local.span);
        }
        for scope in &mut body.scopes {
            span(&mut scope.span);
        }
        for block in &mut body.blocks {
            span(&mut block.span);
            for instruction in &mut block.instructions {
                span(&mut instruction.span);
            }
        }
    }
}

impl QueryEngine {
    /// Use `precheck` for `package`, whose sources are `files` in manifest order.
    pub(crate) fn adopt_precheck(&mut self, precheck: Arc<Precheck>, files: Vec<FileId>) {
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
                let mut definition = lowering.definition.clone();
                let files = &adopted.files;
                tir_spans(&mut definition, |span| {
                    if *span != Span::default() {
                        span.file = files[span.file.0 as usize - 1];
                    }
                });
                let definition = Arc::new(definition);
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

    /// Precheck `package`, whose sources are `files` in manifest order, after a
    /// complete check of this program found no diagnostics. Each definition
    /// whose node or lowering fails is left out: a program reaching it
    /// computes it from source, with the same result.
    pub(crate) fn precheck(
        &mut self,
        key: ContentHash,
        package: &PackageId,
        closure: &BTreeSet<PackageId>,
        files: &[FileId],
    ) -> Result<Vec<u8>, QueryError> {
        let ordinal: BTreeMap<FileId, u32> = files
            .iter()
            .enumerate()
            .map(|(index, file)| (*file, index as u32 + 1))
            .collect();
        let paths: Vec<_> = self
            .items
            .keys()
            .filter(|p| &p.0.package == package)
            .cloned()
            .collect();
        let mut records = vec![];
        for path in paths {
            let Ok((content, dependencies)) = self.semantic_node(&path) else {
                continue;
            };
            let executable = self.items[&path].first().is_some_and(|item| {
                matches!(
                    item.file.tree.items[item.item.0 as usize].kind,
                    ItemKind::Function { body: Some(_), .. }
                        | ItemKind::Const { .. }
                        | ItemKind::Type { drop: Some(_), .. }
                )
            });
            let lowering = if executable {
                self.execution_stack.push(ExecutionReads::default());
                let lowered = self.lower_to_tir(&path);
                let execution = self
                    .execution_stack
                    .pop()
                    .expect("precheck execution frame");
                let Ok(lowered) = lowered else { continue };
                if !lowered.diagnostics.is_empty() {
                    continue;
                }
                let mut definition = (*lowered.value).clone();
                let mut foreign = false;
                tir_spans(&mut definition, |span| {
                    if *span != Span::default() {
                        match ordinal.get(&span.file) {
                            Some(index) => span.file = FileId(*index),
                            None => foreign = true,
                        }
                    }
                });
                if foreign {
                    continue;
                }
                Some(PrecheckedLowering {
                    definition,
                    dependencies: lowered.dependencies,
                    execution,
                })
            } else {
                None
            };
            records.push((
                path,
                PrecheckedDefinition {
                    node: PrecheckedNode {
                        content,
                        dependencies,
                    },
                    lowering,
                },
            ));
        }
        if !self.package_closed(package, closure) {
            return Err(corrupt(format!(
                "{}@{} reads outside its dependency closure",
                package.name, package.version
            )));
        }
        let mut body = vec![];
        let mut definitions = vec![];
        let mut types = EncodedTypes::default();
        for (path, record) in records {
            let start = body.len() as u64;
            hirsel_types::encode_types(&mut types, || bincode::serialize_into(&mut body, &record))
                .map_err(corrupt)?;
            definitions.push((path, start, body.len() as u64 - start));
        }
        let mut bytes = MAGIC.to_vec();
        bincode::serialize_into(
            &mut bytes,
            &Header {
                key,
                package: package.clone(),
                definitions,
            },
        )
        .map_err(corrupt)?;
        bytes.extend(body);
        Ok(bytes)
    }
}

/// A prechecked definition's revision node.
pub(crate) struct PrecheckedNodeRef(Arc<PrecheckedDefinition>);
impl std::ops::Deref for PrecheckedNodeRef {
    type Target = PrecheckedNode;
    fn deref(&self) -> &PrecheckedNode {
        &self.0.node
    }
}
