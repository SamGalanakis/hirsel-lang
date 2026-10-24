use super::*;

#[test]
fn popped_heap_credit_moves_to_result_without_retain_or_retirement() {
    with_context(|ctx| unsafe {
        let source = heap(ctx, 2);
        count::hirsel_rt_v2_dup(ctx, source);
        let run = MetadataRun::new();
        let leaf = node(run, semantic::Origin::Heap, 3, &[]);
        let mut journal = Journal::new(&mut *ctx, run);
        let origin = semantic::Origin::Arena(journal.grant());
        let old = node(run, origin, 8, std::slice::from_ref(&leaf));
        let id = accept(journal.allocate(Update {
            value: old.clone(),
            additions: vec![source],
        }));
        let empty = node(run, origin, 0, &[]);
        let result = accept(journal.replace_transfer(
            id,
            &[],
            &[0],
            Update {
                value: empty.clone(),
                additions: vec![],
            },
        ));
        assert_eq!(result.transferred, vec![source]);
        assert_eq!(result.previous.generation(), old.generation());
        assert_eq!((*source).count, 2);
        assert!(journal.holders(id).unwrap().is_empty());
        assert_eq!(
            journal.inventory().seal_charge(&empty),
            Ok(Charge::Bytes(0))
        );
        let returned_metadata = result.previous.children().next().unwrap().retain();
        drop(result.previous);
        assert_eq!(returned_metadata.record().heap_charge(), Charge::Bytes(3));
        drop(journal);
        assert_eq!((*source).count, 2);
        retire(ctx, result.transferred[0]);
        assert_eq!((*source).count, 1);
        retire(ctx, source);
    });
}

#[test]
fn popped_shared_credit_never_enqueues_a_hidden_release() {
    with_context(|ctx| unsafe {
        let source = heap(ctx, 3);
        roots::hirsel_rt_v2_root_dup(ctx, source);
        let run = MetadataRun::new();
        let leaf = node(run, semantic::Origin::Shared, 0, &[]);
        let mut journal = Journal::new(&mut *ctx, run);
        journal.ctx.as_mut().roots.speculative = true;
        let origin = semantic::Origin::Arena(journal.grant());
        let old = node(run, origin, 8, &[leaf]);
        let id = accept(journal.allocate(Update {
            value: old,
            additions: vec![source],
        }));
        let result = accept(journal.replace_transfer(
            id,
            &[],
            &[0],
            Update {
                value: node(run, origin, 0, &[]),
                additions: vec![],
            },
        ));
        assert_eq!(roots::count(journal.ctx.as_ref(), source), 2);
        assert!(!journal.ctx.as_ref().roots.has_pending());
        drop(journal);
        assert!(!(*ctx).roots.has_pending());
        (*ctx).roots.speculative = false;
        roots::hirsel_rt_v2_root_release(ctx, result.transferred[0]);
        assert_eq!(roots::count(&*ctx, source), 1);
        roots::hirsel_rt_v2_root_release(ctx, source);
    });
}

#[test]
fn mutation_partitions_keep_take_retire_and_add_with_exact_multiplicity() {
    with_context(|ctx| unsafe {
        let source = heap(ctx, 2);
        for _ in 0..4 {
            count::hirsel_rt_v2_dup(ctx, source);
        }
        let run = MetadataRun::new();
        let leaf = node(run, semantic::Origin::Heap, 3, &[]);
        let mut journal = Journal::new(&mut *ctx, run);
        let origin = semantic::Origin::Arena(journal.grant());
        let old = node(run, origin, 24, &[leaf.clone(), leaf.clone(), leaf.clone()]);
        let id = accept(journal.allocate(Update {
            value: old,
            additions: vec![source, source, source],
        }));
        let next = node(run, origin, 16, &[leaf.clone(), leaf]);
        let result = accept(journal.replace_transfer(
            id,
            &[0],
            &[2],
            Update {
                value: next.clone(),
                additions: vec![source],
            },
        ));
        assert_eq!((*source).count, 4); // external + kept + addition + taken; omitted one retired
        assert_eq!(journal.holders(id).unwrap(), &[source, source]);
        assert_eq!(result.transferred, vec![source]);
        assert_eq!(journal.inventory().seal_charge(&next), Ok(Charge::Bytes(6)));
        drop(journal);
        assert_eq!((*source).count, 2);
        retire(ctx, result.transferred[0]);
        assert_eq!((*source).count, 1);
        retire(ctx, source);
    });
}

