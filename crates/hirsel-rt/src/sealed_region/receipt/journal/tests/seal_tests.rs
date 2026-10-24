use super::*;
use crate::region::{
    Region, hirsel_rt_v2_arena_bound, hirsel_rt_v2_region_enter, hirsel_rt_v2_region_exit,
};
use crate::sealed_region::transaction::Event;
use crate::sealed_region::{
    Block, Placement, Plan, Root, SealError as TransactionError, drop_owner, root,
};
use std::mem::MaybeUninit;

unsafe fn enter(ctx: *mut TaskCtx) -> Box<Region> {
    let mut region = Box::new(MaybeUninit::<Region>::uninit());
    unsafe {
        hirsel_rt_v2_region_enter(ctx, region.as_mut_ptr());
        Box::from_raw(Box::into_raw(region).cast())
    }
}
unsafe fn close(ctx: *mut TaskCtx, arena: &mut Region) {
    assert_eq!(
        unsafe { hirsel_rt_v2_region_exit(ctx, arena, 0, 0) },
        crate::STATUS_OK
    );
}
struct Case<'ctx> {
    journal: Journal<'ctx>,
    metadata: Receipt<Record>,
    scalar: Box<u64>,
    plan: Plan,
    source: *mut Header,
}
impl<'ctx> Case<'ctx> {
    /// The dead arena List's physical holder slots are elided, while its
    /// actual source credits and semantic allocation remain retained. The
    /// supplied physical plan still includes its shared ordinary leaf once.
    unsafe fn new(ctx: &'ctx mut TaskCtx) -> Self {
        let source = unsafe { heap(ctx, 2) };
        unsafe {
            count::hirsel_rt_v2_dup(ctx, source);
            count::hirsel_rt_v2_dup(ctx, source);
        }
        let run = MetadataRun::new();
        let leaf = node(run, semantic::Origin::Heap, 3, &[]);
        let mut journal = Journal::new(ctx, run);
        let retained = node(
            run,
            semantic::Origin::Arena(journal.grant()),
            16,
            &[leaf.clone(), leaf],
        );
        accept(unsafe {
            journal.allocate(Update {
                value: retained,
                additions: vec![source, source],
            })
        });
        let metadata = node(run, semantic::Origin::Inline, 0, &[]);
        let mut scalar = Box::new(7u64);
        let plan = Plan {
            root: Root {
                descriptor: 2,
                source: ptr::from_mut(&mut *scalar).cast(),
                extent: 8,
                align: 8,
                patches: vec![],
            },
            blocks: vec![Block {
                source,
                descriptor: 2,
                extent: 8,
                align: 8,
                placement: Placement::Evacuate,
                patches: vec![],
            }],
            shared: vec![],
            charge: Charge::Bytes(6),
        };
        Self {
            journal,
            metadata,
            scalar,
            plan,
            source,
        }
    }
}

#[test]
fn committed_journal_retires_source_once_and_owner_survives_original_guard_and_bytes() {
    with_context(|ctx| unsafe {
        let mut arena = enter(ctx);
        let case = Case::new(&mut *ctx);
        let cleanup = arena.cleanup;
        let owner = match case.journal.seal(&mut *arena, &case.metadata, &case.plan) {
            Ok(owner) => owner,
            Err((error, _)) => panic!("unexpected seal rejection: {error:?}"),
        };
        assert_eq!((*case.source).count, 1);
        assert_eq!(arena.cleanup, cleanup);
        drop(case.scalar);
        drop(case.metadata);
        close(ctx, &mut arena);
        drop(arena);
        assert_eq!(root(owner).cast::<u64>().read(), 7);
        drop_owner(ctx, owner);
        assert_eq!((*case.source).count, 1);
        retire(ctx, case.source);
    });
}

#[test]
fn exhausted_preflight_returns_same_journal_for_entered_cleanup() {
    with_context(|ctx| unsafe {
        let mut arena = enter(ctx);
        hirsel_rt_v2_arena_bound(ctx, &mut *arena, 21);
        arena.remaining -= 16; // already committed semantic list construction
        let case = Case::new(&mut *ctx);
        let cleanup = arena.cleanup;
        let before = case
            .journal
            .inventory()
            .entries()
            .next()
            .unwrap()
            .1
            .generation();
        let Err((SealError::Transaction(TransactionError::Exhausted), journal)) =
            case.journal.seal(&mut *arena, &case.metadata, &case.plan)
        else {
            panic!("expected seal preflight exhaustion")
        };
        assert_eq!(arena.remaining, 5);
        assert_eq!(arena.cleanup, cleanup);
        assert_eq!((*case.source).count, 3);
        assert_eq!(
            journal.inventory().entries().next().unwrap().1.generation(),
            before
        );
        assert_eq!(
            journal.inventory().seal_charge(&case.metadata),
            Ok(Charge::Bytes(6))
        );
        drop(journal);
        assert_eq!((*case.source).count, 1);
        close(ctx, &mut arena);
        retire(ctx, case.source);
    });
}

