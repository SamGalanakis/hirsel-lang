//! Preconstruction storage for the existing canonical builder. Walk the same
//! Snapshot fields in a table-free context: no locator, type-source load,
//! source query, output string or compiler phase is reachable from this pass.
use crate::{Cx, ExportInput, Location, ObligationId, ObligationKind, Site};
use hirsel_codec::{Snapshot, Writer};
use hirsel_diag::RuleId;
use hirsel_syntax::DefPath;
use hirsel_tir::{
    Body, Callee, Constructor, InstructionKind, LocalId, PlaceId, Projection, Rvalue, Terminator,
};
use hirsel_types::{SignatureKind, TypeId};
use std::{alloc::Layout, sync::Arc};

#[derive(Clone, Copy, Default)]
pub(crate) struct WireShape {
    pub types: usize,
    pub spans: usize,
    pub syntax: [usize; 4],
    pub paths: usize,
    pub path_bytes: usize,
    pub max_path_bytes: usize,
    pub path_iter_bytes: usize,
    bytes: usize,
    pub unloaded: bool,
}
impl WireShape {
    fn add(self, other: Self) -> Option<Self> {
        let mut syntax = [0; 4];
        for (n, slot) in syntax.iter_mut().enumerate() {
            *slot = self.syntax[n].checked_add(other.syntax[n])?;
        }
        Some(Self {
            types: self.types.checked_add(other.types)?,
            spans: self.spans.checked_add(other.spans)?,
            syntax,
            paths: self.paths.checked_add(other.paths)?,
            path_bytes: self.path_bytes.checked_add(other.path_bytes)?,
            max_path_bytes: self.max_path_bytes.max(other.max_path_bytes),
            path_iter_bytes: self.path_iter_bytes.checked_add(other.path_iter_bytes)?,
            bytes: self.bytes.checked_add(other.bytes)?,
            unloaded: self.unloaded || other.unloaded,
        })
    }
    fn text_upper(self) -> Option<usize> {
        // Sizing emits zero for all u32 table positions. Real positions use at
        // most ten decimal digits, so add the other nine per occurrence.
        let positions = self
            .syntax
            .iter()
            .try_fold(self.types.checked_add(self.spans)?, |n, v| {
                n.checked_add(*v)
            })?;
        self.bytes.checked_add(positions.checked_mul(9)?)
    }
}
fn shape<T: Snapshot<Cx>>(value: &T, input: &ExportInput, depth: usize) -> Option<WireShape> {
    shape_captured(value, input, depth, None)
}
fn shape_captured<T: Snapshot<Cx>>(
    value: &T,
    input: &ExportInput,
    depth: usize,
    rows: Option<&Arc<std::collections::HashSet<TypeId>>>,
) -> Option<WireShape> {
    let mut cx = Cx::new(input.locator.clone(), input.types.clone());
    cx.captured_types = rows.cloned();
    cx.sizing = Some(WireShape::default());
    let mut writer = Writer::counting_at_depth(cx, depth);
    value.write(&mut writer);
    let bytes = writer.output_bytes()?;
    let (_, cx) = writer.finish();
    let mut shape = cx.sizing?;
    shape.bytes = bytes;
    Some(shape)
}
#[derive(Default)]
struct Bytes(usize);
impl Bytes {
    fn add(&mut self, bytes: usize) -> Option<()> {
        self.0 = self.0.checked_add(bytes)?;
        Some(())
    }
    fn rows<T>(&mut self, len: usize) -> Option<()> {
        // Vec's non-zero minimum and geometric growth; includes an old buffer
        // overlapping a growth allocation. Empty vectors need no allocation.
        if len != 0 {
            self.add(len.checked_mul(3)?.max(8).checked_mul(size_of::<T>())?)?;
        }
        Some(())
    }
    fn hash<K, V>(&mut self, len: usize) -> Option<()> {
        if len == 0 {
            return Some(());
        }
        // hashbrown raw/mod.rs capacity_to_buckets and calculate_layout_for:
        // 7/8 load, power-of-two buckets, one control byte per bucket, a full
        // control group, and alignment. Old/new tables overlap on growth.
        let buckets = len
            .checked_mul(8)?
            .div_ceil(7)
            .checked_next_power_of_two()?
            .max(4);
        let entry = Layout::new::<(K, V)>();
        self.add(
            buckets
                .checked_mul(entry.size().checked_add(1)?)?
                .checked_add(16)?
                .checked_add(entry.align())?
                .checked_mul(2)?,
        )
    }
    fn tree<K, V>(&mut self, len: usize) -> Option<()> {
        if len == 0 {
            return Some(());
        }
        // Rust alloc btree/node.rs: non-root nodes have at least five entries;
        // charge each prospective node as an internal eleven-entry node.
        let fields = [
            Layout::new::<usize>(),
            Layout::new::<u16>(),
            Layout::new::<u16>(),
            Layout::new::<[K; 11]>(),
            Layout::new::<[V; 11]>(),
            Layout::new::<[usize; 12]>(),
        ];
        let align = fields.iter().map(Layout::align).max()?;
        let node = fields.iter().try_fold(align - 1, |n, layout| {
            n.checked_add(layout.size())?
                .checked_add(layout.align() - 1)
        })?;
        self.add((len / 5 + 1).checked_mul(node)?)
    }
}
fn rvalue_uses(value: &Rvalue) -> Option<usize> {
    Some(match value {
        Rvalue::Use(_) | Rvalue::Unary { .. } => 1,
        Rvalue::Binary { .. } => 2,
        Rvalue::Construct {
            constructor: Constructor::SortedMap { .. },
            fields,
            ..
        } => fields.len().checked_add(1)?,
        Rvalue::Construct { fields, .. } => fields.len(),
        Rvalue::Closure { captures, .. } => captures.len(),
        Rvalue::Constant(_)
        | Rvalue::ReadConstant { .. }
        | Rvalue::FunctionReference { .. }
        | Rvalue::Size(_)
        | Rvalue::Shape(_) => 0,
    })
}
fn instruction_uses(body: &Body, kind: &InstructionKind) -> Option<usize> {
    Some(match kind {
        InstructionKind::Defuse { .. }
        | InstructionKind::EndOrigin(_)
        | InstructionKind::StorageDead { .. }
        | InstructionKind::BeginLoan { .. }
        | InstructionKind::Drop { .. }
        | InstructionKind::Check { .. } => 1,
        InstructionKind::BindOrigin { .. } | InstructionKind::Snapshot { .. } => 2,
        InstructionKind::BindFields { fields, .. } => fields.len().checked_add(1)?,
        InstructionKind::Assign { value, .. } => match body.rvalues.get(value.0 as usize) {
            Some(value) => rvalue_uses(value)?.checked_add(1)?,
            None => 1,
        },
        InstructionKind::Replace { .. } => 3,
        InstructionKind::StorageLive(_)
        | InstructionKind::EndScope(_)
        | InstructionKind::EndLoan { .. }
        | InstructionKind::RunCleanup(_) => 0,
    })
}
fn terminator_uses(value: &Terminator) -> Option<usize> {
    Some(match value {
        Terminator::Branch { .. } | Terminator::Switch { .. } | Terminator::Return { .. } => 1,
        Terminator::Call {
            callee, arguments, ..
        } => arguments
            .len()
            .checked_add(usize::from(matches!(callee, Callee::Indirect { .. })))?,
        Terminator::Goto(_)
        | Terminator::Match { .. }
        | Terminator::Fail { .. }
        | Terminator::Unreachable => 0,
    })
}
fn contexts(shape: WireShape, types: usize, edges: usize, module: usize) -> Option<usize> {
    let mut weight = Bytes::default();
    weight.add(shape.path_iter_bytes)?;
    weight.rows::<TypeId>(shape.types)?; // probe roots
    weight.rows::<TypeId>(shape.types.checked_add(edges)?)?; // DFS pending
    weight.rows::<hirsel_types::TypeGraphEdge>(edges)?; // graph_edges temporary
    weight.rows::<TypeId>(types)?; // ordered closure
    weight.rows::<TypeId>(types)?; // its Arc conversion
    weight.hash::<TypeId, ()>(types)?;
    weight.hash::<TypeId, u32>(types)?;
    weight.rows::<Location>(shape.spans)?;
    weight.hash::<Location, u32>(shape.spans)?;
    // Both the location Vec and hash keys own their module strings, and the
    // section emitter temporarily clones the location vector once more.
    weight.add(shape.spans.checked_mul(module)?.checked_mul(3)?)?;
    weight.rows::<Location>(shape.spans)?;
    weight.tree::<DefPath, ()>(shape.paths)?;
    weight.add(shape.path_bytes)?;
    for count in shape.syntax {
        weight.tree::<u32, ()>(count)?;
        weight.tree::<u32, u32>(count)?;
    }
    // A table probe overlaps its enclosing section collect/emit context.
    weight.0.checked_mul(2)
}
fn path_len(path: &DefPath) -> Option<usize> {
    path.0.modules.iter().try_fold(
        path.0
            .package
            .name
            .len()
            .checked_add(path.0.package.version.len())?
            .checked_add(path.0.name.len())?
            .checked_add(2)?,
        |n, part| n.checked_add(part.len())?.checked_add(1),
    )
}

