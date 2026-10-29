//! Schema-specific indexed Lower TIR. Opening validates the archive shape and
//! each row's canonical scalar encoding. Consumer source/type binding happens
//! separately, and neither operation supplies V acceptance.
use crate::{BoundIndexedEnvelope, IndexedEnvelope, TypeRecords, TypeRef};
use hirsel_artifact_store::{Bytes, MAX_RECORD, Reference};
use hirsel_span::{Anchor, AnchorCodec, Span};
use hirsel_syntax::{ContentHash, DefId, DefPath};
use hirsel_tir::*;
use hirsel_types::{TableBuilder, TypeId};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::sync::Arc;

const MAGIC: &[u8; 8] = b"HIRTIR01";
const HEADER: usize = 24;
const EXTENT: usize = 16;
const ROOT: u32 = 1;
const BODY: u32 = 2;
const BLOCK: u32 = 3;
const MATCH: u32 = 4;
const CALLERS: u32 = 5;
const ASSOCIATIONS: u32 = 6;
const TYPES: u32 = 7;
const BODIES: u32 = 8;
const CONTRACTS: u32 = 9;
const LOCALS: u32 = 10;
const PLACES: u32 = 11;
const SCOPES: u32 = 12;
const BLOCKS: u32 = 13;
const RVALUES: u32 = 14;
const CLEANUPS: u32 = 15;
const CLEANUP_NODES: u32 = 16;
const PARAMETERS: u32 = 17;
const CURSORS: u32 = 18;
const TAILS: u32 = 19;
const MATCHES: u32 = 20;
const SCOPED: u32 = 21;
const INSTRUCTIONS: u32 = 22;
const PATTERNS: u32 = 23;
const ARMS: u32 = 24;

fn number(bytes: &[u8], at: usize) -> Option<usize> {
    u64::from_le_bytes(bytes.get(at..at.checked_add(8)?)?.try_into().ok()?)
        .try_into()
        .ok()
}
fn word(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}
fn table(tag: u32, rows: impl IntoIterator<Item = Vec<u8>>) -> Option<Vec<u8>> {
    let rows: Vec<_> = rows.into_iter().collect();
    let start = HEADER.checked_add(rows.len().checked_mul(EXTENT)?)?;
    let total = rows
        .iter()
        .try_fold(start, |n, row| n.checked_add(row.len()))?;
    if total > MAX_RECORD {
        return None;
    }
    let mut bytes = vec![0; start];
    bytes[..8].copy_from_slice(MAGIC);
    bytes[8..12].copy_from_slice(&tag.to_le_bytes());
    bytes[12..16].copy_from_slice(&u32::try_from(rows.len()).ok()?.to_le_bytes());
    bytes[16..24].copy_from_slice(&u64::try_from(total).ok()?.to_le_bytes());
    for (n, row) in rows.into_iter().enumerate() {
        let at = HEADER + n * EXTENT;
        let start = bytes.len();
        bytes[at..at + 8].copy_from_slice(&u64::try_from(start).ok()?.to_le_bytes());
        bytes[at + 8..at + 16].copy_from_slice(&u64::try_from(row.len()).ok()?.to_le_bytes());
        bytes.extend(row);
    }
    Some(bytes)
}
#[derive(Clone, Copy)]
struct Rows<'a> {
    bytes: &'a [u8],
    count: u32,
}
impl<'a> Rows<'a> {
    fn open(bytes: &'a [u8], tag: u32) -> Option<Self> {
        if bytes.get(..8)? != MAGIC || word(bytes, 8)? != tag || number(bytes, 16)? != bytes.len() {
            return None;
        }
        let count = word(bytes, 12)?;
        let mut cursor = HEADER.checked_add((count as usize).checked_mul(EXTENT)?)?;
        if cursor > bytes.len() {
            return None;
        }
        for n in 0..count as usize {
            let at = HEADER + n * EXTENT;
            if number(bytes, at)? != cursor {
                return None;
            }
            cursor = cursor.checked_add(number(bytes, at + 8)?)?;
            if cursor > bytes.len() {
                return None;
            }
        }
        (cursor == bytes.len()).then_some(Self { bytes, count })
    }
    fn row(self, n: u32) -> Option<&'a [u8]> {
        if n >= self.count {
            return None;
        }
        let at = HEADER + n as usize * EXTENT;
        let start = number(self.bytes, at)?;
        self.bytes
            .get(start..start.checked_add(number(self.bytes, at + 8)?)?)
    }
    fn section(self, n: u32, tag: u32) -> Option<Self> {
        Self::open(self.row(n)?, tag)
    }
    // Only CheckedTir and its borrowed rows use this projection. Opening the
    // immutable owner already checked every extent and nested section. Keep
    // demand proportional to the selected row, not the containing array size.
    fn checked(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            count: word(bytes, 12).expect("checked TIR count"),
        }
    }
    fn checked_section(self, n: u32) -> Option<Self> {
        Some(Self::checked(self.row(n)?))
    }
}

