//! The prover-neutral proof seam: stable obligation ids,
//! per-definition TIR digests and obligation checksums, and the canonical
//! `hirsel.tir.export.v14` text that consumers read out of process. Import
//! decodes, re-derives every signature fingerprint, recomputes every digest
//! and table, and re-validates each definition with V; any difference is a
//! located refusal.

#[cfg(test)]
hirsel_guard::install!(admission::tests::Counting: admission::tests::Counting);

mod admission;
mod annotations;
pub use admission::{
    BuildTablesLayout, CapturedTypes, build_captured_tables_bytes, build_owned_tables_bytes,
    build_owned_tables_layout, build_tables_bytes, capture_types, capture_types_bytes,
};
mod constants;
mod content;
pub mod findings;
#[cfg(test)]
mod format_tests;
mod obligations;
pub mod sidecar;
pub mod wire;

pub use annotations::{ANNOTATION_DIGEST_DOMAIN, Anchor, Annotation, annotation_digest};
pub use content::{
    CallSite, CallSiteKind, ContentReference, DEFINITION_HASH_DOMAIN, DefinitionContent,
    DefinitionGroup, call_sites, content_references, definition_groups, definition_hashes,
    group_hashes, references,
};
pub use obligations::{
    Detail, Obligation, ObligationId, ObligationKind, Site, SitePosition, projected_places,
};
pub use wire::{Cx, Locator};

use hirsel_codec::{DecodeError, Observe, Reader, Snapshot, Writer, line_column, snapshot_struct};
use hirsel_diag::RuleId;
use hirsel_syntax::{ContentHash, DefPath, PackageId};
use hirsel_tir::{Definition, DefinitionRef, Materialized, TIR_VERSION};
use hirsel_types::encoding::{canonical_encoding, fingerprint, sealed};
use hirsel_types::{Signature, SignatureDependency, TypeInterner};
use hirsel_validate::SignatureMap;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::rc::Rc;
use std::sync::Arc;

/// PS-8 format; bumped on TIR_VERSION or a ruled change, never on LIR churn.
/// v7: each definition, signature and materialized value carries its
/// canonical type table (first-encounter closure), not its producer's store.
/// v8: declared dictionary callable actuals and explicit constant alias edges.
/// v9: source scoped selectors and closure classification.
/// v10: independently checked fixed-origin borrow-cursor claims.
/// v11: contextual arena-scope grant ownership.
/// v12: combined grant encoding and logical invocation semantics.
/// v13: compile-time source-event charges (CT-4).
pub const FORMAT: &str = "hirsel.tir.export.v14";
/// PS-5.
pub const TIR_DIGEST_DOMAIN: &str = "hirsel.tir.digest.v1";
/// PS-6.
pub const CHECKSUM_DOMAIN: &str = "hirsel.obligation.checksum.v1";
/// PS-9: the digest over the whole export node.
pub const EXPORT_DIGEST_DOMAIN: &str = "hirsel.tir.export.digest.v1";

/// A portable source site: the module identity (`pkg@ver/mod`), one-based
/// lines and Unicode-scalar columns. Line 0 marks a span outside every source.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct Location {
    pub module: String,
    pub line: u32,
    pub column: u32,
    pub end_line: u32,
    pub end_column: u32,
}
impl Location {
    /// A span outside every source.
    pub fn unknown() -> Self {
        Self {
            module: String::new(),
            line: 0,
            column: 0,
            end_line: 0,
            end_column: 0,
        }
    }
}

/// The producing compiler, compared by exact equality on import.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Environment {
    pub interface_version: u32,
    pub compiler_version: String,
    pub spec_hash: ContentHash,
}
snapshot_struct!(Environment {
    interface_version,
    compiler_version,
    spec_hash
} in Cx);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Header {
    pub format: String,
    pub tir_version: u32,
    pub environment: Environment,
    pub entry: DefPath,
    /// The launch entry's semantic program revision.
    pub program_revision: Option<ContentHash>,
}
snapshot_struct!(Header {
    format,
    tir_version,
    environment,
    entry,
    program_revision
} in Cx);