/// Exact already-loaded type closure selected by the canonical input fields.
/// The capture contains no native or validation authority.
pub struct CapturedTypes {
    rows: Arc<std::collections::HashSet<TypeId>>,
    types: Arc<hirsel_types::TypeInterner>,
}
fn input_shape(input: &ExportInput) -> Option<WireShape> {
    let mut total = WireShape::default();
    for section in input.signatures.values() {
        for signature in std::iter::once(&section.public).chain(section.owner_view.as_ref()) {
            total = total.add(shape(signature, input, 0)?)?;
        }
    }
    for definition in input.definitions.values() {
        total = total.add(shape(&**definition, input, 0)?)?;
    }
    for constant in input.constants.values() {
        total = total.add(shape(constant, input, 0)?)?;
    }
    Some(total)
}
/// Workspace required to capture the selected type closure. Reads loaded rows
/// only, allocates nothing and never calls a source/locator/provider.
pub fn capture_types_bytes(input: &ExportInput) -> Option<u64> {
    let roots = input_shape(input)?.types;
    let mut edges = 0usize;
    for id in input.types.ids() {
        if let Some(row) = input.types.get_if_loaded(id) {
            edges = edges.checked_add(shape(row, input, 0)?.types)?;
        }
    }
    let mut weight = Bytes::default();
    weight.rows::<TypeId>(roots)?;
    weight.rows::<TypeId>(roots.checked_add(edges)?)?;
    weight.rows::<hirsel_types::TypeGraphEdge>(edges)?;
    weight.hash::<TypeId, ()>(input.types.len())?;
    weight.rows::<TypeId>(input.types.len())?;
    weight.rows::<TypeId>(input.types.len())?; // Arc conversion overlap
    weight.add(
        2 * size_of::<usize>()
            + size_of::<std::collections::HashSet<TypeId>>()
            + size_of::<CapturedTypes>(),
    )?;
    weight.0.try_into().ok()
}
/// Capture under an admitted workspace before any Vec/hash table is allocated.
/// A lazy row on the selected closure refuses; unrelated lazy rows are ignored.
/// No provider I/O or compiler work runs even during capture.
pub fn capture_types(input: &ExportInput, limit: u64) -> Option<CapturedTypes> {
    if capture_types_bytes(input)? > limit {
        return None;
    }
    let count = input.types.len();
    let mut cx = Cx::new(input.locator.clone(), input.types.clone());
    cx.capture_roots = Some(Vec::new());
    let mut writer = Writer::discarding(cx);
    for section in input.signatures.values() {
        for signature in std::iter::once(&section.public).chain(section.owner_view.as_ref()) {
            signature.write(&mut writer);
        }
    }
    for definition in input.definitions.values() {
        definition.write(&mut writer);
    }
    for constant in input.constants.values() {
        constant.write(&mut writer);
    }
    let mut pending = writer.finish().1.capture_roots?;
    let mut seen = std::collections::HashSet::new();
    while let Some(id) = pending.pop() {
        if id == TypeId::DANGLING || !seen.insert(id) {
            continue;
        }
        if seen.len() > count || input.types.len() != count {
            return None;
        }
        let row = input.types.get_if_loaded(id)?;
        pending.extend(row.kind.graph_edges().into_iter().map(|edge| edge.ty));
    }
    Some(CapturedTypes {
        rows: Arc::new(seen),
        types: input.types.clone(),
    })
}
/// Inspect construction using exactly the admitted, already-loaded closure.
/// A capture from another interner or lacking an input edge is refused.
pub fn build_captured_tables_bytes(
    input: &ExportInput,
    max_module_bytes: usize,
    types: &CapturedTypes,
) -> Option<u64> {
    if !Arc::ptr_eq(&input.types, &types.types) {
        return None;
    }
    build_tables(input, max_module_bytes, Some(&types.rows))
}
/// Construction workspace when the input's unique payloads move into Export.
/// The source body-name set owns copied paths while the input map is consumed.
/// Retained input payloads stay under the caller's existing input reservation.
pub fn build_owned_tables_bytes(
    input: &ExportInput,
    max_module_bytes: usize,
    types: &CapturedTypes,
) -> Option<u64> {
    let base = build_captured_tables_bytes(input, max_module_bytes, types)?;
    let mut extra = Bytes::default();
    extra.tree::<DefPath, ()>(input.definitions.len().checked_add(input.skipped.len())?)?;
    for path in input
        .definitions
        .keys()
        .chain(input.skipped.iter().map(|item| &item.name))
    {
        let payload = path.0.modules.iter().try_fold(
            path.0
                .package
                .name
                .len()
                .checked_add(path.0.package.version.len())?
                .checked_add(path.0.name.len())?
                .checked_add(path.0.modules.len().checked_mul(size_of::<String>())?)?,
            |bytes, module| bytes.checked_add(module.len()),
        )?;
        extra.add(payload)?;
    }
    base.checked_add(extra.0.try_into().ok()?)
}
/// Upper bound for canonical construction tables, digest-text scratch and
/// section metadata. Input payload clones are a separate caller obligation.
/// `max_module_bytes` bounds every location module supplied by the locator.
/// No locator or provider is called. Types needed by the input must already
/// be materialized by its source producer; lazy gaps refuse this inspection.
pub fn build_tables_bytes(input: &ExportInput, max_module_bytes: usize) -> Option<u64> {
    build_tables(input, max_module_bytes, None)
}
fn build_tables(
    input: &ExportInput,
    max_module_bytes: usize,
    captured: Option<&Arc<std::collections::HashSet<TypeId>>>,
) -> Option<u64> {
    let mut types = WireShape::default();
    let mut type_rows = 0usize;
    let mut max_contracts = 0usize;
    for id in input
        .types
        .ids()
        .filter(|id| captured.is_none_or(|rows| rows.contains(id)))
    {
        let Some(row) = input.types.get_if_loaded(id) else {
            continue;
        };
        // A materialized graph may not hide another type-source fetch in sizing.
        let row_shape = shape_captured(row, input, 2, captured)?;
        if row_shape.unloaded {
            return None;
        }
        types = types.add(row_shape)?;
        type_rows = type_rows.checked_add(1)?;
        if let hirsel_types::TypeKind::Function(function) = &row.kind {
            max_contracts = max_contracts.max(function.contracts.len());
        }
    }
    let edges = types.types;
    let mut data = WireShape::default();
    let mut weight = Bytes::default();
    let mut largest_text = 0usize;
    let mut largest_tables = 0usize;
    let mut max_path = 0usize;
    let mut max_path_clone = 0usize;
    let mut path_payload = 0usize;
    for (path, section) in &input.signatures {
        max_path = max_path.max(path_len(path)?);
        let clone_bytes = path.0.modules.iter().try_fold(
            path.0
                .package
                .name
                .len()
                .checked_add(path.0.package.version.len())?
                .checked_add(path.0.name.len())?
                .checked_add(path.0.modules.len().checked_mul(size_of::<String>())?)?,
            |n, module| n.checked_add(module.len()),
        )?;
        max_path_clone = max_path_clone.max(clone_bytes);
        path_payload = path_payload.checked_add(path_len(path)?)?;
        for signature in std::iter::once(&section.public).chain(section.owner_view.as_ref()) {
            data = data.add(shape_captured(signature, input, 0, captured)?)?;
            if let SignatureKind::Function(function) = &signature.kind {
                max_contracts = max_contracts.max(function.contracts.len());
            }
        }
    }
    for section in input.signatures.values() {
        data = data.add(shape_captured(&section.dependencies, input, 0, captured)?)?;
    }
    for value in input.constants.values() {
        data = data.add(shape_captured(value, input, 0, captured)?)?;
    }
    weight.rows::<crate::DefinitionSection>(input.definitions.len())?;
    weight.rows::<crate::SignatureSection>(input.signatures.len())?;
    weight.rows::<crate::ConstantSection>(input.constants.len())?;
    weight.tree::<&DefPath, ()>(input.definitions.len().checked_add(input.skipped.len())?)?;
    for definition in input.definitions.values() {
        if definition
            .types
            .iter()
            .any(|id| input.types.get_if_loaded(*id).is_none())
        {
            return None;
        }
        let own = shape_captured(&**definition, input, 0, captured)?;
        data = data.add(own)?;
        // Definition's TypeTable is nested two levels in its digest preimage.
        // The count, tags, separators and closing node fit this fixed header.
        let text = own
            .text_upper()?
            .checked_add(types.text_upper()?)?
            .checked_add(64)?;
        if own.unloaded {
            return None;
        }
        largest_text = largest_text.max(text);
        let mut instructions = 0usize;
        let mut blocks = 0usize;
        let mut places = 0usize;
        let mut uses = 0usize;
        let mut projections = 0usize;
        let mut calls = 0usize;
        let mut max_rule = 6usize; // built-in FACT-1, MEM-13/14 and SEM-1
        let mut map_rows = 0usize;
        for body in &definition.bodies {
            places = places.checked_add(body.places.len())?;
            map_rows = map_rows
                .checked_add(body.locals.len())?
                .checked_add(body.rvalues.len())?;
            for place in &body.places {
                projections = projections.checked_add(place.projection.len())?;
            }
            for block in &body.blocks {
                blocks = blocks.checked_add(1)?;
                uses = uses.checked_add(terminator_uses(&block.terminator)?)?;
                if matches!(block.terminator, Terminator::Call { .. }) {
                    calls = calls.checked_add(1)?;
                }
                if let Terminator::Fail { rule, .. } = &block.terminator {
                    max_rule = max_rule.max(rule.as_str().len());
                }
                for instruction in &block.instructions {
                    instructions = instructions.checked_add(1)?;
                    uses = uses.checked_add(instruction_uses(body, &instruction.kind)?)?;
                    if let InstructionKind::Check { rule, .. } = &instruction.kind {
                        max_rule = max_rule.max(rule.as_str().len());
                    }
                }
            }
        }
        let call_clauses = calls.checked_mul(max_contracts)?;
        let projected = places.checked_mul(2)?;
        let obligations = instructions
            .checked_add(blocks)?
            .checked_add(call_clauses)?
            .checked_add(projected)?
            .checked_add(max_contracts)?;
        let sites = instructions
            .checked_mul(2)?
            .checked_add(blocks)?
            .checked_add(call_clauses)?
            .checked_add(uses.checked_mul(2)?)?;
        let mut tables = Bytes::default();
        tables.rows::<(ObligationId, Vec<Site>, crate::Detail)>(obligations)?;
        tables.rows::<crate::Obligation>(obligations)?;
        tables.rows::<Site>(sites.checked_add(obligations.checked_mul(4)?)?)?;
        tables.tree::<(RuleId, ObligationKind), u32>(obligations)?;
        tables.tree::<ObligationId, (hirsel_tir::BodyId, PlaceId)>(projected)?;
        tables.tree::<u32, Vec<Site>>(max_contracts)?;
        tables.hash::<(usize, usize, usize), u32>(instructions)?;
        tables.hash::<hirsel_tir::BodyId, u32>(max_contracts)?;
        tables.hash::<LocalId, hirsel_tir::BodyId>(map_rows)?;
        tables.hash::<LocalId, hirsel_tir::BodyId>(map_rows)?;
        tables.rows::<&hirsel_types::Contract>(max_contracts.checked_mul(2)?)?;
        tables.add(
            obligations
                .checked_mul(2)?
                .checked_add(projected.checked_mul(2)?)?
                .checked_mul(max_rule)?,
        )?;
        tables.hash::<(usize, LocalId, Vec<Projection>, RuleId), usize>(projected)?;
        tables.hash::<(usize, LocalId, Vec<Projection>), PlaceId>(places)?;
        tables.rows::<Projection>(projections.checked_mul(4)?)?;
        tables.rows::<PlaceId>(uses)?;
        tables.tree::<DefPath, ()>(calls)?;
        tables.rows::<DefPath>(calls)?;
        let path_copies = obligations
            .checked_add(projected)?
            .checked_add(call_clauses)?
            .checked_add(calls)?
            .checked_add(1)?;
        let path_clone = max_path_clone
            .max(own.max_path_bytes)
            .max(types.max_path_bytes);
        tables.add(path_copies.checked_mul(path_clone.checked_add(size_of::<DefPath>())?)?)?;
        // checksum's formatted owner paths and vector of byte slices overlap.
        tables.rows::<String>(input.signatures.len())?;
        tables.rows::<&[u8]>(input.signatures.len().checked_mul(2)?.checked_add(2)?)?;
        tables.add(path_payload.checked_mul(2)?)?;
        tables.add(
            max_path
                .checked_add(max_rule)?
                .checked_add(40)?
                .checked_mul(2)?,
        )?;
        largest_tables = largest_tables.max(tables.0);
        // Retained per-definition location/reference/operation/obligation rows.
        weight.rows::<Location>(own.spans.checked_add(types.spans)?)?;
        weight.add(
            own.spans
                .checked_add(types.spans)?
                .checked_mul(max_module_bytes)?,
        )?;
        weight.rows::<hirsel_types::SignatureDependency>(own.paths.checked_add(types.paths)?)?;
        weight.add(tables.0)?;
    }
    if data.unloaded {
        return None;
    }
    // annotation_digest clones its input independently of Export's retained
    // annotation copy and creates a temporary empty type interner.
    weight.rows::<crate::Annotation>(input.annotations.len())?;
    weight.add(
        std::alloc::Layout::new::<[usize; 2]>()
            .extend(Layout::new::<hirsel_types::TypeInterner>())
            .ok()?
            .0
            .pad_to_align()
            .size(),
    )?;
    weight.add(2 * size_of::<usize>())?;
    for annotation in &input.annotations {
        weight.add(annotation.name.len())?;
        weight.add(annotation.location.module.len())?;
        weight.rows::<String>(annotation.payload.len())?;
        for token in &annotation.payload {
            weight.add(token.len())?;
        }
        let path = &annotation.path.0;
        weight.add(path.package.name.len())?;
        weight.add(path.package.version.len())?;
        weight.add(path.name.len())?;
        weight.rows::<String>(path.modules.len())?;
        for module in &path.modules {
            weight.add(module.len())?;
        }
    }
    let annotations = shape(&input.annotations, input, 0)?;
    largest_text = largest_text.max(annotations.text_upper()?);
    weight.rows::<crate::Skipped>(input.definitions.len().checked_add(input.skipped.len())?)?;
    weight.add(
        input
            .definitions
            .len()
            .checked_mul(max_path.checked_mul(2)?.checked_add(96)?)?,
    )?;
    let context = contexts(data.add(types)?, type_rows, edges, max_module_bytes)?;
    weight.add(context)?;
    weight.add(largest_tables)?;
    // encode_with_output_limit constructs the trailing digest in a second
    // Writer and wraps it with newlines before appending it to the main text.
    let mut digest = Writer::counting(());
    digest.open("ExportDigest");
    digest.bytes(&[0; 32]);
    digest.close();
    weight.add(digest.output_bytes()?.checked_add(2)?.checked_mul(3)?)?;
    // String growth can overlap its old allocation before realloc returns.
    weight.add(largest_text.checked_mul(3)?.max(8))?;
    weight.0.try_into().ok()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{Environment, Header, SignatureSection};
    use hirsel_span::Span;
    use hirsel_syntax::{ContentHash, DefId};
    use hirsel_tir::{
        Block, BlockId, BodyId, CleanupId, Definition, Instruction, Local, Operand, ScopeId,
        Storage,
    };
    use hirsel_types::{
        Primitive, Signature, Type, TypeBase, TypeBaseSource, TypeInterner, TypeKind, UseKind,
    };
    use std::{
        alloc::{GlobalAlloc, System},
        cell::Cell,
        collections::BTreeMap,
        rc::Rc,
        sync::Arc,
    };

    #[derive(Clone, Copy, Default)]
    struct Count {
        live: isize,
        peak: isize,
        allocations: usize,
    }
    thread_local! {
        static COUNT: Cell<Option<Count>> = const { Cell::new(None) };
    }
    fn count(added: usize, removed: usize) {
        let _ = COUNT.try_with(|slot| {
            if let Some(mut count) = slot.get() {
                count.live += added as isize - removed as isize;
                count.peak = count.peak.max(count.live);
                count.allocations += usize::from(added != 0);
                slot.set(Some(count));
            }
        });
    }
    // Same System delegation as driver/tests/validate_memory.rs; thread-local
    // counters exclude concurrent test harness work from this control's peak.
    pub(crate) struct Counting;
    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let pointer = unsafe { System.alloc(layout) };
            if !pointer.is_null() {
                count(layout.size(), 0);
            }
            pointer
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            let pointer = unsafe { System.alloc_zeroed(layout) };
            if !pointer.is_null() {
                count(layout.size(), 0);
            }
            pointer
        }
        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            unsafe { System.dealloc(pointer, layout) };
            count(0, layout.size());
        }
        unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            let moved = unsafe { System.realloc(pointer, layout, size) };
            if !moved.is_null() {
                count(size, layout.size());
            }
            moved
        }
    }
    fn measured<T>(run: impl FnOnce() -> T) -> (T, Count) {
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                COUNT.set(None);
            }
        }
        COUNT.set(Some(Count::default()));
        let reset = Reset;
        let result = run();
        let count = COUNT.get().unwrap();
        drop(reset);
        (result, count)
    }
    fn input(types: Arc<TypeInterner>) -> ExportInput {
        ExportInput {
            types,
            header: Header {
                format: crate::FORMAT.into(),
                tir_version: hirsel_tir::TIR_VERSION,
                environment: Environment {
                    interface_version: 1,
                    compiler_version: "admission".into(),
                    spec_hash: ContentHash::default(),
                },
                entry: DefPath::core("admission"),
                program_revision: None,
            },
            definitions: BTreeMap::new(),
            signatures: BTreeMap::new(),
            constants: BTreeMap::new(),
            annotations: vec![],
            skipped: vec![],
            locator: Rc::new(|_| Location::unknown()),
        }
    }
    struct Lazy;
    impl TypeBaseSource for Lazy {
        fn rows(&self, _: u32, _: &TypeBase) -> Option<Vec<(u32, Type)>> {
            panic!("sizing must not load type rows")
        }
        fn candidates(&self, _: &Type, _: &TypeBase) -> Vec<u32> {
            panic!("sizing must not intern types")
        }
    }
    #[test]
    fn sizing_is_allocation_free_and_refuses_lazy_roots_before_provider_reads() {
        let base = TypeBase::from_source(1, Arc::new(Lazy));
        let id = base.id_at(0).unwrap();
        let types = TypeInterner::new(Some(base));
        let mut input = input(types.clone());
        input.locator = Rc::new(|_| panic!("sizing must not locate source spans"));
        input.constants.insert(
            DefPath::core("lazy"),
            hirsel_tir::Materialized {
                root: hirsel_tir::ValueNode {
                    ty: id,
                    kind: hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Unit),
                },
            },
        );
        let (bound, counts) = measured(|| build_tables_bytes(&input, 0));
        assert!(bound.is_none());
        assert_eq!(counts.allocations, 0);
        assert!(types.get_if_loaded(id).is_none());
    }
    #[test]
    fn sizing_preserves_the_canonical_missing_type_position_without_loading_rows() {
        let mut input = input(TypeInterner::new(None));
        input.constants.insert(
            DefPath::core("missing"),
            hirsel_tir::Materialized {
                root: hirsel_tir::ValueNode {
                    ty: TypeId::DANGLING,
                    kind: hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Unit),
                },
            },
        );
        let (bound, counts) = measured(|| build_tables_bytes(&input, 0));
        assert!(bound.is_some());
        assert_eq!(counts.allocations, 0);
        let built = crate::build(&input);
        let text = crate::encode(&built, &input.locator);
        assert!(text.contains("4294967295"));
        assert_eq!(
            crate::encoded_bytes(&built, &input.locator),
            Some(text.len())
        );
    }
    #[test]
    fn shared_path_emission_borrows_payloads_and_preserves_flat_canonical_bytes() {
        let values = vec!["long-payload".repeat(1024); 4];
        let path: hirsel_types::SharedPath<String> = values.clone().into();
        let input = input(TypeInterner::new(None));
        let (shape, sizing) = measured(|| shape(&path, &input, 0));
        assert_eq!(sizing.allocations, 0);
        let mut writer = Writer::counting(Cx::new(input.locator.clone(), input.types.clone()));
        let (_, emission) = measured(|| path.write(&mut writer));
        assert!(emission.peak as usize <= shape.unwrap().path_iter_bytes);
        let mut actual = Writer::with_context(Cx::new(input.locator.clone(), input.types.clone()));
        path.write(&mut actual);
        let mut flat = Writer::with_context(Cx::new(input.locator.clone(), input.types.clone()));
        values.write(&mut flat);
        assert_eq!(actual.finish().0, flat.finish().0);
    }
    #[test]
    fn captured_type_workspace_ignores_unrelated_lazy_rows_and_guards_every_input_edge() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Partial(AtomicUsize);
        impl TypeBaseSource for Partial {
            fn rows(&self, position: u32, base: &TypeBase) -> Option<Vec<(u32, Type)>> {
                self.0.fetch_add(1, Ordering::Relaxed);
                assert_eq!(position, 0, "capturing/sizing must not load the lazy child");
                Some(vec![(
                    0,
                    Type::with_kind(TypeKind::Box(base.id_at(1).unwrap())),
                )])
            }
            fn candidates(&self, _: &Type, _: &TypeBase) -> Vec<u32> {
                vec![]
            }
        }
        let source = Arc::new(Partial(AtomicUsize::new(0)));
        let base = TypeBase::from_source(2, source.clone());
        let unrelated = base.id_at(0).unwrap();
        let types = TypeInterner::new(Some(base));
        types.get(unrelated).unwrap();
        let unit = types.intern(Type::unit());
        let mut input = input(types.clone());
        input.constants.insert(
            DefPath::core("unit"),
            hirsel_tir::Materialized {
                root: hirsel_tir::ValueNode {
                    ty: unit,
                    kind: hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Unit),
                },
            },
        );
        assert!(
            build_tables_bytes(&input, 0).is_none(),
            "the original all-loaded-rows inspection remains conservative"
        );
        let (needed, sizing) = measured(|| capture_types_bytes(&input));
        assert_eq!(sizing.allocations, 0);
        let needed = needed.unwrap();
        let (missing, refused) = measured(|| capture_types(&input, needed - 1));
        assert!(missing.is_none());
        assert_eq!(
            refused.allocations, 0,
            "N-1 refuses before workspace allocation"
        );
        let (captured, actual) = measured(|| capture_types(&input, needed));
        let captured = captured.unwrap();
        assert!(actual.peak as u64 <= needed);
        assert!(actual.allocations > 0);
        let (bound, sizing) = measured(|| build_captured_tables_bytes(&input, 0, &captured));
        assert!(bound.is_some());
        assert_eq!(sizing.allocations, 0);
        assert_eq!(
            crate::encode(&crate::build(&input), &input.locator),
            crate::export(&input)
        );
        input.constants.values_mut().next().unwrap().root.ty = unrelated;
        assert!(
            build_captured_tables_bytes(&input, 0, &captured).is_none(),
            "a capture cannot omit an input root"
        );
        assert!(
            capture_types(&input, capture_types_bytes(&input).unwrap()).is_none(),
            "an actual lazy closure gap refuses"
        );
        assert_eq!(source.0.load(Ordering::Relaxed), 1);
    }
    fn construction_input(width: usize) -> ExportInput {
        let types = TypeInterner::new(None);
        let ty = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::U64)));
        let mut input = input(types);
        let path = input.header.entry.clone();
        let span = Span::default();
        let projection = vec![
            Projection::Index(LocalId(0)),
            Projection::VariantField {
                variant: 0,
                field: 0,
            },
        ]
        .into();
        let mut definition = Definition {
            caller_bodies: vec![],
            protocol_associations: vec![],
            types: Arc::from([ty]),
            path: DefId::intern(&path),
            signature_hash: ContentHash::default(),
            entry: BodyId(0),
            contracts: vec![],
            bodies: vec![Body {
                arena_scope_grant: None,
                parameter_scope: hirsel_syntax::LexicalOwner::first(&path)
                    .site(hirsel_syntax::NameSite::Item),
                locals: vec![Local {
                    ty,
                    storage: Storage::Local,
                    scope: ScopeId(0),
                    span,
                }],
                places: vec![hirsel_tir::Place {
                    loan: None,
                    binding: None,
                    root: LocalId(0),
                    projection,
                    ty,
                }],
                blocks: vec![Block {
                    span,
                    instructions: vec![
                        Instruction {
                            kind: InstructionKind::Check {
                                rule: RuleId::of("FACT-1"),
                                condition: Operand {
                                    place: PlaceId(0),
                                    usage: UseKind::Copy
                                }
                            },
                            span,
                            on_failure: CleanupId(0)
                        };
                        width
                    ],
                    terminator: Terminator::Unreachable,
                }],
                scopes: vec![],
                rvalues: vec![],
                cleanups: vec![],
                cleanup_nodes: vec![],
                entry: BlockId(0),
                parameters: vec![],
                result: ty,
                borrow_cursors: vec![],
                tail_sites: vec![],
                matches: vec![],
                scoped_parameters: vec![],
            }],
        };
        definition.types = hirsel_tir::definition_types(&input.types, &definition);
        input.definitions.insert(path.clone(), Arc::new(definition));
        input.signatures.insert(
            path.clone(),
            SignatureSection {
                path: path.clone(),
                dependencies: vec![],
                owner_view: None,
                public: Signature {
                    definition: DefId::intern(&path),
                    metadata: Default::default(),
                    fingerprint: ContentHash::default(),
                    kind: SignatureKind::Const {
                        ty,
                        generics: vec![].into(),
                    },
                },
            },
        );
        input
    }
    #[test]
    fn consuming_construction_moves_original_buffers_and_preserves_exact_scalar_bytes() {
        for width in [1, 4, 16] {
            let input = construction_input(width);
            let expected = crate::export(&input);
            let body_buffer = input.definitions.values().next().unwrap().bodies.as_ptr() as usize;
            let locator = input.locator.clone();
            let captured = capture_types(&input, capture_types_bytes(&input).unwrap()).unwrap();
            let bound = build_owned_tables_bytes(&input, 0, &captured).unwrap();
            let (built, actual) = measured(|| crate::build_owned(input));
            let built = built.unwrap();
            assert!(built.skipped.is_empty());
            assert_eq!(
                built.definitions[0].tir.bodies.as_ptr() as usize,
                body_buffer,
                "move the original body allocation"
            );
            assert!(
                actual.peak as u64 <= bound,
                "width {width}: actual {} exceeds workspace {bound}",
                actual.peak
            );
            assert_eq!(crate::encode(&built, &locator), expected);
        }
        for weak in [false, true] {
            let input = construction_input(4);
            let original = input.definitions.values().next().unwrap();
            let shared = (!weak).then(|| original.clone());
            let weak_owner = weak.then(|| Arc::downgrade(original));
            let (built, actual) = measured(|| crate::build_owned(input));
            assert!(
                built.is_none(),
                "shared or externally upgradable originals cannot move"
            );
            assert_eq!(
                actual.allocations, 0,
                "refuse before construction allocations"
            );
            drop((shared, weak_owner));
        }
    }
    #[test]
    fn construction_bound_covers_real_obligation_and_context_allocation_peaks() {
        for width in [1, 4, 16] {
            let input = construction_input(width);
            let (_, clones) = measured(|| {
                (
                    input.header.clone(),
                    input
                        .definitions
                        .values()
                        .map(|value| (**value).clone())
                        .collect::<Vec<_>>(),
                    input.signatures.values().cloned().collect::<Vec<_>>(),
                )
            });
            let (bound, sizing) = measured(|| build_tables_bytes(&input, 0));
            assert_eq!(sizing.allocations, 0);
            let (built, actual) = measured(|| crate::build(&input));
            assert!(built.skipped.is_empty());
            assert!(!built.definitions[0].obligations.is_empty());
            assert!(
                actual.peak as u64 <= bound.unwrap() + clones.peak as u64,
                "width {width}: actual {} tables {:?} payload {}",
                actual.peak,
                bound,
                clones.peak
            );
        }
    }
}