/// Complete scalar definition metadata. Body and type arrays remain indexed.
#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct TirHeader {
    pub version: u32,
    pub inferred_unique: hirsel_types::UniqueSummary,
    pub path: DefId,
    pub signature_hash: ContentHash,
    pub entry: BodyId,
}
#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct TirBodyHeader {
    pub arena_scope_grant: Option<ArenaScopeGrant>,
    pub parameter_scope: hirsel_types::SiteRef,
    pub entry: BlockId,
    pub result: TypeId,
}
fn encode<T: Serialize>(value: &T) -> Option<Vec<u8>> {
    bincode::serialize(value).ok()
}
fn array<T: Serialize>(tag: u32, values: &[T]) -> Option<Vec<u8>> {
    table(tag, values.iter().map(encode).collect::<Option<Vec<_>>>()?)
}
fn encode_block(block: &Block) -> Option<Vec<u8>> {
    table(
        BLOCK,
        [
            encode(&block.span)?,
            array(INSTRUCTIONS, &block.instructions)?,
            encode(&block.terminator)?,
        ],
    )
}
fn encode_match(value: &MatchTable) -> Option<Vec<u8>> {
    table(
        MATCH,
        [
            encode(&value.scrutinee)?,
            array(PATTERNS, &value.patterns)?,
            array(ARMS, &value.arms)?,
        ],
    )
}
fn encode_body(body: &Body) -> Option<Vec<u8>> {
    table(
        BODY,
        [
            encode(&TirBodyHeader {
                arena_scope_grant: body.arena_scope_grant.clone(),
                parameter_scope: body.parameter_scope,
                entry: body.entry,
                result: body.result,
            })?,
            array(LOCALS, &body.locals)?,
            array(PLACES, &body.places)?,
            array(SCOPES, &body.scopes)?,
            table(
                BLOCKS,
                body.blocks
                    .iter()
                    .map(encode_block)
                    .collect::<Option<Vec<_>>>()?,
            )?,
            array(RVALUES, &body.rvalues)?,
            array(CLEANUPS, &body.cleanups)?,
            array(CLEANUP_NODES, &body.cleanup_nodes)?,
            array(PARAMETERS, &body.parameters)?,
            array(CURSORS, &body.borrow_cursors)?,
            array(TAILS, &body.tail_sites)?,
            table(
                MATCHES,
                body.matches
                    .iter()
                    .map(encode_match)
                    .collect::<Option<Vec<_>>>()?,
            )?,
            array(SCOPED, &body.scoped_parameters)?,
        ],
    )
}
fn encode_definition(definition: &Definition) -> Option<Vec<u8>> {
    table(
        ROOT,
        [
            encode(&TirHeader {
                version: TIR_VERSION,
                inferred_unique: definition.inferred_unique.clone(),
                path: definition.path,
                signature_hash: definition.signature_hash,
                entry: definition.entry,
            })?,
            array(CALLERS, &definition.caller_bodies)?,
            array(ASSOCIATIONS, &definition.protocol_associations)?,
            array(TYPES, &definition.types)?,
            table(
                BODIES,
                definition
                    .bodies
                    .iter()
                    .map(encode_body)
                    .collect::<Option<Vec<_>>>()?,
            )?,
            array(CONTRACTS, &definition.contracts)?,
        ],
    )
}
impl TypeRecords {
    /// Materialize the complete required type closure and one indexed TIR
    /// envelope. Persistence associations remain the caller's atomic publish.
    pub fn encode_tir(
        &self,
        definition: &Definition,
        owner: impl FnMut(Anchor) -> Option<(DefPath, u32)>,
    ) -> Option<Vec<u8>> {
        let mut types = TableBuilder::new(self.types.clone());
        let mut anchors = AnchorCodec::Collect(vec![]);
        // Collect the semantic first-use order before portable-equivalent
        // children are merged. Pool sharing cannot choose archive positions.
        drop(types.with_encoding(|| {
            hirsel_span::with_anchor_codec(&mut anchors, || {
                crate::indexed_types::inline_names(|| encode_definition(definition))
            })
        })?);
        self.indexed_record(types.roots(), anchors, owner, || {
            encode_definition(definition)
        })
        .map(|(bytes, _)| bytes)
    }
}

