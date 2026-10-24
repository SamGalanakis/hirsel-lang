//! Runtime descriptors from the final schemas: layouts derived by `hirsel_lir::layout`'s
//! `LayoutBuilder` and checked by its validator, lowered by
//! `hirsel_lir::rt_program` and materialized as C descriptors. Values are filled by a
//! walk of the LayoutRecord's nested drop program, and [`expected`] is an independent oracle:
//! the same walk, recording the events a release must produce in order.

use std::cell::RefCell;
use std::collections::HashMap;

use hirsel_layout::layout::{LayoutBuilder, OwnerRepresentation, Owners};
use hirsel_lir::layout::evaluate_size;
use hirsel_lir::layout_types::{
    DropStep, LayoutId, LayoutKind, LayoutTable, SizeSpec, TagForm, TargetSpec,
};
use hirsel_lir::layout_validate::validate_table;
use hirsel_lir::rt_program::{self, Descriptor, StepKind};
use hirsel_rt::{Config, Header, LAYOUT_MAGIC, LAYOUT_VERSION, Layout, Step, TaskCtx};
use hirsel_syntax::{ContentHash, DefId, DefPath, ItemPath, PackageId};
use hirsel_tir::{BodyId, RepresentationRecord};
use hirsel_types::{Layout as Promises, TypeId, TypeInterner};

use super::{Ctx, leak};

pub fn path(name: &str) -> DefPath {
    DefPath(ItemPath {
        package: PackageId {
            name: "runtime-fixture".into(),
            version: "0".into(),
        },
        modules: vec!["m".into()],
        name: name.into(),
    })
}

pub use hirsel_types::testing::Types as Ty;

thread_local! {
    /// A test's session interner: every row a test builds, owner shapes
    /// included, lives in it, as in a compile session.
    static SESSION: std::sync::Arc<TypeInterner> = TypeInterner::new(None);
}

/// A type builder over this test thread's session interner.
pub fn ty() -> Ty {
    Ty {
        store: SESSION.with(|s| s.clone()),
    }
}

/// How a nominal is represented.
#[derive(Clone, Copy, Default)]
pub struct Nominal {
    pub resource: bool,
    pub finalizer: bool,
    pub unsafe_ffi: bool,
    pub soa: bool,
}

/// Declare a source nominal whose private shape `shape` builds.
pub fn nominal(
    owners: &mut Owners,
    name: &str,
    spec: Nominal,
    shape: impl FnOnce(&mut Ty) -> TypeId,
) -> DefPath {
    let mut t = ty();
    let private_shape = shape(&mut t);
    owners.nominals.insert(
        DefId::intern(&path(name)),
        OwnerRepresentation::Defined(RepresentationRecord {
            definition: path(name),
            representation_revision: ContentHash::of_bytes(name.as_bytes()),
            generics: Vec::new(),
            private_shape,
            layout_promises: Promises {
                soa: spec.soa,
                ..Promises::default()
            },
            resource: spec.resource,
            finalizer: spec
                .finalizer
                .then(|| (path(&format!("{name}.drop")), BodyId(0))),
            unsafe_ffi_package: spec.unsafe_ffi,
        }),
    );
    path(name)
}

/// Declare a run-context handle type; a resource handle closes on drop.
pub fn handle(owners: &mut Owners, name: &str, resource: bool) -> DefPath {
    owners.nominals.insert(
        DefId::intern(&path(name)),
        OwnerRepresentation::Handle {
            revision: ContentHash::of_bytes(name.as_bytes()),
            resource,
        },
    );
    path(name)
}

/// One finalizer or handle-close event of a release.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Event {
    /// A finalizer ran on the value at this address.
    Finalize(u64),
    /// The Io adapter closed this handle.
    Close(u64),
}

thread_local! {
    static LOG: RefCell<Vec<Event>> = const { RefCell::new(Vec::new()) };
    static FAIL_AT: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
}

/// Clear the event log and reserve room, so logging inside a release never allocates.
pub fn reset_log() {
    LOG.with(|log| {
        let mut log = log.borrow_mut();
        log.clear();
        log.reserve(1 << 16);
    });
}