/// A definition in the closure whose TIR is not exported (PS-9).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Skipped {
    pub name: DefPath,
    pub reason: String,
}
snapshot_struct!(Skipped { name, reason } in Cx);

/// One definition's section: its own location table, TIR, digest, the
/// signature fingerprints its checksums cover, the registry operations it
/// calls (callees with a signature and no body, exported or skipped) and its
/// obligations.
#[derive(Clone, Debug)]
pub struct DefinitionSection {
    pub path: DefPath,
    pub locations: Vec<Location>,
    pub tir: Definition,
    pub tir_digest: ContentHash,
    pub references: Vec<SignatureDependency>,
    pub operations: Vec<DefPath>,
    pub obligations: Vec<Obligation>,
}

/// A signature with the dependency list its fingerprint covers, and for an
/// opaque type the owner-package view that the owner's bodies see.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SignatureSection {
    pub path: DefPath,
    pub dependencies: Vec<SignatureDependency>,
    pub public: Signature,
    pub owner_view: Option<Signature>,
}

#[derive(Clone, Debug)]
pub struct ConstantSection {
    pub path: DefPath,
    pub value: Materialized,
}

/// The decoded export. Spans in imported TIR are location handles:
/// `file` is the definition section ordinal and `start == end` indexes its
/// location table. Its TypeIds are rows of `types`.
#[derive(Clone, Debug)]
pub struct Export {
    pub types: Arc<TypeInterner>,
    pub header: Header,
    pub definitions: Vec<DefinitionSection>,
    pub signatures: Vec<SignatureSection>,
    pub constants: Vec<ConstantSection>,
    pub skipped: Vec<Skipped>,
    /// PS-17, sorted by definition, then source order.
    pub annotations: Vec<Annotation>,
    pub annotation_digest: ContentHash,
}

/// What a producer hands to [`export`].
pub struct ExportInput {
    pub types: Arc<TypeInterner>,
    pub header: Header,
    pub definitions: BTreeMap<DefPath, DefinitionRef>,
    pub signatures: BTreeMap<DefPath, SignatureSection>,
    pub constants: BTreeMap<DefPath, Materialized>,
    pub skipped: Vec<Skipped>,
    pub annotations: Vec<Annotation>,
    pub locator: Locator,
}

/// A refused import: `at` is `line:column` in the text, or a definition path
/// and field.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportError {
    pub at: String,
    pub message: String,
}
impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.at, self.message)
    }
}
pub(crate) fn refuse(at: impl Into<String>, message: impl Into<String>) -> ImportError {
    ImportError {
        at: at.into(),
        message: message.into(),
    }
}

impl Observe for Cx {
    fn def_path(&mut self, path: &DefPath) {
        if self.capture_roots.is_some() {
            return;
        }
        if let Some(shape) = &mut self.sizing {
            shape.paths = shape.paths.checked_add(1).unwrap_or(usize::MAX);
            let payload = path
                .0
                .package
                .name
                .len()
                .saturating_add(path.0.package.version.len())
                .saturating_add(path.0.name.len())
                .saturating_add(
                    path.0
                        .modules
                        .len()
                        .saturating_mul(std::mem::size_of::<String>()),
                )
                .saturating_add(
                    path.0
                        .modules
                        .iter()
                        .fold(0usize, |n, s| n.saturating_add(s.len())),
                );
            shape.max_path_bytes = shape.max_path_bytes.max(payload);
            shape.path_bytes = shape.path_bytes.checked_add(payload).unwrap_or(usize::MAX);
        } else if self.collecting {
            self.references.insert(path.clone());
        }
    }
    fn reference(&mut self, path: &DefPath) -> Option<String> {
        let collecting = self.collecting;
        self.content
            .as_mut()
            .map(|naming| naming.reference(path, collecting))
    }
}

/// Run the collect pass of `write` on a scratch writer, then leave `w` ready
/// to emit with the finished section tables.
pub(crate) fn collect(w: &mut Writer<Cx>, write: impl Fn(&mut Writer<Cx>)) {
    let locator = w.context.locator.clone();
    let types = w.context.types.clone();
    let mut cx = std::mem::replace(&mut w.context, Cx::new(locator, types));
    cx.begin_collect();
    let mut scratch = Writer::discarding(cx);
    write(&mut scratch);
    let (_, mut cx) = scratch.finish();
    cx.begin_emit();
    w.context = cx;
}

