//! Compiled drop glue: a LayoutRecord's drop program lowered to straight code, in place of the
//! runtime's walk of the step program `hirsel_lir::rt_program` lowers from the same record.
//!
//! A [`Walk`] emits a drop program over a value at an address: every step in listed order,
//! each counted holder releasing one credit. `Inst::Release` of an inline value uses it
//! directly. A counted block whose payload layout is [`covered`] is released through
//! `hirsel.release.<key>(ctx, block, anchor, site, budget) -> i8`, one private function per
//! layout and module, named by the layout's `LayoutIdentity`: it consumes one credit, and when that
//! was the last (the count is 1, T04 DropLast) it walks the payload and then returns the
//! storage through `hirsel_rt_v2_free`, so a parent is freed after its children, except for the
//! release that ends its program (the runtime's `STEP_RELEASE_LAST`, the order the runtime's
//! worklist frees in): nothing of the parent remains to run, so it is freed first. A tail child
//! of the function's own layout (a list's next link, a tree's last child) is then released by
//! looping back to the function's head, so a chain of any length releases in one frame. A
//! covered layout with an empty program has no function: its release is the count test and
//! the free, inline.
//!
//! The runtime keeps everything else. A holder whose block layout the record does not fix (a
//! slice's owner, a callable's environment) and a block whose layout has a finalizer, an
//! inline resource or structure-of-arrays columns use the same inline count edge. Their
//! final credit goes to `hirsel_rt_v2_release` with the descriptor their own header names.
//! Other children are released by native recursion under a budget of [`GLUE_MAX_DEPTH`]
//! nested glue frames; a block that dies with the budget spent goes to `hirsel_rt_v2_release`
//! too, whose worklist uses constant native stack and never calls glue, so the stack a
//! release needs is bounded whatever the data. Either way a child is drained before its
//! parent's next step, so glue and walk free the same blocks, close the same handles and run
//! the same finalizers in the same order.

use hirsel_lir::layout::payload_offset;
use hirsel_lir::layout_types::{
    DropStep, FieldLayout, LayoutId, LayoutKind, LayoutRecord, LayoutTable, SizeSpec,
};
use hirsel_lir::runtime_abi::{
    COUNT_ARENA_SHARED, COUNT_IMMORTAL, COUNT_OFFSET, COUNT_UNIQUE, LAYOUT_OFFSET,
};

use crate::context::{Access, BasicBlock, BinOp, Builder, Function, IntPredicate, Module, Value};
use crate::emit::{load_tag, low_bit};

/// Nested glue frames one release may hold before a dying child goes to the runtime's
/// worklist instead.
pub(crate) const GLUE_MAX_DEPTH: u64 = 64;

/// A static repeat of at most this many elements is unrolled; a longer one is a loop.
const UNROLL_MAX: u64 = 4;

/// A drop program no validated unit reaches glue with.
pub(crate) struct GlueError(pub(crate) String);

type Glued<T = ()> = Result<T, GlueError>;

fn invalid<T>(what: impl Into<String>) -> Glued<T> {
    Err(GlueError(what.into()))
}

/// The runtime entries glue calls, declared by the emitter that owns the import manifest.
#[derive(Clone, Copy)]
pub(crate) struct Runtime<'m> {
    /// `hirsel_rt_v2_release`.
    pub(crate) release: Function<'m>,
    /// `hirsel_rt_v2_free`.
    pub(crate) free: Function<'m>,
    /// `hirsel_rt_v2_close_handle`.
    pub(crate) close_handle: Function<'m>,
    /// `hirsel_rt_v2_root_drop`.
    pub(crate) root_release: Function<'m>,
    pub(crate) measure_release: Option<Function<'m>>,
    /// The program builds tracked arena blocks (MEM-6), which a release leaves alone.
    pub(crate) tracking: bool,
}

/// A release function whose body is still to be generated, with its entry block.
pub(crate) type Pending<'m> = (LayoutId, Function<'m>, BasicBlock<'m>);