pub fn take_log() -> Vec<Event> {
    LOG.with(|log| std::mem::take(&mut *log.borrow_mut()))
}

/// Make the finalizers of the values at these addresses fail.
pub fn fail_at(addresses: Vec<u64>) {
    FAIL_AT.with(|f| *f.borrow_mut() = addresses);
}

/// The finalizer of every derived resource and of leaf blocks: logs the value's address and,
/// when told to, fails with rule 100 + its position in the [`fail_at`] list.
pub unsafe extern "C" fn log_address(
    ctx: *mut TaskCtx,
    value: *mut u8,
    anchor: u64,
    _operation_site: u64,
) -> u8 {
    let address = value as u64;
    LOG.with(|log| log.borrow_mut().push(Event::Finalize(address)));
    match FAIL_AT.with(|f| f.borrow().iter().position(|&a| a == address)) {
        Some(index) => unsafe {
            hirsel_rt::failure::hirsel_rt_v2_failure_begin(
                ctx,
                1,
                100 + index as u32,
                anchor,
                anchor,
            )
        },
        None => 0,
    }
}

/// The Io adapter's handle close: logs the handle.
pub unsafe extern "C" fn log_close(
    _ctx: *mut TaskCtx,
    handle: u64,
    _anchor: u64,
    _operation_site: u64,
) -> u8 {
    LOG.with(|log| log.borrow_mut().push(Event::Close(handle)));
    0
}

/// A diagnostic context whose handle drops close through [`log_close`].
pub fn context() -> Ctx {
    Ctx::with(Config {
        flags: hirsel_rt::context::CONFIG_DIAGNOSTIC,
        close_handle: Some(log_close),
        ..Config::new()
    })
}

/// A derived, validated table with C descriptors for the layouts a test releases.
pub struct Derived {
    pub table: LayoutTable,
    pub descriptors: HashMap<LayoutId, Descriptor>,
    materialized: HashMap<LayoutId, &'static Layout>,
    /// A leaf block's layout: 8 bytes, finalized by [`log_address`].
    pub leaf: &'static Layout,
}

/// Derive `types` on x86-64, validate the table, and materialize descriptors for every layout
/// a block of these types holds (and every layout those name).
pub fn derive(owners: &Owners, store: &TypeInterner, types: &[TypeId]) -> (Derived, Vec<LayoutId>) {
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let mut builder = LayoutBuilder::new(&target, owners);
    let ids: Vec<LayoutId> = types
        .iter()
        .map(|&ty| builder.layout(store, ty).expect("derivable type"))
        .collect();
    let table = builder.finish();
    if let Err(violations) = validate_table(&table, &target) {
        panic!("B's validator rejects the derived table: {violations:#?}");
    }
    let roots: Vec<LayoutId> = (0..table.layouts.len() as u32).map(LayoutId).collect();
    let descriptors: HashMap<LayoutId, Descriptor> = rt_program::descriptors(&table, &roots)
        .expect("every derived layout lowers")
        .into_iter()
        .map(|d| (d.layout, d))
        .collect();
    let materialized = materialize(&descriptors);
    let leaf = super::layout(8, 0, Some(log_address), Vec::new());
    (
        Derived {
            table,
            descriptors,
            materialized,
            leaf,
        },
        ids,
    )
}