#[test]
fn caller_physical_plan_cannot_understate_journal_charge() {
    with_context(|ctx| unsafe {
        let mut arena = enter(ctx);
        let mut case = Case::new(&mut *ctx);
        case.plan.charge = Charge::Bytes(0);
        let Err((SealError::ChargeMismatch, journal)) =
            case.journal.seal(&mut *arena, &case.metadata, &case.plan)
        else {
            panic!("expected independent charge mismatch")
        };
        assert_eq!((*case.source).count, 3);
        assert_eq!(journal.inventory().entries().len(), 1);
        drop(journal);
        assert_eq!((*case.source).count, 1);
        close(ctx, &mut arena);
        retire(ctx, case.source);
    });
}

#[test]
fn recoverable_staging_failure_never_disarms_journal_custody() {
    for failed in [Event::Owner, Event::Ready] {
        with_context(|ctx| unsafe {
            let mut arena = enter(ctx);
            let case = Case::new(&mut *ctx);
            let cleanup = arena.cleanup;
            let mut seen = false;
            let result =
                case.journal
                    .seal_with(&mut *arena, &case.metadata, &case.plan, &mut |event| {
                        if event == failed {
                            seen = true;
                            Err(TransactionError::Interrupted)
                        } else {
                            Ok(())
                        }
                    });
            let Err((SealError::Transaction(TransactionError::Interrupted), journal)) = result
            else {
                panic!("expected staging interruption")
            };
            assert!(seen);
            assert_eq!(arena.cleanup, cleanup);
            assert_eq!((*case.source).count, 3);
            assert_eq!(journal.inventory().entries().len(), 1);
            assert_eq!(journal.ctx.as_ref().region, ptr::from_mut(&mut *arena));
            drop(journal);
            assert_eq!((*case.source).count, 1);
            close(ctx, &mut arena);
            retire(ctx, case.source);
        });
    }
}

#[test]
fn destination_shared_credits_exist_before_journal_source_retirement() {
    with_context(|ctx| unsafe {
        let mut arena = enter(ctx);
        let source = heap(ctx, 3);
        roots::hirsel_rt_v2_root_dup(ctx, source);
        roots::hirsel_rt_v2_root_dup(ctx, source);
        let run = MetadataRun::new();
        let shared = node(run, semantic::Origin::Shared, 0, &[]);
        let mut journal = Journal::new(&mut *ctx, run);
        let retained = node(
            run,
            semantic::Origin::Arena(journal.grant()),
            16,
            &[shared.clone(), shared],
        );
        accept(journal.allocate(Update {
            value: retained,
            additions: vec![source, source],
        }));
        let metadata = node(run, semantic::Origin::Inline, 0, &[]);
        let mut scalar = 7u64;
        let plan = Plan {
            root: Root {
                descriptor: 2,
                source: ptr::from_mut(&mut scalar).cast(),
                extent: 8,
                align: 8,
                patches: vec![],
            },
            blocks: vec![],
            shared: vec![source, source],
            charge: Charge::Bytes(0),
        };
        let mut saw_ready = false;
        let result = journal.seal_with(&mut *arena, &metadata, &plan, &mut |event| {
            if event == Event::Ready {
                // Two source journal credits plus external alias plus TWO
                // private destination credits. No transient root-count one.
                assert_eq!((*source).count, 5);
                saw_ready = true;
            }
            Ok(())
        });
        let owner = match result {
            Ok(owner) => owner,
            Err((error, _)) => panic!("unexpected {error:?}"),
        };
        assert!(saw_ready);
        assert_eq!(roots::count(&*ctx, source), 3);
        close(ctx, &mut arena);
        assert_eq!(roots::count(&*ctx, source), 3);
        drop_owner(ctx, owner);
        assert_eq!(roots::count(&*ctx, source), 1);
        roots::hirsel_rt_v2_root_release(ctx, source);
    });
}

#[test]
fn shared_staging_rollback_restores_destination_but_preserves_source_journal() {
    with_context(|ctx| unsafe {
        let mut arena = enter(ctx);
        let source = heap(ctx, 3);
        roots::hirsel_rt_v2_root_dup(ctx, source);
        roots::hirsel_rt_v2_root_dup(ctx, source);
        let run = MetadataRun::new();
        let shared = node(run, semantic::Origin::Shared, 0, &[]);
        let mut journal = Journal::new(&mut *ctx, run);
        let retained = node(
            run,
            semantic::Origin::Arena(journal.grant()),
            16,
            &[shared.clone(), shared],
        );
        accept(journal.allocate(Update {
            value: retained,
            additions: vec![source, source],
        }));
        let metadata = node(run, semantic::Origin::Inline, 0, &[]);
        let mut scalar = 7u64;
        let plan = Plan {
            root: Root {
                descriptor: 2,
                source: ptr::from_mut(&mut scalar).cast(),
                extent: 8,
                align: 8,
                patches: vec![],
            },
            blocks: vec![],
            shared: vec![source, source],
            charge: Charge::Bytes(0),
        };
        let result = journal.seal_with(&mut *arena, &metadata, &plan, &mut |event| {
            if event == Event::Ready {
                assert_eq!((*source).count, 5);
                Err(TransactionError::Interrupted)
            } else {
                Ok(())
            }
        });
        let Err((SealError::Transaction(TransactionError::Interrupted), journal)) = result else {
            panic!("expected rollback")
        };
        assert_eq!(roots::count(journal.ctx.as_ref(), source), 3);
        drop(journal);
        assert_eq!(roots::count(&*ctx, source), 1);
        close(ctx, &mut arena);
        roots::hirsel_rt_v2_root_release(ctx, source);
    });
}