#[test]
fn invalid_or_overlapping_transfer_ordinals_preserve_input_and_old_custody() {
    with_context(|ctx| unsafe {
        let source = heap(ctx, 2);
        count::hirsel_rt_v2_dup(ctx, source);
        count::hirsel_rt_v2_dup(ctx, source);
        let run = MetadataRun::new();
        let mut journal = Journal::new(&mut *ctx, run);
        let origin = semantic::Origin::Arena(journal.grant());
        let old = node(run, origin, 8, &[]);
        let id = accept(journal.allocate(Update {
            value: old.clone(),
            additions: vec![source],
        }));
        for (keep, take) in [
            (vec![0], vec![0]),
            (vec![], vec![0, 0]),
            (vec![], vec![1]),
            (vec![], vec![1, 0]),
        ] {
            let Err((Error::InvalidRetention, input)) = journal.replace_transfer(
                id,
                &keep,
                &take,
                Update {
                    value: old.clone(),
                    additions: vec![source],
                },
            ) else {
                panic!("expected invalid transfer")
            };
            assert_eq!(input.additions, vec![source]);
            assert_eq!(input.value.generation(), old.generation());
            assert_eq!((*source).count, 3);
            assert_eq!(journal.holders(id).unwrap(), &[source]);
        }
        let wrong = node(run, semantic::Origin::Inline, 0, &[]);
        let Err((Error::Semantic(semantic::Error::WrongAllocationOrigin), input)) = journal
            .replace_transfer(
                id,
                &[],
                &[0],
                Update {
                    value: wrong,
                    additions: vec![source],
                },
            )
        else {
            panic!("expected wrong metadata rejection")
        };
        assert_eq!((*source).count, 3);
        assert_eq!(
            journal.inventory().entries().next().unwrap().1.generation(),
            old.generation()
        );
        retire(journal.ctx.as_ptr(), input.additions[0]);
        drop(journal);
        assert_eq!((*source).count, 1);
        retire(ctx, source);
    });
}

#[test]
fn entered_failure_cleanup_keeps_committed_mutation_and_releases_taken_result_once() {
    with_context(|ctx| unsafe {
        let source = heap(ctx, 3);
        roots::hirsel_rt_v2_root_dup(ctx, source);
        let run = MetadataRun::new();
        let leaf = node(run, semantic::Origin::Shared, 0, &[]);
        let mut journal = Journal::new(&mut *ctx, run);
        let origin = semantic::Origin::Arena(journal.grant());
        let old = node(run, origin, 8, &[leaf]);
        let id = accept(journal.allocate(Update {
            value: old,
            additions: vec![source],
        }));
        let empty = node(run, origin, 0, &[]);
        let result = accept(journal.replace_transfer(
            id,
            &[],
            &[0],
            Update {
                value: empty.clone(),
                additions: vec![],
            },
        ));
        // Model the entered body's ordinary local-result cleanup after a later
        // failure. The committed mutation/slot is not reset to its old receipt.
        roots::hirsel_rt_v2_root_release(journal.ctx.as_ptr(), result.transferred[0]);
        assert_eq!(roots::count(journal.ctx.as_ref(), source), 1);
        assert_eq!(
            journal.inventory().entries().next().unwrap().1.generation(),
            empty.generation()
        );
        drop(journal);
        assert_eq!(roots::count(&*ctx, source), 1);
        roots::hirsel_rt_v2_root_release(ctx, source);
    });
}

#[test]
fn demanded_guard_transfers_out_then_scope_close_does_not_retire_returned_credit() {
    with_context(|ctx| unsafe {
        let source = heap(ctx, 2);
        count::hirsel_rt_v2_dup(ctx, source);
        let run = MetadataRun::new();
        let mut guard = crate::sealed_region::guard::DemandedGuard::new(&mut *ctx, run);
        let origin = semantic::Origin::Arena(guard.grant().unwrap());
        let leaf = node(run, semantic::Origin::Heap, 3, &[]);
        let value = node(run, origin, 8, &[leaf]);
        let id = match guard.allocate(Update {
            value,
            additions: vec![source],
        }) {
            Ok(id) => id,
            Err((error, _)) => panic!("{error:?}"),
        };
        let result = match guard.replace_transfer(
            id,
            &[],
            &[0],
            Update {
                value: node(run, origin, 0, &[]),
                additions: vec![],
            },
        ) {
            Ok(result) => result,
            Err((error, _)) => panic!("{error:?}"),
        };
        assert_eq!(guard.close(0, 0), crate::STATUS_OK);
        assert_eq!((*source).count, 2);
        retire(ctx, result.transferred[0]);
        assert_eq!((*source).count, 1);
        retire(ctx, source);
    });
}