fn raw<R>(f: impl FnOnce() -> R) -> R {
    crate::indexed_types::inline_names(|| {
        hirsel_types::with_session_ids(|| hirsel_span::with_anchor_codec(&mut AnchorCodec::Raw, f))
    })
}
struct Validation<'a> {
    envelope: &'a IndexedEnvelope,
    next_type: u32,
    next_anchor: u32,
}
fn scalar<T: DeserializeOwned + Serialize>(
    bytes: &[u8],
    validation: &mut Validation<'_>,
) -> Option<()> {
    let value: T = raw(|| crate::options(bytes.len()).deserialize(bytes)).ok()?;
    if raw(|| encode(&value))?.as_slice() != bytes {
        return None;
    }
    // These scopes collect every referenced position without resolving a child
    // or interning a graph. Only a demanded typed read opens child components.
    let mut anchors = AnchorCodec::Collect(vec![]);
    let (serialized, types) = hirsel_types::with_table_collection(|| {
        hirsel_span::with_anchor_codec(&mut anchors, || {
            crate::indexed_types::inline_names(|| bincode::serialize_into(std::io::sink(), &value))
        })
    });
    serialized.ok()?;
    let AnchorCodec::Collect(anchors) = anchors else {
        unreachable!()
    };
    for anchor in anchors {
        if anchor.0 > validation.envelope.anchor_count()
            || anchor.0 > validation.next_anchor.checked_add(1)?
        {
            return None;
        }
        if anchor.0 == validation.next_anchor + 1 {
            validation.next_anchor += 1;
        }
    }
    for ty in types {
        let bytes = hirsel_types::with_session_ids(|| bincode::serialize(&ty)).ok()?;
        let position = u32::from_le_bytes(bytes.try_into().ok()?);
        if position >= validation.envelope.type_count() || position > validation.next_type {
            return None;
        }
        if position == validation.next_type {
            validation.next_type += 1;
        }
    }
    Some(())
}
fn check_array<T: DeserializeOwned + Serialize>(
    rows: Rows<'_>,
    envelope: &mut Validation<'_>,
) -> Option<()> {
    for n in 0..rows.count {
        scalar::<T>(rows.row(n)?, envelope)?;
    }
    Some(())
}
fn check_match(rows: Rows<'_>, envelope: &mut Validation<'_>) -> Option<()> {
    if rows.count != 3 {
        return None;
    }
    scalar::<PlaceId>(rows.row(0)?, envelope)?;
    check_array::<MatchPattern>(rows.section(1, PATTERNS)?, envelope)?;
    check_array::<MatchArm>(rows.section(2, ARMS)?, envelope)
}
fn check_block(rows: Rows<'_>, envelope: &mut Validation<'_>) -> Option<()> {
    if rows.count != 3 {
        return None;
    }
    scalar::<Span>(rows.row(0)?, envelope)?;
    check_array::<Instruction>(rows.section(1, INSTRUCTIONS)?, envelope)?;
    scalar::<Terminator>(rows.row(2)?, envelope)
}
fn check_body(rows: Rows<'_>, envelope: &mut Validation<'_>) -> Option<()> {
    if rows.count != 13 {
        return None;
    }
    scalar::<TirBodyHeader>(rows.row(0)?, envelope)?;
    check_array::<Local>(rows.section(1, LOCALS)?, envelope)?;
    check_array::<Place>(rows.section(2, PLACES)?, envelope)?;
    check_array::<Scope>(rows.section(3, SCOPES)?, envelope)?;
    let blocks = rows.section(4, BLOCKS)?;
    for n in 0..blocks.count {
        check_block(Rows::open(blocks.row(n)?, BLOCK)?, envelope)?;
    }
    check_array::<Rvalue>(rows.section(5, RVALUES)?, envelope)?;
    check_array::<Cleanup>(rows.section(6, CLEANUPS)?, envelope)?;
    check_array::<CleanupNode>(rows.section(7, CLEANUP_NODES)?, envelope)?;
    check_array::<LocalId>(rows.section(8, PARAMETERS)?, envelope)?;
    check_array::<BorrowCursor>(rows.section(9, CURSORS)?, envelope)?;
    check_array::<TailSite>(rows.section(10, TAILS)?, envelope)?;
    let matches = rows.section(11, MATCHES)?;
    for n in 0..matches.count {
        check_match(Rows::open(matches.row(n)?, MATCH)?, envelope)?;
    }
    check_array::<ScopedParameter>(rows.section(12, SCOPED)?, envelope)?;
    Some(())
}