/// Leak C descriptors: every layout first, then the steps that name them.
fn materialize(descriptors: &HashMap<LayoutId, Descriptor>) -> HashMap<LayoutId, &'static Layout> {
    let order: Vec<&Descriptor> = descriptors.values().collect();
    let layouts: &'static mut [Layout] = Box::leak(
        order
            .iter()
            .map(|d| Layout {
                magic: LAYOUT_MAGIC,
                version: LAYOUT_VERSION,
                size: d.size,
                align: d.align,
                finalizer: d.finalizer.as_ref().map(|_| log_address as _),
                steps: std::ptr::null(),
                step_count: d.steps.len() as u64,
                field_count: d.field_count,
                flags: d.flags,
                logical: d.logical,
                scan: std::ptr::null(),
                scan_words: u64::MAX,
                logical_count: d.logical_count.map_or(std::ptr::null(), |words| {
                    Box::leak(Box::new(words)).as_ptr()
                }),
            })
            .collect::<Vec<_>>()
            .into_boxed_slice(),
    );
    let index: HashMap<LayoutId, usize> = order
        .iter()
        .enumerate()
        .map(|(i, d)| (d.layout, i))
        .collect();
    let base = layouts.as_mut_ptr();
    for (i, d) in order.iter().enumerate() {
        let steps: Vec<Step> = d
            .steps
            .iter()
            .map(|s| Step {
                kind: s.kind.code(),
                field: s.field,
                variant: s.variant,
                span: s.span,
                offset: s.offset,
                stride: s.stride,
                count: s.count,
                count_offset: s.count_offset,
                // SAFETY: `index` maps into `layouts`, which lives forever.
                layout: s
                    .layout
                    .map_or(std::ptr::null(), |id| unsafe { base.add(index[&id]) }),
            })
            .collect();
        let steps: &'static [Step] = Box::leak(steps.into_boxed_slice());
        // SAFETY: `i` indexes `layouts`; no reference into it is live.
        unsafe { (*base.add(i)).steps = steps.as_ptr() };
    }
    let layouts: &'static [Layout] = layouts;
    for layout in layouts {
        super::register_fixture(layout);
    }
    order
        .iter()
        .enumerate()
        .map(|(i, d)| (d.layout, &layouts[i]))
        .collect()
}

impl Derived {
    pub fn kind(&self, id: LayoutId) -> &LayoutKind {
        &self.table.layout(id).unwrap().kind
    }

    /// The payload layout of a block layout.
    pub fn payload(&self, id: LayoutId) -> LayoutId {
        match self.kind(id) {
            LayoutKind::Block { payload } | LayoutKind::Box { payload } => *payload,
            other => panic!("not a block layout: {other:?}"),
        }
    }