/// A release function's name: keyed on its payload's `LayoutIdentity`, so it names no number the
/// program assigns.
fn name(record: &LayoutRecord) -> String {
    let digest = hirsel_lir::snapshot::digest("drop-glue", &record.key);
    let hex: String = digest
        .0
        .iter()
        .take(16)
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("hirsel.release.{hex}")
}

/// Whether glue releases a block of payload `layout` itself: the layout has no finalizer and
/// its drop program, through every inline child and element, has no step that needs the
/// runtime's finalization states or column frames.
pub(crate) fn covered(layouts: &LayoutTable, layout: LayoutId) -> bool {
    layouts
        .layout(layout)
        .is_some_and(|record| record.finalizer.is_none() && plain(layouts, &record.drop))
}

fn plain(layouts: &LayoutTable, steps: &[DropStep]) -> bool {
    steps.iter().all(|step| match step {
        // A Shared root's frozen payload thaws in the runtime's walk.
        DropStep::Finalize
        | DropStep::ColumnsReverse(_)
        | DropStep::Thaw { .. }
        | DropStep::ReleaseSealed { .. } => false,
        // A Shared root's release is a runtime call under the root count contract.
        DropStep::Release { .. }
        | DropStep::ReleaseRoot { .. }
        | DropStep::ReleaseReceipt { .. }
        | DropStep::Environment { .. }
        | DropStep::CloseHandle { .. } => true,
        DropStep::ActiveVariant { variants, .. } => {
            variants.iter().all(|(_, steps)| plain(layouts, steps))
        }
        DropStep::Child { layout, .. }
        | DropStep::ElementsReverse {
            element: layout, ..
        }
        | DropStep::RepeatReverse {
            element: layout, ..
        } => covered(layouts, *layout),
    })
}

/// The value a drop program runs over: its layout, and the active variant of every tagged
/// value entered on the way, keyed by the tag's offset in the value.
#[derive(Clone)]
pub(crate) struct Scope {
    layout: LayoutId,
    arms: Vec<(u64, u32)>,
}

impl Scope {
    pub(crate) fn of(layout: LayoutId) -> Self {
        Self {
            layout,
            arms: Vec::new(),
        }
    }
}

/// The payload layout of the counted block reference a `Release` step at `offset` drops,
/// when the scope's layout fixes it: the step is the drop program of a `Block` field found
/// through the fields of records and of the active variants. `None` leaves the block to
/// its own header's descriptor.
fn holder(layouts: &LayoutTable, scope: &Scope, offset: u64) -> Option<LayoutId> {
    let (mut layout, mut base) = (scope.layout, 0u64);
    loop {
        let record = layouts.layout(layout)?;
        if let LayoutKind::Block { payload }
        | LayoutKind::Box { payload }
        | LayoutKind::Shared { payload } = record.kind
        {
            return (offset == base).then_some(payload);
        }
        let fields: &[FieldLayout] = match &record.tag {
            Some(tag) => {
                let at = base + tag.offset;
                let (_, active) = scope.arms.iter().find(|(tag, _)| *tag == at)?;
                &tag.variants.iter().find(|v| v.ordinal == *active)?.fields
            }
            None => &record.fields,
        };
        let field = fields.iter().find(|field| {
            let start = base + field.offset;
            match layouts.layout(field.layout).map(|child| &child.size) {
                Some(SizeSpec::Fixed(size)) => start <= offset && offset - start < *size,
                _ => false,
            }
        })?;
        layout = field.layout;
        base += field.offset;
    }
}