/// Read a section's location table into the decoder context.
fn read_locations(r: &mut Reader<'_, Cx>) -> Result<Vec<Location>, DecodeError> {
    let locations = Vec::<Location>::read(r)?;
    r.context.locations = locations.clone();
    Ok(locations)
}

impl Snapshot<Cx> for DefinitionSection {
    fn write(&self, w: &mut Writer<Cx>) {
        let body = |w: &mut Writer<Cx>| {
            self.tir.write(w);
            self.tir_digest.write(w);
            self.references.write(w);
            self.operations.write(w);
            self.obligations.write(w);
        };
        collect(w, body);
        w.open("DefinitionSection");
        self.path.write(w);
        w.context.locations.clone().write(w);
        body(w);
        w.close();
        w.context.end_section();
    }
    fn read(r: &mut Reader<'_, Cx>) -> Result<Self, DecodeError> {
        r.open("DefinitionSection")?;
        let path = DefPath::read(r)?;
        let locations = read_locations(r)?;
        let value = Self {
            path,
            locations,
            tir: Definition::read(r)?,
            tir_digest: ContentHash::read(r)?,
            references: Vec::read(r)?,
            operations: Vec::read(r)?,
            obligations: Vec::read(r)?,
        };
        r.close()?;
        r.context.locations.clear();
        r.context.section += 1;
        Ok(value)
    }
}

impl Snapshot<Cx> for SignatureSection {
    fn write(&self, w: &mut Writer<Cx>) {
        let body = |w: &mut Writer<Cx>| {
            self.dependencies.write(w);
            self.public.write(w);
            self.owner_view.write(w);
        };
        collect(w, body);
        w.open("SignatureSection");
        self.path.write(w);
        w.context.locations.clone().write(w);
        body(w);
        w.close();
        w.context.end_section();
    }
    fn read(r: &mut Reader<'_, Cx>) -> Result<Self, DecodeError> {
        r.open("SignatureSection")?;
        let path = DefPath::read(r)?;
        read_locations(r)?;
        let value = Self {
            path,
            dependencies: Vec::read(r)?,
            public: Signature::read(r)?,
            owner_view: Option::read(r)?,
        };
        r.close()?;
        r.context.locations.clear();
        Ok(value)
    }
}

impl Snapshot<Cx> for ConstantSection {
    fn write(&self, w: &mut Writer<Cx>) {
        collect(w, |w| self.value.write(w));
        w.open("ConstantSection");
        self.path.write(w);
        w.context.locations.clone().write(w);
        self.value.write(w);
        w.close();
        w.context.end_section();
    }
    fn read(r: &mut Reader<'_, Cx>) -> Result<Self, DecodeError> {
        r.open("ConstantSection")?;
        let path = DefPath::read(r)?;
        read_locations(r)?;
        let value = Materialized::read(r)?;
        r.close()?;
        r.context.locations.clear();
        Ok(Self { path, value })
    }
}

impl Snapshot<Cx> for Export {
    fn write(&self, w: &mut Writer<Cx>) {
        w.open("Export");
        self.header.write(w);
        self.definitions.write(w);
        self.signatures.write(w);
        self.constants.write(w);
        self.skipped.write(w);
        self.annotations.write(w);
        self.annotation_digest.write(w);
        w.close();
    }
    fn read(r: &mut Reader<'_, Cx>) -> Result<Self, DecodeError> {
        r.open("Export")?;
        let value = Export {
            types: r.context.types.clone(),
            header: Snapshot::read(r)?,
            definitions: Snapshot::read(r)?,
            signatures: Snapshot::read(r)?,
            constants: Snapshot::read(r)?,
            skipped: Snapshot::read(r)?,
            annotations: Snapshot::read(r)?,
            annotation_digest: Snapshot::read(r)?,
        };
        r.close()?;
        Ok(value)
    }
}