/// An exact-content checked, structurally validated Lower archive. Small row
/// views retain its complete immutable source, type, backing and admission owners.
#[derive(Clone)]
pub struct CheckedTir {
    source: BoundIndexedEnvelope,
    payload: Bytes,
}
impl CheckedTir {
    pub const DOMAIN: hirsel_artifact_store::Domain = hirsel_artifact_store::Domain {
        kind: 0x4e00_0024,
        schema: 2,
    };
    pub fn open(
        bytes: Bytes,
        expected: Reference,
        records: Arc<TypeRecords>,
        anchor: impl FnMut(&DefPath, u32) -> Option<Anchor>,
    ) -> Option<Self> {
        if expected.domain != Self::DOMAIN {
            return None;
        }
        let envelope = IndexedEnvelope::open(bytes, expected)?;
        let payload = envelope.payload();
        let mut references = std::collections::HashSet::new();
        for n in 0..envelope.type_count() {
            if !references.insert(envelope.type_reference(n)?) {
                return None;
            }
        }
        let mut owners = std::collections::HashSet::new();
        for n in 0..envelope.anchor_count() {
            if !owners.insert(envelope.anchor(n)?.to_owned()) {
                return None;
            }
        }
        let mut validation = Validation {
            envelope: &envelope,
            next_type: 0,
            next_anchor: 0,
        };
        let rows = Rows::open(&payload, ROOT)?;
        if rows.count != 6 {
            return None;
        }
        scalar::<TirHeader>(rows.row(0)?, &mut validation)?;
        let header: TirHeader = raw(|| {
            crate::options(rows.row(0).expect("checked header").len())
                .deserialize(rows.row(0).expect("checked header"))
        })
        .ok()?;
        if header.version != TIR_VERSION {
            return None;
        }
        check_array::<BodyId>(rows.section(1, CALLERS)?, &mut validation)?;
        check_array::<ProtocolAssociationReceipt>(rows.section(2, ASSOCIATIONS)?, &mut validation)?;
        check_array::<TypeId>(rows.section(3, TYPES)?, &mut validation)?;
        let bodies = rows.section(4, BODIES)?;
        for n in 0..bodies.count {
            check_body(Rows::open(bodies.row(n)?, BODY)?, &mut validation)?;
        }
        check_array::<(u32, BodyId)>(rows.section(5, CONTRACTS)?, &mut validation)?;
        if validation.next_type != envelope.type_count()
            || validation.next_anchor != envelope.anchor_count()
        {
            return None;
        }
        Some(Self {
            source: envelope.bind(records, anchor)?,
            payload,
        })
    }
    fn root(&self) -> Rows<'_> {
        Rows::checked(&self.payload)
    }
    pub fn backing(&self) -> &Bytes {
        self.source.envelope().backing()
    }
    pub fn reference(&self) -> Reference {
        self.source.envelope().reference()
    }
    pub fn header(&self) -> Option<TirHeader> {
        self.source.decode(self.root().row(0)?)
    }
    pub fn body_count(&self) -> u32 {
        self.root()
            .checked_section(4)
            .expect("checked bodies")
            .count
    }
    pub fn body(&self, n: u32) -> Option<TirBodyRows<'_>> {
        let rows = self.root().checked_section(4)?;
        Some(TirBodyRows {
            owner: self,
            rows: Rows::checked(rows.row(n)?),
        })
    }
    pub fn type_count(&self) -> u32 {
        self.root().checked_section(3).expect("checked types").count
    }
    pub fn type_reference(&self, n: u32) -> Option<TypeRef> {
        let row = self.root().checked_section(3)?.row(n)?;
        self.source.envelope().type_reference(word(row, 0)?)
    }
    pub fn type_id(&self, n: u32) -> Option<TypeId> {
        self.source.decode(self.root().checked_section(3)?.row(n)?)
    }
    pub fn caller_body_count(&self) -> u32 {
        self.root()
            .checked_section(1)
            .expect("checked callers")
            .count
    }
    pub fn caller_body(&self, n: u32) -> Option<BodyId> {
        self.source.decode(self.root().checked_section(1)?.row(n)?)
    }
    pub fn protocol_association_count(&self) -> u32 {
        self.root()
            .checked_section(2)
            .expect("checked associations")
            .count
    }
    pub fn protocol_association(&self, n: u32) -> Option<ProtocolAssociationReceipt> {
        self.source
            .decode(self.root().checked_section(2)?.row(n)?)
    }
    pub fn contract_count(&self) -> u32 {
        self.root()
            .checked_section(5)
            .expect("checked contracts")
            .count
    }
    pub fn contract(&self, n: u32) -> Option<(u32, BodyId)> {
        self.source
            .decode(self.root().checked_section(5)?.row(n)?)
    }
    pub fn caller_bodies(&self) -> Option<Vec<BodyId>> {
        self.collect(self.root().checked_section(1)?)
    }
    pub fn protocol_associations(&self) -> Option<Vec<ProtocolAssociationReceipt>> {
        self.collect(self.root().checked_section(2)?)
    }
    pub fn contracts(&self) -> Option<Vec<(u32, BodyId)>> {
        self.collect(self.root().checked_section(5)?)
    }
    fn collect<T: DeserializeOwned>(&self, rows: Rows<'_>) -> Option<Vec<T>> {
        (0..rows.count)
            .map(|n| self.source.decode(rows.row(n)?))
            .collect()
    }
    /// Explicit transformation boundary. Read-only row access does not call it.
    pub fn materialize(&self) -> Option<Definition> {
        let header = self.header()?;
        Some(Definition {
            inferred_unique: header.inferred_unique,
            path: header.path,
            signature_hash: header.signature_hash,
            entry: header.entry,
            caller_bodies: self.caller_bodies()?,
            protocol_associations: self.protocol_associations()?,
            types: self
                .collect::<TypeId>(self.root().checked_section(3)?)?
                .into(),
            bodies: (0..self.body_count())
                .map(|n| self.body(n)?.materialize())
                .collect::<Option<_>>()?,
            contracts: self.contracts()?,
        })
    }
}
#[derive(Clone, Copy)]
pub struct TirBodyRows<'a> {
    owner: &'a CheckedTir,
    rows: Rows<'a>,
}
impl<'a> TirBodyRows<'a> {
    pub fn header(self) -> Option<TirBodyHeader> {
        self.owner.source.decode(self.rows.row(0)?)
    }
    pub fn local_count(self) -> u32 {
        self.rows.checked_section(1).expect("checked locals").count
    }
    pub fn local(self, n: u32) -> Option<Local> {
        self.owner
            .source
            .decode(self.rows.checked_section(1)?.row(n)?)
    }
    pub fn place_count(self) -> u32 {
        self.rows.checked_section(2).expect("checked places").count
    }
    pub fn place(self, n: u32) -> Option<Place> {
        self.owner
            .source
            .decode(self.rows.checked_section(2)?.row(n)?)
    }
    pub fn scope_count(self) -> u32 {
        self.rows.checked_section(3).expect("checked scopes").count
    }
    pub fn scope(self, n: u32) -> Option<Scope> {
        self.owner
            .source
            .decode(self.rows.checked_section(3)?.row(n)?)
    }
    pub fn block_count(self) -> u32 {
        self.rows.checked_section(4).expect("checked blocks").count
    }
    pub fn block(self, n: u32) -> Option<TirBlockRows<'a>> {
        Some(TirBlockRows {
            owner: self.owner,
            rows: Rows::checked(self.rows.checked_section(4)?.row(n)?),
        })
    }
    pub fn rvalue_count(self) -> u32 {
        self.rows
            .checked_section(5)
            .expect("checked rvalues")
            .count
    }
    pub fn rvalue(self, n: u32) -> Option<Rvalue> {
        self.owner
            .source
            .decode(self.rows.checked_section(5)?.row(n)?)
    }
    pub fn cleanup_count(self) -> u32 {
        self.rows
            .checked_section(6)
            .expect("checked cleanups")
            .count
    }
    pub fn cleanup(self, n: u32) -> Option<Cleanup> {
        self.owner
            .source
            .decode(self.rows.checked_section(6)?.row(n)?)
    }
    pub fn cleanup_node_count(self) -> u32 {
        self.rows
            .checked_section(7)
            .expect("checked cleanup_nodes")
            .count
    }
    pub fn cleanup_node(self, n: u32) -> Option<CleanupNode> {
        self.owner
            .source
            .decode(self.rows.checked_section(7)?.row(n)?)
    }
    pub fn parameter_count(self) -> u32 {
        self.rows
            .checked_section(8)
            .expect("checked parameters")
            .count
    }
    pub fn parameter(self, n: u32) -> Option<LocalId> {
        self.owner
            .source
            .decode(self.rows.checked_section(8)?.row(n)?)
    }
    pub fn borrow_cursor_count(self) -> u32 {
        self.rows
            .checked_section(9)
            .expect("checked borrow_cursors")
            .count
    }
    pub fn borrow_cursor(self, n: u32) -> Option<BorrowCursor> {
        self.owner
            .source
            .decode(self.rows.checked_section(9)?.row(n)?)
    }
    pub fn tail_site_count(self) -> u32 {
        self.rows
            .checked_section(10)
            .expect("checked tail_sites")
            .count
    }
    pub fn tail_site(self, n: u32) -> Option<TailSite> {
        self.owner
            .source
            .decode(self.rows.checked_section(10)?.row(n)?)
    }
    pub fn match_table_count(self) -> u32 {
        self.rows
            .checked_section(11)
            .expect("checked matches")
            .count
    }
    pub fn match_table(self, n: u32) -> Option<TirMatchRows<'a>> {
        Some(TirMatchRows {
            owner: self.owner,
            rows: Rows::checked(self.rows.checked_section(11)?.row(n)?),
        })
    }
    pub fn scoped_parameter_count(self) -> u32 {
        self.rows
            .checked_section(12)
            .expect("checked scoped_parameters")
            .count
    }
    pub fn scoped_parameter(self, n: u32) -> Option<ScopedParameter> {
        self.owner
            .source
            .decode(self.rows.checked_section(12)?.row(n)?)
    }
    pub fn materialize(self) -> Option<Body> {
        let header = self.header()?;
        Some(Body {
            arena_scope_grant: header.arena_scope_grant,
            parameter_scope: header.parameter_scope,
            entry: header.entry,
            result: header.result,
            locals: self.owner.collect(self.rows.checked_section(1)?)?,
            places: self.owner.collect(self.rows.checked_section(2)?)?,
            scopes: self.owner.collect(self.rows.checked_section(3)?)?,
            blocks: (0..self.block_count())
                .map(|n| self.block(n)?.materialize())
                .collect::<Option<_>>()?,
            rvalues: self.owner.collect(self.rows.checked_section(5)?)?,
            cleanups: self.owner.collect(self.rows.checked_section(6)?)?,
            cleanup_nodes: self.owner.collect(self.rows.checked_section(7)?)?,
            parameters: self.owner.collect(self.rows.checked_section(8)?)?,
            borrow_cursors: self.owner.collect(self.rows.checked_section(9)?)?,
            tail_sites: self.owner.collect(self.rows.checked_section(10)?)?,
            matches: (0..self.match_table_count())
                .map(|n| self.match_table(n)?.materialize())
                .collect::<Option<_>>()?,
            scoped_parameters: self.owner.collect(self.rows.checked_section(12)?)?,
        })
    }
}
#[derive(Clone, Copy)]
pub struct TirBlockRows<'a> {
    owner: &'a CheckedTir,
    rows: Rows<'a>,
}
impl TirBlockRows<'_> {
    pub fn span(self) -> Option<Span> {
        self.owner.source.decode(self.rows.row(0)?)
    }
    pub fn instruction_count(self) -> u32 {
        self.rows
            .checked_section(1)
            .expect("checked instructions")
            .count
    }
    pub fn instruction(self, n: u32) -> Option<Instruction> {
        self.owner
            .source
            .decode(self.rows.checked_section(1)?.row(n)?)
    }
    pub fn terminator(self) -> Option<Terminator> {
        self.owner.source.decode(self.rows.row(2)?)
    }
    pub fn materialize(self) -> Option<Block> {
        Some(Block {
            span: self.span()?,
            instructions: self.owner.collect(self.rows.checked_section(1)?)?,
            terminator: self.terminator()?,
        })
    }
}
#[derive(Clone, Copy)]
pub struct TirMatchRows<'a> {
    owner: &'a CheckedTir,
    rows: Rows<'a>,
}
impl TirMatchRows<'_> {
    pub fn scrutinee(self) -> Option<PlaceId> {
        self.owner.source.decode(self.rows.row(0)?)
    }
    pub fn pattern_count(self) -> u32 {
        self.rows
            .checked_section(1)
            .expect("checked patterns")
            .count
    }
    pub fn pattern(self, n: u32) -> Option<MatchPattern> {
        self.owner
            .source
            .decode(self.rows.checked_section(1)?.row(n)?)
    }
    pub fn arm_count(self) -> u32 {
        self.rows.checked_section(2).expect("checked arms").count
    }
    pub fn arm(self, n: u32) -> Option<MatchArm> {
        self.owner
            .source
            .decode(self.rows.checked_section(2)?.row(n)?)
    }
    pub fn materialize(self) -> Option<MatchTable> {
        Some(MatchTable {
            scrutinee: self.scrutinee()?,
            patterns: self.owner.collect(self.rows.checked_section(1)?)?,
            arms: self.owner.collect(self.rows.checked_section(2)?)?,
        })
    }
}

#[cfg(test)]
mod tests;