    pub fn descriptor(&self, id: LayoutId) -> &'static Layout {
        self.materialized[&id]
    }

    /// Allocate a block for `payload` with `count` elements (and capacity `capacity`) when its
    /// size is dynamic, write those words, and fill the value.
    pub fn build(&self, ctx: &Ctx, payload: LayoutId, count: u64, rng: &mut u64) -> *mut Header {
        let record = self.table.layout(payload).unwrap();
        let mut header = vec![0u8; 64];
        let words = self.dynamic_words(payload, count);
        for &(offset, value) in &words {
            header[offset as usize..offset as usize + 8].copy_from_slice(&value.to_le_bytes());
        }
        let extent = evaluate_size(&record.size, &header).expect("extent");
        let block = super::alloc_block(ctx.0, self.descriptor(payload), extent.max(1));
        let value = super::payload(block);
        for &(offset, word) in &words {
            unsafe {
                value
                    .add(offset as usize)
                    .cast::<u64>()
                    .write_unaligned(word)
            };
        }
        self.fill(ctx, &record.drop, value, rng);
        block
    }

    /// The count and capacity words a dynamic root payload needs.
    fn dynamic_words(&self, payload: LayoutId, count: u64) -> Vec<(u64, u64)> {
        let record = self.table.layout(payload).unwrap();
        if matches!(record.size, SizeSpec::Fixed(_)) {
            return Vec::new();
        }
        match record.drop.first() {
            Some(DropStep::ElementsReverse { count_offset, .. }) => vec![(*count_offset, count)],
            Some(DropStep::ColumnsReverse(c)) => {
                let capacity = count.next_multiple_of(c.capacity_quantum) + c.capacity_quantum;
                vec![(c.count_offset, count), (c.capacity_offset, capacity)]
            }
            _ => panic!("unsupported dynamic root {:?}", record.kind),
        }
    }

    /// Fill a value by walking its drop program: fresh leaf blocks for holders, fresh handle ids,
    /// and a chosen tag for each variant (sometimes one with nothing to drop).
    fn fill(&self, ctx: &Ctx, drop: &[DropStep], base: *mut u8, rng: &mut u64) {
        for step in drop {
            match step {
                DropStep::Finalize => {}
                DropStep::Release { offset } | DropStep::Environment { offset } => {
                    let leaf = super::alloc_block(ctx.0, self.leaf, 8);
                    unsafe {
                        base.add(*offset as usize)
                            .cast::<*mut Header>()
                            .write_unaligned(leaf)
                    };
                }
                DropStep::ReleaseReceipt { .. } => {
                    panic!("ordinary generated corpus cannot manufacture private receipt custody")
                }
                DropStep::Thaw { .. } | DropStep::ReleaseRoot { .. } => {
                    panic!("generated layouts hold no Shared root")
                }
                DropStep::CloseHandle { offset } => {
                    let id = 1_000_000 + next(rng) % 1_000_000;
                    unsafe { base.add(*offset as usize).cast::<u64>().write_unaligned(id) };
                }
                DropStep::Child { offset, layout } => {
                    let child = &self.table.layout(*layout).unwrap().drop;
                    self.fill(ctx, child, unsafe { base.add(*offset as usize) }, rng);
                }
                DropStep::ActiveVariant {
                    tag_offset,
                    form,
                    variants,
                } => {
                    let pick = next(rng) as usize % (variants.len() + 1);
                    let at = unsafe { base.add(*tag_offset as usize) };
                    match form {
                        TagForm::LiveBit { bit } => unsafe {
                            let before = at.cast::<u64>().read_unaligned();
                            let mask = 1u64 << bit;
                            at.cast::<u64>().write_unaligned(if pick < variants.len() {
                                before | mask
                            } else {
                                before & !mask
                            });
                        },
                        TagForm::Ordinal => {
                            let tag = variants
                                .get(pick)
                                .map_or(u32::MAX - 1, |(ordinal, _)| *ordinal);
                            unsafe { at.cast::<u32>().write_unaligned(tag) };
                        }
                        // The dataful variant's steps store its non-null word.
                        TagForm::Niche { .. } => unsafe {
                            at.cast::<*mut Header>()
                                .write_unaligned(std::ptr::null_mut())
                        },
                    }
                    if let Some((_, steps)) = variants.get(pick) {
                        self.fill(ctx, steps, base, rng);
                    }
                }
                DropStep::ElementsReverse {
                    count_offset,
                    offset,
                    stride,
                    element,
                } => {
                    let n = read(base, *count_offset);
                    for i in 0..n {
                        self.fill_layout(ctx, *element, base, offset + i * stride, rng);
                    }
                }
                DropStep::RepeatReverse {
                    offset,
                    count,
                    stride,
                    element,
                } => {
                    for i in 0..*count {
                        self.fill_layout(ctx, *element, base, offset + i * stride, rng);
                    }
                }
                DropStep::ColumnsReverse(c) => {
                    let (n, capacity) = (read(base, c.count_offset), read(base, c.capacity_offset));
                    for i in 0..n {
                        for column in &c.columns {
                            let at = c.data_offset + capacity * column.prefix + i * column.stride;
                            self.fill_layout(ctx, column.layout, base, at, rng);
                        }
                    }
                }
            }
        }
    }

    fn fill_layout(&self, ctx: &Ctx, id: LayoutId, base: *mut u8, at: u64, rng: &mut u64) {
        let drop = &self.table.layout(id).unwrap().drop;
        self.fill(ctx, drop, unsafe { base.add(at as usize) }, rng);
    }

    /// The events releasing the filled value at `base` must produce, in order: a direct walk of
    /// the nested drop program, independent of the lowering.
    pub fn expected(&self, id: LayoutId, base: *mut u8, out: &mut Vec<Event>) {
        let drop = &self.table.layout(id).unwrap().drop;
        self.walk(drop, base, out);
    }

    fn walk(&self, drop: &[DropStep], base: *mut u8, out: &mut Vec<Event>) {
        for step in drop {
            match step {
                DropStep::Finalize => out.push(Event::Finalize(base as u64)),
                DropStep::Release { offset } | DropStep::Environment { offset } => {
                    let child = unsafe {
                        base.add(*offset as usize)
                            .cast::<*mut Header>()
                            .read_unaligned()
                    };
                    out.push(Event::Finalize(super::payload(child) as u64));
                }
                DropStep::CloseHandle { offset } => out.push(Event::Close(read(base, *offset))),
                DropStep::ReleaseReceipt { .. } => {
                    panic!("ordinary generated corpus cannot manufacture private receipt custody")
                }
                DropStep::Thaw { .. } | DropStep::ReleaseRoot { .. } => {
                    panic!("generated layouts hold no Shared root")
                }
                DropStep::Child { offset, layout } => {
                    self.expected(*layout, unsafe { base.add(*offset as usize) }, out)
                }
                DropStep::ActiveVariant {
                    tag_offset,
                    form,
                    variants,
                } => {
                    let at = unsafe { base.add(*tag_offset as usize) };
                    let word = match form {
                        TagForm::Ordinal => u64::from(unsafe { at.cast::<u32>().read_unaligned() }),
                        TagForm::LiveBit { .. } => unsafe { at.cast::<u64>().read_unaligned() },
                        TagForm::Niche { .. } => {
                            (unsafe { at.cast::<*mut Header>().read_unaligned() }) as u64
                        }
                    };
                    let tag = form.active(word);
                    if let Some((_, steps)) = variants.iter().find(|(o, _)| *o == tag) {
                        self.walk(steps, base, out);
                    }
                }
                DropStep::ElementsReverse {
                    count_offset,
                    offset,
                    stride,
                    element,
                } => {
                    for i in (0..read(base, *count_offset)).rev() {
                        self.expected(
                            *element,
                            unsafe { base.add((offset + i * stride) as usize) },
                            out,
                        );
                    }
                }
                DropStep::RepeatReverse {
                    offset,
                    count,
                    stride,
                    element,
                } => {
                    for i in (0..*count).rev() {
                        self.expected(
                            *element,
                            unsafe { base.add((offset + i * stride) as usize) },
                            out,
                        );
                    }
                }
                DropStep::ColumnsReverse(c) => {
                    let (n, capacity) = (read(base, c.count_offset), read(base, c.capacity_offset));
                    for i in (0..n).rev() {
                        for column in c.columns.iter().rev() {
                            let at = c.data_offset + capacity * column.prefix + i * column.stride;
                            self.expected(column.layout, unsafe { base.add(at as usize) }, out);
                        }
                    }
                }
            }
        }
    }

    /// Live blocks of the context.
    pub fn stats_live(&self, ctx: &Ctx) -> u64 {
        ctx.stats().live_blocks
    }

    /// Whether any lowered descriptor uses `kind`.
    pub fn uses(&self, kind: StepKind) -> bool {
        self.descriptors
            .values()
            .any(|d| d.steps.iter().any(|s| s.kind == kind))
    }
}