/// PS-5: the definition's TIR text under its own tables, the signature
/// fingerprints of every definition its TIR names (PS-6), and its callees.
struct Digested {
    locations: Vec<Location>,
    tir_digest: ContentHash,
    references: Vec<SignatureDependency>,
}

fn digest_definition(
    types: &Arc<TypeInterner>,
    definition: &Definition,
    locator: &Locator,
    signatures: &BTreeMap<DefPath, SignatureSection>,
) -> Result<Digested, String> {
    let mut w = Writer::with_context(Cx::new(locator.clone(), types.clone()));
    collect(&mut w, |w| definition.write(w));
    definition.write(&mut w);
    let (text, cx) = w.finish();
    let tir_digest = ContentHash::of_parts(
        TIR_DIGEST_DOMAIN,
        [TIR_VERSION.to_le_bytes().as_slice(), text.as_bytes()],
    );
    let references = cx
        .references
        .iter()
        .map(|path| {
            signatures
                .get(path)
                .map(|s| SignatureDependency {
                    definition: hirsel_syntax::DefId::intern(path),
                    fingerprint: s.public.fingerprint,
                })
                .ok_or_else(|| format!("names {path} without an exported signature"))
        })
        .collect::<Result<_, _>>()?;
    Ok(Digested {
        locations: cx.locations,
        tir_digest,
        references,
    })
}

/// PS-6.
pub fn checksum(
    id: &ObligationId,
    tir_digest: &ContentHash,
    references: &[SignatureDependency],
) -> ContentHash {
    let id = id.to_string();
    let paths: Vec<String> = references
        .iter()
        .map(|r| r.definition.to_string())
        .collect();
    let mut parts: Vec<&[u8]> = vec![id.as_bytes(), tir_digest.0.as_slice()];
    for (path, reference) in paths.iter().zip(references) {
        parts.push(path.as_bytes());
        parts.push(reference.fingerprint.0.as_slice());
    }
    ContentHash::of_parts(CHECKSUM_DOMAIN, parts)
}

/// Build one section: digest, references, obligation table and checksums.
/// `bodies` names every definition with a body, exported or skipped; a callee
/// outside it with an exported signature is a registry operation.
fn section(
    types: &Arc<TypeInterner>,
    definition: &Definition,
    locator: &Locator,
    signatures: &BTreeMap<DefPath, SignatureSection>,
    bodies: &BTreeSet<&DefPath>,
) -> Result<DefinitionSection, String> {
    let digested = digest_definition(types, definition, locator, signatures)?;
    let table = obligations::table(types, definition, |path| {
        signatures.get(path).map(|s| &s.public)
    });
    let obligations = table
        .obligations
        .into_iter()
        .map(|(id, sites, detail)| Obligation {
            checksum: checksum(&id, &digested.tir_digest, &digested.references),
            id,
            sites,
            detail,
        })
        .collect();
    Ok(DefinitionSection {
        path: definition.path.path().clone(),
        locations: digested.locations,
        tir: definition.clone(),
        tir_digest: digested.tir_digest,
        references: digested.references,
        operations: table
            .callees
            .into_iter()
            .filter(|path| !bodies.contains(path) && signatures.contains_key(path))
            .collect(),
        obligations,
    })
}

fn section_owned(
    types: &Arc<TypeInterner>,
    definition: Definition,
    locator: &Locator,
    signatures: &BTreeMap<DefPath, SignatureSection>,
    bodies: &BTreeSet<DefPath>,
) -> Result<DefinitionSection, String> {
    let digested = digest_definition(types, &definition, locator, signatures)?;
    let table = obligations::table(types, &definition, |path| {
        signatures.get(path).map(|s| &s.public)
    });
    let obligations = table
        .obligations
        .into_iter()
        .map(|(id, sites, detail)| Obligation {
            checksum: checksum(&id, &digested.tir_digest, &digested.references),
            id,
            sites,
            detail,
        })
        .collect();
    Ok(DefinitionSection {
        path: definition.path.path().clone(),
        locations: digested.locations,
        tir: definition,
        tir_digest: digested.tir_digest,
        references: digested.references,
        operations: table
            .callees
            .into_iter()
            .filter(|path| !bodies.contains(path) && signatures.contains_key(path))
            .collect(),
        obligations,
    })
}

