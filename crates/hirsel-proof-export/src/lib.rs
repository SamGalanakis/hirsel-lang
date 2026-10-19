//! The prover-neutral proof seam: stable obligation ids,
//! per-definition TIR digests and obligation checksums, and the canonical
//! `hirsel.tir.export.v4` text that consumers read out of process. Import
//! decodes, re-derives every signature fingerprint, recomputes every digest
//! and table, and re-validates each definition with V; any difference is a
//! located refusal.
mod content;
mod obligations;
pub mod sidecar;
pub mod wire;

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
use hirsel_types::encoding::{fingerprint, sealed};
use hirsel_types::{Signature, SignatureDependency};
use hirsel_validate::{Signatures, ValidatedSignatures};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::rc::Rc;

/// PS-8 format; bumped on TIR_VERSION or a ruled change, never on LIR churn.
pub const FORMAT: &str = "hirsel.tir.export.v4";
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
/// location table.
#[derive(Clone, Debug)]
pub struct Export {
    pub header: Header,
    pub definitions: Vec<DefinitionSection>,
    pub signatures: Vec<SignatureSection>,
    pub constants: Vec<ConstantSection>,
    pub skipped: Vec<Skipped>,
}

/// What a producer hands to [`export`].
pub struct ExportInput {
    pub header: Header,
    pub definitions: BTreeMap<DefPath, DefinitionRef>,
    pub signatures: BTreeMap<DefPath, SignatureSection>,
    pub constants: BTreeMap<DefPath, Materialized>,
    pub skipped: Vec<Skipped>,
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
fn refuse(at: impl Into<String>, message: impl Into<String>) -> ImportError {
    ImportError {
        at: at.into(),
        message: message.into(),
    }
}

impl Observe for Cx {
    fn def_path(&mut self, path: &DefPath) {
        if self.collecting {
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
    let mut cx = std::mem::replace(&mut w.context, Cx::new(locator));
    cx.begin_collect();
    let mut scratch = Writer::with_context(cx);
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

snapshot_struct!(Export {
    header,
    definitions,
    signatures,
    constants,
    skipped
} in Cx);

/// PS-5: the definition's TIR text under its own tables, the signature
/// fingerprints of every definition its TIR names (PS-6), and its callees.
struct Digested {
    locations: Vec<Location>,
    tir_digest: ContentHash,
    references: Vec<SignatureDependency>,
}

fn digest_definition(
    definition: &Definition,
    locator: &Locator,
    signatures: &BTreeMap<DefPath, SignatureSection>,
) -> Result<Digested, String> {
    let mut w = Writer::with_context(Cx::new(locator.clone()));
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
                    definition: path.clone(),
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
    definition: &Definition,
    locator: &Locator,
    signatures: &BTreeMap<DefPath, SignatureSection>,
    bodies: &BTreeSet<&DefPath>,
) -> Result<DefinitionSection, String> {
    let digested = digest_definition(definition, locator, signatures)?;
    let table = obligations::table(definition, |path| signatures.get(path).map(|s| &s.public));
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
        path: definition.path.clone(),
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
        match section(definition, &input.locator, &input.signatures, &bodies) {
            Ok(section) => definitions.push(section),
            Err(reason) => skipped.push(Skipped {
                name: path.clone(),
                reason,
            }),
        }
    }
    skipped.sort_by(|a, b| a.name.cmp(&b.name));
    Export {
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
    }
}

fn encode_body(export: &Export, locator: &Locator) -> String {
    let mut w = Writer::with_context(Cx::new(locator.clone()));
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

/// PS-8/PS-9: build and encode.
pub fn export(input: &ExportInput) -> String {
    encode(&build(input), &input.locator)
}

impl Export {
    /// A locator over the imported location tables (see [`Export`]).
    pub fn locator(&self) -> Locator {
        let tables: Vec<Vec<Location>> = self
            .definitions
            .iter()
            .map(|d| d.locations.clone())
            .collect();
        Rc::new(move |span| {
            tables
                .get(span.file.0 as usize)
                .and_then(|table| table.get(span.start as usize))
                .cloned()
                .unwrap_or_else(Location::unknown)
        })
    }
    /// The producer input that re-exports this export.
    pub fn to_input(&self) -> ExportInput {
        ExportInput {
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

/// PS-10: decode, check versions, re-derive every fingerprint and check its
/// uses, recompute every digest, obligation table and checksum, V, canonical
/// form and the export digest.
pub fn import(text: &str, expected: &Environment) -> Result<Export, ImportError> {
    let unlocated: Locator = Rc::new(|_| Location::unknown());
    let mut r = Reader::with_context(text, Cx::new(unlocated));
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
        let rebuilt = section(&stored.tir, &locator, &signatures, &bodies)
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
        if section.public.definition != section.path {
            return Err(refuse(format!("{at}.public"), "names another definition"));
        }
        if fingerprint(&section.public, &section.dependencies).as_ref()
            != Ok(&section.public.fingerprint)
        {
            return Err(refuse(
                format!("{at}.public.fingerprint"),
                "does not re-derive from the signature and its dependencies",
            ));
        }
        for dependency in &section.dependencies {
            if let Some(exported) = fingerprints.get(&dependency.definition)
                && *exported != dependency.fingerprint
            {
                return Err(refuse(
                    format!("{at}.dependencies"),
                    format!("{} is stale", dependency.definition),
                ));
            }
        }
        if let Some(view) = &section.owner_view
            && (view == &section.public || sealed(view) != section.public)
        {
            return Err(refuse(
                format!("{at}.owner_view"),
                "is not a view of the public signature",
            ));
        }
    }
    for definition in &export.definitions {
        let at = definition.path.to_string();
        if definition.tir.path != definition.path {
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
    let mut views: BTreeMap<&PackageId, ValidatedSignatures> = BTreeMap::new();
    for definition in &export.definitions {
        let package = &definition.path.0.package;
        let view = views.entry(package).or_insert_with(|| {
            let signatures: Signatures = export
                .signatures
                .iter()
                .map(|s| {
                    let owner = &s.path.0.package == package;
                    let signature = match (&s.owner_view, owner) {
                        (Some(view), true) => view,
                        _ => &s.public,
                    };
                    (s.path.clone(), signature.clone())
                })
                .collect();
            ValidatedSignatures::new(signatures)
        });
        let validation = view.validate(&definition.tir);
        if let Some(violation) = validation.violations.first() {
            return Err(refuse(
                definition.path.to_string(),
                format!("V rejects the definition: {violation:?}"),
            ));
        }
    }
    Ok(())
}