/// Whether releasing a block of covered payload `layout` can never fail: every holder its
/// program reaches, through blocks too, has a covered layout the record fixes, and none is
/// a handle. Such a release runs no finalizer and closes nothing, in glue or in the walk,
/// so its status is always 0 and no frame carries one.
fn infallible(layouts: &LayoutTable, layout: LayoutId) -> bool {
    fn quiet(
        layouts: &LayoutTable,
        scope: &Scope,
        steps: &[DropStep],
        seen: &mut Vec<LayoutId>,
    ) -> bool {
        let inline = |layout: LayoutId, seen: &mut Vec<LayoutId>| {
            layouts
                .layout(layout)
                .is_some_and(|record| quiet(layouts, &Scope::of(layout), &record.drop, seen))
        };
        steps.iter().all(|step| match step {
            DropStep::Release { offset } => match holder(layouts, scope, *offset) {
                Some(payload) if covered(layouts, payload) => {
                    seen.contains(&payload) || {
                        seen.push(payload);
                        inline(payload, seen)
                    }
                }
                _ => false,
            },
            // Mutex wrappers are atomic roots too, and their last close may fail.
            DropStep::ReleaseRoot { offset } => holder(layouts, scope, *offset)
                .and_then(|payload| layouts.layout(payload))
                .is_some_and(|record| {
                    matches!(record.kind, LayoutKind::Frozen { value }
                    if layouts.layout(value).is_some_and(|value| !matches!(value.kind, LayoutKind::Handle | LayoutKind::HostCapability { .. })))
                }),
            DropStep::ActiveVariant {
                tag_offset,
                variants,
                ..
            } => variants.iter().all(|(variant, steps)| {
                let mut inner = scope.clone();
                inner.arms.push((*tag_offset, *variant));
                quiet(layouts, &inner, steps, seen)
            }),
            DropStep::Child { layout, .. }
            | DropStep::ElementsReverse {
                element: layout, ..
            }
            | DropStep::RepeatReverse {
                element: layout, ..
            } => inline(*layout, seen),
            DropStep::ReleaseReceipt { .. } => true,
            DropStep::ReleaseSealed { .. }
            | DropStep::Finalize
            | DropStep::Environment { .. }
            | DropStep::CloseHandle { .. }
            | DropStep::Thaw { .. }
            | DropStep::ColumnsReverse(_) => false,
        })
    }
    let mut seen = vec![layout];
    covered(layouts, layout)
        && layouts
            .layout(layout)
            .is_some_and(|record| quiet(layouts, &Scope::of(layout), &record.drop, &mut seen))
}

/// The inline edge of the count word contract (`hirsel_lir::runtime_abi::COUNT_OFFSET`): one
/// credit of `block` is consumed. T04 DropLast, T16 DropEdge — at `COUNT_UNIQUE` the block
/// dies and the builder is left where the caller drops its fields and frees it. T04
/// DropImmortal — an immortal count, or a tracked arena block's (MEM-6, runtime ABI 11), is not
/// written. T04 Drop — any other count is stored one lower. Returns the block the last two
/// continue in.
fn drop_edge<'m>(
    module: &'m Module<'m>,
    builder: &Builder<'m>,
    function: Function<'m>,
    block: Value<'m>,
    measurement: Option<(Function<'m>, Value<'m>)>,
    tracking: bool,
) -> BasicBlock<'m> {
    let i64_ty = module.int_type(32);
    let int = |n: u64| module.const_int(i64_ty, u128::from(n));
    let [dying, shared, lower, done] = std::array::from_fn(|_| module.append_block(function));
    let word = if COUNT_OFFSET == 0 {
        block
    } else {
        builder.offset(block, int(COUNT_OFFSET))
    };
    let count = builder.load_access(i64_ty, word, 4, Access::Count);
    let unique = builder.icmp(IntPredicate::Eq, count, int(COUNT_UNIQUE));
    builder.cond_br(unique, dying, shared);
    builder.position(shared);
    if let Some((hook, context)) = measurement {
        builder.call(hook, &[context, block]);
    }
    let immortal = if tracking {
        builder.icmp(IntPredicate::Uge, count, int(COUNT_ARENA_SHARED))
    } else {
        builder.icmp(IntPredicate::Eq, count, int(COUNT_IMMORTAL))
    };
    builder.cond_br(immortal, done, lower);
    builder.position(lower);
    builder.store_access(
        builder.binary(BinOp::Sub, count, int(1)),
        word,
        4,
        Access::Count,
    );
    builder.br(done);
    builder.position(dying);
    done
}