/// Build the export; a definition whose section cannot be built joins the
/// skip list with its reason.
pub fn build(input: &ExportInput) -> Export {
    let bodies: BTreeSet<&DefPath> = input
        .definitions
        .keys()
        .chain(input.skipped.iter().map(|s| &s.name))
        .collect();
    let mut skipped = input.skipped.clone();
    let mut definitions = vec![];
    for (path, definition) in &input.definitions {
        match section(
            &input.types,
            definition,
            &input.locator,
            &input.signatures,
            &bodies,
        ) {
            Ok(section) => definitions.push(section),
            Err(reason) => skipped.push(Skipped {
                name: path.clone(),
                reason,
            }),
        }
    }
    skipped.sort_by(|a, b| a.name.cmp(&b.name));
    Export {
        types: input.types.clone(),
        header: input.header.clone(),
        definitions,
        signatures: input.signatures.values().cloned().collect(),
        constants: input
            .constants
            .iter()
            .map(|(path, value)| ConstantSection {
                path: path.clone(),
                value: value.clone(),
            })
            .collect(),
        skipped,
        annotations: input.annotations.clone(),
        annotation_digest: annotation_digest(&input.annotations),
    }
}

/// Consume uniquely owned original artifacts and move their existing payloads
/// into the canonical export. Shared originals refuse before construction;
/// callers keep the input's retained-storage grant through the returned export.
/// No body or signature payload is cloned by this path.
pub fn build_owned(input: ExportInput) -> Option<Export> {
    if input
        .definitions
        .values()
        .any(|definition| Arc::strong_count(definition) != 1 || Arc::weak_count(definition) != 0)
    {
        return None;
    }
    let definitions_input = input.definitions;
    let bodies: BTreeSet<_> = definitions_input
        .keys()
        .chain(input.skipped.iter().map(|s| &s.name))
        .cloned()
        .collect();
    let mut owned = Vec::with_capacity(definitions_input.len());
    let mut skipped = input.skipped;
    for (path, definition) in definitions_input {
        let definition = Arc::try_unwrap(definition)
            .unwrap_or_else(|_| unreachable!("original's unique ownership was checked"));
        match section_owned(
            &input.types,
            definition,
            &input.locator,
            &input.signatures,
            &bodies,
        ) {
            Ok(section) => owned.push(section),
            Err(reason) => skipped.push(Skipped { name: path, reason }),
        }
    }
    skipped.sort_by(|a, b| a.name.cmp(&b.name));
    let annotation_digest = annotation_digest(&input.annotations);
    Some(Export {
        types: input.types,
        header: input.header,
        definitions: owned,
        signatures: input.signatures.into_values().collect(),
        constants: input
            .constants
            .into_iter()
            .map(|(path, value)| ConstantSection { path, value })
            .collect(),
        skipped,
        annotations: input.annotations,
        annotation_digest,
    })
}

fn encode_body(export: &Export, locator: &Locator) -> String {
    let mut w = Writer::with_context(Cx::new(locator.clone(), export.types.clone()));
    export.write(&mut w);
    w.finish().0
}

fn digest_node(body: &str) -> String {
    let digest = ContentHash::of_parts(EXPORT_DIGEST_DOMAIN, [body.as_bytes()]);
    let mut w = Writer::with_context(());
    w.open("ExportDigest");
    w.bytes(&digest.0);
    w.close();
    format!("\n{}\n", w.finish().0)
}

/// The canonical text: the export node, then its digest node.
pub fn encode(export: &Export, locator: &Locator) -> String {
    let body = encode_body(export, locator);
    let digest = digest_node(&body);
    body + &digest
}