fn read(base: *mut u8, offset: u64) -> u64 {
    unsafe { base.add(offset as usize).cast::<u64>().read_unaligned() }
}

/// A deterministic xorshift generator.
pub fn next(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

/// Derived layouts for finalizer tests.
pub struct Resources {
    pub derived: Derived,
    /// The payload of `Box[Res]`, where `Res` is a resource with a finalizer whose fields are a
    /// `Box[u64]`, a closing `Socket` handle and a `Box[f64]`.
    pub boxed: LayoutId,
}

/// Derive [`Resources`]; `unsafe_ffi` declares `Res` in an `unsafe ffi` package.
pub fn resources(unsafe_ffi: bool) -> Resources {
    use hirsel_types::Primitive::{F64, U64};
    let mut owners = Owners::default();
    let socket = handle(&mut owners, "Socket", true);
    let res = nominal(
        &mut owners,
        "Res",
        Nominal {
            resource: true,
            finalizer: true,
            unsafe_ffi,
            ..Nominal::default()
        },
        |t| {
            let (u, f) = (t.p(U64), t.p(F64));
            let a = t.boxed(u);
            let s = t.named(&socket, Vec::new());
            let b = t.boxed(f);
            t.record(&[a, s, b])
        },
    );
    let mut t = ty();
    let r = t.named(&res, Vec::new());
    let boxed = t.boxed(r);
    let (derived, ids) = derive(&owners, &t.store, &[boxed]);
    Resources {
        boxed: derived.payload(ids[0]),
        derived,
    }
}