/// The release function of covered payload `layout`, declared (and queued for its body) if
/// new. A queued function already has its entry block, so the module counts it as defined.
fn declare<'m>(
    module: &'m Module<'m>,
    layouts: &LayoutTable,
    layout: LayoutId,
    pending: &mut Vec<Pending<'m>>,
) -> Glued<Function<'m>> {
    let (ptr, i64_ty) = (module.ptr_type(), module.int_type(64));
    let ty = module.function_type(module.int_type(8), &[ptr, ptr, i64_ty, i64_ty, i64_ty]);
    let Some(record) = layouts.layout(layout) else {
        return invalid(format!("no layout {}", layout.0));
    };
    let name = name(record);
    let fresh = !module.defines(&name);
    let function = module.function(&name, ty).map_err(GlueError)?;
    if fresh {
        module.make_private(function);
        pending.push((layout, function, module.append_block(function)));
    }
    Ok(function)
}

/// Generates the body of every queued release function, and of those they call.
///
/// A program whose last step releases a block of its own layout (a list's tail, a tree's
/// last child) loops instead of recursing: the dying block is freed and its tail child takes
/// its place at the loop head, so a chain of any length releases in one frame and never
/// reaches the runtime's worklist.
pub(crate) fn define<'m>(
    module: &'m Module<'m>,
    layouts: &LayoutTable,
    runtime: Runtime<'m>,
    mut pending: Vec<Pending<'m>>,
) -> Glued {
    while let Some((layout, function, entry)) = pending.pop() {
        let Some(record) = layouts.layout(layout) else {
            return invalid(format!("no layout {}", layout.0));
        };
        let Ok(payload) = payload_offset(record.align) else {
            return invalid("a payload offset out of range");
        };
        let builder = Builder::new(module);
        builder.position(entry);
        let (i8_ty, i64_ty, ptr_ty) = (module.int_type(8), module.int_type(64), module.ptr_type());
        let int = |n: u64| module.const_int(i64_ty, u128::from(n));
        let status = builder.alloca(i8_ty, 1);
        builder.store(module.const_int(i8_ty, 0), status, 1);
        let (context, first, anchor, operation_site, budget) = (
            module.parameter(function, 0),
            module.parameter(function, 1),
            module.parameter(function, 2),
            module.parameter(function, 3),
            module.parameter(function, 4),
        );
        // The loop head: the block whose credit is consumed next.
        let head = module.append_block(function);
        builder.br(head);
        builder.position(head);
        let block = builder.phi(ptr_ty);
        builder.add_incoming(block, first, entry);
        let [spent, drop] = std::array::from_fn(|_| module.append_block(function));
        let done = drop_edge(
            module,
            &builder,
            function,
            block,
            runtime.measure_release.map(|hook| (hook, context)),
            runtime.tracking,
        );
        builder.cond_br(builder.icmp(IntPredicate::Eq, budget, int(0)), spent, drop);
        // The budget of nested glue frames is spent: the runtime's worklist drains the block.
        // A loop never spends it, so this is the first block.
        builder.position(spent);
        let descriptor = builder.load(
            module.int_type(32),
            builder.offset(block, int(LAYOUT_OFFSET)),
            4,
        );
        let descriptor = builder.binary(
            BinOp::And,
            descriptor,
            module.const_int(module.int_type(32), 0x1fffffff),
        );
        let null = module.const_zero(ptr_ty);
        let drained = builder.call(
            runtime.release,
            &[context, block, descriptor, null, anchor, operation_site],
        );
        let infallible = infallible(layouts, layout);
        builder.ret(if infallible {
            module.const_int(i8_ty, 0)
        } else {
            drained
        });
        builder.position(drop);
        // At a spent budget the runtime records the dying edge instead, exactly once.
        if let Some(hook) = runtime.measure_release {
            builder.call(hook, &[context, block]);
        }
        let mut walk = Walk {
            module,
            layouts,
            runtime,
            function,
            builder: &builder,
            context,
            anchor,
            operation_site,
            budget: builder.binary(BinOp::Sub, budget, int(1)),
            status,
            pending: &mut pending,
            looping: Some(Loop {
                layout,
                head,
                block,
            }),
        };
        walk.tail_steps(
            &Scope::of(layout),
            builder.offset(block, int(payload)),
            record.align,
            &record.drop,
        )?;
        // T16 Free, after every child has drained.
        builder.call(
            crate::runtime::free_fast(module, runtime.free),
            &[context, block],
        );
        builder.ret(builder.load(i8_ty, status, 1));
        // The tail child survives: the blocks before it accumulated the status.
        builder.position(done);
        builder.ret(if infallible {
            module.const_int(i8_ty, 0)
        } else {
            builder.load(i8_ty, status, 1)
        });
    }
    Ok(())
}