/// Exact canonical output length, including the trailing digest node, without
/// allocating output text. The ordinary context table visitors still run and
/// need the caller's working memory; this does not build an ExportInput.
pub fn encoded_bytes(export: &Export, locator: &Locator) -> Option<usize> {
    let mut body = Writer::counting(Cx::new(locator.clone(), export.types.clone()));
    export.write(&mut body);
    let mut digest = Writer::counting(());
    digest.open("ExportDigest");
    digest.bytes(&[0; 32]);
    digest.close();
    body.output_bytes()?
        .checked_add(digest.output_bytes()?)?
        .checked_add(2)
}

/// Encode an already built export into a caller-admitted output byte buffer.
/// Refuses the whole result if its canonical text and trailing digest do not
/// fit. Export construction and context tables require separate working memory.
/// The limit includes UTF-8 escapes, whitespace and the complete digest node.
pub fn encode_with_output_limit(
    export: &Export,
    locator: &Locator,
    limit: usize,
) -> Option<String> {
    let mut writer =
        Writer::with_output_limit(Cx::new(locator.clone(), export.types.clone()), limit)?;
    export.write(&mut writer);
    let (body, _) = writer.finish_checked();
    let mut body = body?;
    let digest = digest_node(&body);
    if body.len().checked_add(digest.len())? > limit {
        return None;
    }
    body.push_str(&digest);
    Some(body)
}

/// Emit directly into the final exact-length Arc, retaining no String copy.
/// Context tables still require the caller's separate construction workspace.
pub fn encode_arc_with_output_limit(
    export: &Export,
    locator: &Locator,
    limit: usize,
) -> Option<Arc<str>> {
    let length = encoded_bytes(export, locator)?;
    if length > limit {
        return None;
    }
    let mut writer =
        Writer::with_exact_arc(Cx::new(locator.clone(), export.types.clone()), length)?;
    export.write(&mut writer);
    let digest = digest_node(writer.written_text()?);
    writer.append_raw(&digest);
    writer.finish_arc_checked().0
}

/// PS-8/PS-9: build and encode.
pub fn export(input: &ExportInput) -> String {
    encode(&build(input), &input.locator)
}

impl Export {
    /// The digest of the trailing `(ExportDigest …)` node (PS-9).
    pub fn digest(&self) -> ContentHash {
        let body = encode_body(self, &self.locator());
        ContentHash::of_parts(EXPORT_DIGEST_DOMAIN, [body.as_bytes()])
    }
    /// A locator over the imported location tables (see [`Export`]).
    pub fn locator(&self) -> Locator {
        let tables: Vec<Vec<Location>> = self
            .definitions
            .iter()
            .map(|d| d.locations.clone())
            .collect();
        Rc::new(move |span| {
            tables
                .get(span.anchor.0 as usize)
                .and_then(|table| table.get(span.start as usize))
                .cloned()
                .unwrap_or_else(Location::unknown)
        })
    }
    /// The producer input that re-exports this export.
    pub fn to_input(&self) -> ExportInput {
        ExportInput {
            types: self.types.clone(),
            header: self.header.clone(),
            definitions: self
                .definitions
                .iter()
                .map(|d| (d.path.clone(), std::sync::Arc::new(d.tir.clone())))
                .collect(),
            signatures: self
                .signatures
                .iter()
                .map(|s| (s.path.clone(), s.clone()))
                .collect(),
            constants: self
                .constants
                .iter()
                .map(|c| (c.path.clone(), c.value.clone()))
                .collect(),
            skipped: self.skipped.clone(),
            annotations: self.annotations.clone(),
            locator: self.locator(),
        }
    }
    pub fn definition(&self, path: &DefPath) -> Option<&DefinitionSection> {
        self.definitions.iter().find(|d| &d.path == path)
    }
    /// Attach a run-time failure to obligations without touching `Failure`:
    /// every obligation of `path` with `rule` that has a site starting at
    /// `line` and `column` (an observed failure's operation site when present,
    /// otherwise its stable primary site). MEM-13/14
    /// report at the first instruction touching the place, so several
    /// candidates may match (G7).
    pub fn failure_obligations(
        &self,
        path: &DefPath,
        rule: &RuleId,
        line: u32,
        column: u32,
    ) -> Vec<&ObligationId> {
        let Some(section) = self.definition(path) else {
            return vec![];
        };
        section
            .obligations
            .iter()
            .filter(|o| &o.id.rule == rule)
            .filter(|o| {
                o.sites.iter().any(|s| {
                    section
                        .locations
                        .get(s.span.start as usize)
                        .is_some_and(|l| l.line == line && l.column == column)
                })
            })
            .map(|o| &o.id)
            .collect()
    }
}

fn located(text: &str, error: DecodeError) -> ImportError {
    let (line, column) = line_column(text, error.offset);
    refuse(format!("{line}:{column}"), error.message)
}

/// The export's header alone, for a consumer that pins the producing
/// compiler before importing (PS-7): `import(text, &header.environment)`.
pub fn read_header(text: &str) -> Result<Header, ImportError> {
    let unlocated: Locator = Rc::new(|_| Location::unknown());
    let mut r = Reader::with_context(text, Cx::new(unlocated, TypeInterner::new(None)));
    r.open("Export")
        .and_then(|()| Header::read(&mut r))
        .map_err(|e| located(text, e))
}

/// PS-10: decode, check versions, re-derive every fingerprint and check its
/// uses, recompute every digest, obligation table and checksum, V, canonical
/// form and the export digest. Imported rows are interned into `types`.
pub fn import(
    text: &str,
    expected: &Environment,
    types: &Arc<TypeInterner>,
) -> Result<Export, ImportError> {
    let unlocated: Locator = Rc::new(|_| Location::unknown());
    let mut r = Reader::with_context(text, Cx::new(unlocated, types.clone()));
    let export = Export::read(&mut r).map_err(|e| located(text, e))?;
    let body_end = r.offset();
    let digest = (|| -> Result<ContentHash, DecodeError> {
        r.open("ExportDigest")?;
        let digest = ContentHash::read(&mut r)?;
        r.close()?;
        r.finish()?;
        Ok(digest)
    })()
    .map_err(|e| located(text, e))?;
    let header = &export.header;
    if header.format != FORMAT {
        return Err(refuse(
            "header.format",
            format!("format {:?} is not {FORMAT}", header.format),
        ));
    }
    if header.tir_version != TIR_VERSION {
        return Err(refuse(
            "header.tir_version",
            format!("TIR version {} is not {TIR_VERSION}", header.tir_version),
        ));
    }
    if &header.environment != expected {
        return Err(refuse(
            "header.environment",
            "compiler, interface or spec version differs from the importer's",
        ));
    }
    // Checks run from the most to the least located: signatures, then each
    // definition's recomputed digest and table, then V, then the canonical
    // form and the export digest over everything.
    verify_signatures(&export)?;
    let locator = export.locator();
    let signatures: BTreeMap<DefPath, SignatureSection> = export
        .signatures
        .iter()
        .map(|s| (s.path.clone(), s.clone()))
        .collect();
    let bodies: BTreeSet<&DefPath> = export
        .definitions
        .iter()
        .map(|d| &d.path)
        .chain(export.skipped.iter().map(|s| &s.name))
        .collect();
    for stored in &export.definitions {
        let at = stored.path.to_string();
        let rebuilt = section(types, &stored.tir, &locator, &signatures, &bodies)
            .map_err(|reason| refuse(&at, reason))?;
        if rebuilt.tir_digest != stored.tir_digest {
            return Err(refuse(format!("{at}.tir_digest"), "does not match its TIR"));
        }
        if rebuilt.references != stored.references {
            return Err(refuse(format!("{at}.references"), "does not match its TIR"));
        }
        if rebuilt.operations != stored.operations {
            return Err(refuse(format!("{at}.operations"), "does not match its TIR"));
        }
        if rebuilt.obligations != stored.obligations {
            return Err(refuse(
                format!("{at}.obligations"),
                "obligation table or checksum does not match its TIR",
            ));
        }
    }
    annotations::verify(&export)?;
    validate(&export)?;
    let body = encode_body(&export, &locator);
    if body != text[..body_end] {
        return Err(refuse("export", "text is not in canonical form"));
    }
    if digest != ContentHash::of_parts(EXPORT_DIGEST_DOMAIN, [body.as_bytes()]) {
        return Err(refuse("export digest", "does not match the export"));
    }
    if text[body_end..] != digest_node(&body) {
        return Err(refuse("export digest", "is not in canonical form"));
    }
    Ok(export)
}