/// The loop of a release function: its payload layout, its head and the block it releases.
#[derive(Clone, Copy)]
pub(crate) struct Loop<'m> {
    layout: LayoutId,
    head: BasicBlock<'m>,
    block: Value<'m>,
}

/// Emits drop programs at a builder's position in `function`.
pub(crate) struct Walk<'m, 'g> {
    pub(crate) module: &'m Module<'m>,
    pub(crate) layouts: &'g LayoutTable,
    pub(crate) runtime: Runtime<'m>,
    pub(crate) function: Function<'m>,
    pub(crate) builder: &'g Builder<'m>,
    pub(crate) context: Value<'m>,
    /// The cleanup anchor every finalizer and close of this release reports.
    pub(crate) anchor: Value<'m>,
    pub(crate) operation_site: Value<'m>,
    /// Nested glue frames a child released here may still hold (an `i64`).
    pub(crate) budget: Value<'m>,
    /// An `i8` in the frame that every runtime status is or-ed into: a failing leaf does
    /// not stop the walk.
    pub(crate) status: Value<'m>,
    pub(crate) pending: &'g mut Vec<Pending<'m>>,
    /// The release function's loop, when this walk is its body.
    pub(crate) looping: Option<Loop<'m>>,
}

impl<'m, 'g> Walk<'m, 'g> {
    fn int(&self, n: u64) -> Value<'m> {
        self.module
            .const_int(self.module.int_type(64), u128::from(n))
    }

    fn at(&self, base: Value<'m>, offset: u64) -> Value<'m> {
        if offset == 0 {
            base
        } else {
            self.builder.offset(base, self.int(offset))
        }
    }

    fn block(&self) -> BasicBlock<'m> {
        self.module.append_block(self.function)
    }

    fn accumulate(&self, result: Value<'m>) {
        let i8_ty = self.module.int_type(8);
        let before = self.builder.load(i8_ty, self.status, 1);
        self.builder.store(
            self.builder.binary(BinOp::Or, before, result),
            self.status,
            1,
        );
    }

    /// Consumes one credit of `block`, whose payload layout is `payload` when the releasing
    /// program fixes it.
    pub(crate) fn release(&mut self, block: Value<'m>, payload: Option<LayoutId>) -> Glued {
        let ptr_ty = self.module.ptr_type();
        let Some(payload) = payload.filter(|payload| covered(self.layouts, *payload)) else {
            // The count word's contract is independent of the payload layout. Only
            // the final credit needs its descriptor and the runtime's drop walk.
            let done = drop_edge(
                self.module,
                self.builder,
                self.function,
                block,
                self.runtime
                    .measure_release
                    .map(|hook| (hook, self.context)),
                self.runtime.tracking,
            );
            // The runtime accounts for this dying edge, exactly once.
            // The header's own descriptor also serves an erased environment.
            let descriptor = self.builder.load(
                self.module.int_type(32),
                self.builder.offset(block, self.int(LAYOUT_OFFSET)),
                4,
            );
            let descriptor = self.builder.binary(
                BinOp::And,
                descriptor,
                self.module.const_int(self.module.int_type(32), 0x1fffffff),
            );
            let null = self.module.const_zero(ptr_ty);
            let arguments = [
                self.context,
                block,
                descriptor,
                null,
                self.anchor,
                self.operation_site,
            ];
            self.accumulate(self.builder.call(self.runtime.release, &arguments));
            self.builder.br(done);
            self.builder.position(done);
            return Ok(());
        };
        let leaf = self
            .layouts
            .layout(payload)
            .is_some_and(|record| record.drop.is_empty());
        if !leaf {
            let glue = declare(self.module, self.layouts, payload, self.pending)?;
            let arguments = [
                self.context,
                block,
                self.anchor,
                self.operation_site,
                self.budget,
            ];
            let status = self.builder.call(glue, &arguments);
            if !infallible(self.layouts, payload) {
                self.accumulate(status);
            }
            return Ok(());
        }
        // Nothing to walk: T16 Free when it was the last credit.
        let done = drop_edge(
            self.module,
            self.builder,
            self.function,
            block,
            self.runtime
                .measure_release
                .map(|hook| (hook, self.context)),
            self.runtime.tracking,
        );
        if let Some(hook) = self.runtime.measure_release {
            self.builder.call(hook, &[self.context, block]);
        }
        self.builder.call(
            crate::runtime::free_fast(self.module, self.runtime.free),
            &[self.context, block],
        );
        self.builder.br(done);
        self.builder.position(done);
        Ok(())
    }

    /// The release of `child` that ends the release function's program (the runtime's
    /// `STEP_RELEASE_LAST`): the function's block runs nothing more, so it is freed first.
    /// A child of the function's own layout is then released at the loop head; any other
    /// child is released here and the function returns. Outside a release function it is an
    /// ordinary release.
    fn tail_release(&mut self, child: Value<'m>, payload: Option<LayoutId>) -> Glued {
        let Some(looping) = self.looping else {
            return self.release(child, payload);
        };
        self.builder.call(
            crate::runtime::free_fast(self.module, self.runtime.free),
            &[self.context, looping.block],
        );
        if payload == Some(looping.layout) {
            self.builder
                .add_incoming(looping.block, child, self.builder.current());
            self.builder.br(looping.head);
        } else {
            self.release(child, payload)?;
            let i8_ty = self.module.int_type(8);
            self.builder.ret(self.builder.load(i8_ty, self.status, 1));
        }
        // Whatever the caller emits next is unreachable.
        let after = self.block();
        self.builder.position(after);
        Ok(())
    }

    /// Hints that the header of the block reference at `at` is read soon, so its miss
    /// overlaps the release before it.
    fn prefetch(&self, at: Value<'m>, align: u32) -> Glued {
        let ptr_ty = self.module.ptr_type();
        let i32_ty = self.module.int_type(32);
        let prefetch = self
            .module
            .intrinsic("llvm.prefetch", &[ptr_ty])
            .map_err(GlueError)?;
        let block = self.builder.load(ptr_ty, at, align);
        let int = |n: u32| self.module.const_int(i32_ty, u128::from(n));
        // Read access, highest locality, data cache.
        self.builder
            .call(prefetch, &[block, int(0), int(3), int(1)]);
        Ok(())
    }

    /// The drop program `steps` over the value of `scope` at `base`, aligned to `align`.
    pub(crate) fn steps(
        &mut self,
        scope: &Scope,
        base: Value<'m>,
        align: u32,
        steps: &[DropStep],
    ) -> Glued {
        self.steps_at(scope, base, align, steps, false)
    }

    /// The release function's whole program: its last step may continue the loop.
    fn tail_steps(
        &mut self,
        scope: &Scope,
        base: Value<'m>,
        align: u32,
        steps: &[DropStep],
    ) -> Glued {
        self.steps_at(scope, base, align, steps, true)
    }

    /// `steps`, the last of which ends the release function's program when `tail`.
    fn steps_at(
        &mut self,
        scope: &Scope,
        base: Value<'m>,
        align: u32,
        steps: &[DropStep],
        tail: bool,
    ) -> Glued {
        for (index, step) in steps.iter().enumerate() {
            if let (DropStep::Release { offset }, Some(DropStep::Release { offset: next })) =
                (step, steps.get(index + 1))
            {
                // A child with a program of its own may take long: start the next
                // sibling's header load before it.
                let walks = holder(self.layouts, scope, *offset)
                    .and_then(|payload| self.layouts.layout(payload))
                    .is_some_and(|record| !record.drop.is_empty());
                if walks {
                    self.prefetch(self.at(base, *next), align.min(low_bit(*next)).min(8))?;
                }
            }
            self.step_at(scope, base, align, step, tail && index + 1 == steps.len())?;
        }
        Ok(())
    }

    /// One step of a drop program over the value of `scope` at `base`.
    pub(crate) fn step(
        &mut self,
        scope: &Scope,
        base: Value<'m>,
        align: u32,
        step: &DropStep,
    ) -> Glued {
        self.step_at(scope, base, align, step, false)
    }

    /// `step`, which ends the release function's program when `tail`.
    fn step_at(
        &mut self,
        scope: &Scope,
        base: Value<'m>,
        align: u32,
        step: &DropStep,
        tail: bool,
    ) -> Glued {
        let (ptr_ty, i64_ty) = (self.module.ptr_type(), self.module.int_type(64));
        let word = |offset: u64| align.min(low_bit(offset)).min(8);
        match step {
            DropStep::Release { offset } => {
                let block = self
                    .builder
                    .load(ptr_ty, self.at(base, *offset), word(*offset));
                let payload = holder(self.layouts, scope, *offset);
                if tail {
                    return self.tail_release(block, payload);
                }
                self.release(block, payload)
            }
            DropStep::Environment { offset } => {
                let block = self
                    .builder
                    .load(ptr_ty, self.at(base, *offset), word(*offset));
                if tail {
                    return self.tail_release(block, None);
                }
                self.release(block, None)
            }
            DropStep::ReleaseRoot { offset } => {
                // Never a tail release: the runtime may defer it, and the walk frees the
                // releasing block after it, as `STEP_RELEASE_ROOT` does.
                let block = self
                    .builder
                    .load(ptr_ty, self.at(base, *offset), word(*offset));
                self.accumulate(self.builder.call(
                    self.runtime.root_release,
                    &[self.context, block, self.anchor, self.operation_site],
                ));
                Ok(())
            }
            DropStep::CloseHandle { offset } => {
                // The Io adapter closes the handle, shielded; a failing close is recorded and
                // the walk goes on.
                let handle = self
                    .builder
                    .load(i64_ty, self.at(base, *offset), word(*offset));
                let arguments = [self.context, handle, self.anchor, self.operation_site];
                self.accumulate(self.builder.call(self.runtime.close_handle, &arguments));
                Ok(())
            }
            DropStep::ReleaseSealed { .. } => {
                invalid("sealed owner drop requires linked runtime emission")
            }
            DropStep::ReleaseReceipt { offset } => {
                let receipt = self
                    .builder
                    .load(ptr_ty, self.at(base, *offset), word(*offset));
                let entry = hirsel_lir::runtime_abi::RuntimeEntry::ReceiptRelease;
                let release = self
                    .module
                    .function(
                        entry.declaration().symbol,
                        self.module
                            .function_type(self.module.void_type(), &[ptr_ty]),
                    )
                    .map_err(GlueError)?;
                crate::runtime::substrate(self.module, release, entry);
                self.builder.call(release, &[receipt]);
                Ok(())
            }
            DropStep::Finalize => invalid("compiled drop code that reaches a finalizer"),
            DropStep::Thaw { .. } => invalid("compiled drop code that reaches a frozen value"),
            DropStep::Child { offset, layout } => {
                let child = self.record(*layout)?;
                if child.finalizer.is_some() {
                    return invalid("compiled drop code that reaches a finalizer");
                }
                self.steps(
                    &Scope::of(*layout),
                    self.at(base, *offset),
                    word(*offset),
                    &child.drop,
                )
            }
            DropStep::ActiveVariant {
                tag_offset,
                form,
                variants,
            } => {
                let active = load_tag(self.builder, base, align, *tag_offset, *form, false);
                let done = self.block();
                let here = self.builder.current();
                let mut cases = Vec::new();
                for (variant, steps) in variants {
                    let arm = self.block();
                    self.builder.position(arm);
                    let mut inner = scope.clone();
                    inner.arms.push((*tag_offset, *variant));
                    self.steps_at(&inner, base, align, steps, tail)?;
                    self.builder.br(done);
                    cases.push((
                        self.module
                            .const_int(self.module.int_type(32), u128::from(*variant)),
                        arm,
                    ));
                }
                self.builder.position(here);
                self.builder.switch(active, done, &cases);
                self.builder.position(done);
                Ok(())
            }
            DropStep::RepeatReverse {
                offset,
                count,
                stride,
                element,
            } => {
                let drop = &self.record(*element)?.drop;
                let (scope, align) = (Scope::of(*element), word(*offset).min(low_bit(*stride)));
                if drop.is_empty() {
                    return Ok(());
                }
                if *count > UNROLL_MAX {
                    let first = self.at(base, *offset);
                    return self.elements(&scope, first, align, self.int(*count), *stride, drop);
                }
                for index in (0..*count).rev() {
                    let at = self.at(base, offset + index * stride);
                    self.steps(&scope, at, align, drop)?;
                }
                Ok(())
            }
            DropStep::ElementsReverse {
                count_offset,
                offset,
                stride,
                element,
            } => {
                let drop = &self.record(*element)?.drop;
                if drop.is_empty() {
                    return Ok(());
                }
                let count =
                    self.builder
                        .load(i64_ty, self.at(base, *count_offset), word(*count_offset));
                let (scope, align) = (Scope::of(*element), word(*offset).min(low_bit(*stride)));
                self.elements(&scope, self.at(base, *offset), align, count, *stride, drop)
            }
            DropStep::ColumnsReverse(_) => {
                invalid("compiled drop code for structure-of-arrays columns")
            }
        }
    }

    fn record(&self, layout: LayoutId) -> Glued<&'g LayoutRecord> {
        let layouts: &'g LayoutTable = self.layouts;
        match layouts.layout(layout) {
            Some(record) => Ok(record),
            None => invalid(format!("no layout {}", layout.0)),
        }
    }

    /// `drop` over each of `count` elements `stride` bytes apart from `first`, the highest
    /// index first.
    fn elements(
        &mut self,
        scope: &Scope,
        first: Value<'m>,
        align: u32,
        count: Value<'m>,
        stride: u64,
        drop: &[DropStep],
    ) -> Glued {
        let i64_ty = self.module.int_type(64);
        let [head, body, done] = std::array::from_fn(|_| self.block());
        let before = self.builder.current();
        self.builder.br(head);
        self.builder.position(head);
        let left = self.builder.phi(i64_ty);
        self.builder.add_incoming(left, count, before);
        let none = self.builder.icmp(IntPredicate::Eq, left, self.int(0));
        self.builder.cond_br(none, done, body);
        self.builder.position(body);
        let index = self.builder.binary(BinOp::Sub, left, self.int(1));
        let offset = self.builder.binary(BinOp::Mul, index, self.int(stride));
        let element = self.builder.offset(first, offset);
        self.steps(scope, element, align, drop)?;
        self.builder
            .add_incoming(left, index, self.builder.current());
        self.builder.br(head);
        self.builder.position(done);
        Ok(())
    }
}