/// Whether the owner `view` seals to `section`'s public signature. A checked
/// serializable opt-in publishes its representation graph (SER-1); the owner's copy
/// of that graph has its own row provenance, so the two compare by their canonical
/// encoding, the identity the fingerprint covers.
fn seals_to(types: &TypeInterner, view: &Signature, section: &SignatureSection) -> bool {
    let sealed = sealed(view);
    let public = &section.public;
    if sealed == *public {
        return true;
    }
    let checked = matches!(&sealed.kind,
        hirsel_types::SignatureKind::Type(t) if t.serializable_check.is_some());
    checked
        && sealed.definition == public.definition
        && sealed.metadata == public.metadata
        && sealed.fingerprint == public.fingerprint
        && canonical_encoding(types, &sealed, &section.dependencies)
            .ok()
            .is_some_and(|bytes| {
                canonical_encoding(types, public, &section.dependencies).ok() == Some(bytes)
            })
}

/// Every signature fingerprint (declaration, fact helper, registry row) has
/// one derivation, its encoding over the dependency list it covers, so the
/// importer re-derives each one; an owner view is consistent exactly when it
/// seals to its public signature. Then every use must name those fingerprints.
fn verify_signatures(export: &Export) -> Result<(), ImportError> {
    let fingerprints: BTreeMap<&DefPath, ContentHash> = export
        .signatures
        .iter()
        .map(|s| (&s.path, s.public.fingerprint))
        .collect();
    for section in &export.signatures {
        let at = section.path.to_string();
        if *section.public.definition.path() != section.path {
            return Err(refuse(format!("{at}.public"), "names another definition"));
        }
        if fingerprint(&export.types, &section.public, &section.dependencies).as_ref()
            != Ok(&section.public.fingerprint)
        {
            return Err(refuse(
                format!("{at}.public.fingerprint"),
                "does not re-derive from the signature and its dependencies",
            ));
        }
        for dependency in &section.dependencies {
            if let Some(exported) = fingerprints.get(dependency.definition.path())
                && *exported != dependency.fingerprint
            {
                return Err(refuse(
                    format!("{at}.dependencies"),
                    format!("{} is stale", dependency.definition),
                ));
            }
        }
        if let Some(view) = &section.owner_view
            && (view == &section.public || !seals_to(&export.types, view, section))
        {
            return Err(refuse(
                format!("{at}.owner_view"),
                "is not a view of the public signature",
            ));
        }
    }
    for definition in &export.definitions {
        let at = definition.path.to_string();
        if *definition.tir.path.path() != definition.path {
            return Err(refuse(format!("{at}.tir"), "names another definition"));
        }
        if fingerprints.get(&definition.path) != Some(&definition.tir.signature_hash) {
            return Err(refuse(
                format!("{at}.signature_hash"),
                "is not its signature's fingerprint",
            ));
        }
    }
    Ok(())
}

/// V over every definition against its package's signature view.
fn validate(export: &Export) -> Result<(), ImportError> {
    let mut views: BTreeMap<&PackageId, SignatureMap> = BTreeMap::new();
    for definition in &export.definitions {
        let package = &definition.path.0.package;
        let view = views.entry(package).or_insert_with(|| {
            let mut signatures = SignatureMap::new(export.types.clone());
            signatures.extend(export.signatures.iter().map(|s| {
                let owner = &s.path.0.package == package;
                let signature = match (&s.owner_view, owner) {
                    (Some(view), true) => view,
                    _ => &s.public,
                };
                (&s.path, signature.clone())
            }));
            signatures
        });
        let validation =
            hirsel_validate::validate(&definition.tir, &export.types, view, &mut vec![]);
        if let Some(violation) = validation.violations.first() {
            return Err(refuse(
                definition.path.to_string(),
                format!("V rejects the definition: {violation:?}"),
            ));
        }
    }
    constants::validate(export)
}
