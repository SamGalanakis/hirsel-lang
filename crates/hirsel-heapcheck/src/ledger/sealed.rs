//! Independent closed-storage cell audit before sealed ownership can commit.
//! This reads H allocations/cells, not receipt images, candidate drop programs,
//! runtime headers or the reference interpreter. It grants no storage custody.
use super::*;
use crate::violation::err;

impl Ledger {
    /// The live scope's storage must be a closed ownership graph, except for
    /// separately counted Shared roots. Every incoming physical cell from
    /// outside is forbidden, including an uncounted borrowed cell. The caller
    /// must separately audit SSA borrows, source correspondence and the affine
    /// scope/owner transition; this check does not implement SealArena.
    ///
    /// The returned Shared multiplicities count physical owning occurrences,
    /// never unique target addresses. No language or metadata count is changed.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "sealed execution is not linked yet")
    )]
    pub(crate) fn sealed_cells(&self, region: usize, word: u64) -> Checked<BTreeMap<Ptr, u64>> {
        if !matches!(word, 4 | 8) {
            return fail(V::Malformed, "invalid sealed cell pointer width");
        }
        let scope = self
            .regions
            .get(region)
            .ok_or_else(|| err(V::Malformed, "sealed storage has no scope"))?;
        if !scope.live {
            return fail(V::LiveRegion, "sealed storage scope is no longer live");
        }
        let mut listed = BTreeSet::new();
        for id in &scope.allocations {
            if !listed.insert(*id) {
                return fail(V::Malformed, "scope lists one storage allocation twice");
            }
            let allocation = self
                .lookup(*id)
                .ok_or_else(|| err(V::ForgedPointer, "scope lists unknown storage"))?;
            if allocation.owner != Owner::Region(region) {
                return fail(V::RegionEscape, "scope lists storage with another owner");
            }
            if !matches!(allocation.state, AllocState::Live | AllocState::Dead) {
                return fail(V::UseAfterFree, "scope storage is being drained or reused");
            }
        }
        // Derive the actual live ownership set from allocation records rather
        // than trusting the scope's candidate list or a journal of transfers.
        let inside: BTreeSet<_> = self
            .allocations
            .indexed()
            .filter(|(_, allocation)| {
                allocation.owner == Owner::Region(region) && allocation.state == AllocState::Live
            })
            .map(|(id, _)| AllocId(id as u32))
            .collect();
        if inside.iter().any(|id| !listed.contains(id)) {
            return fail(
                V::RegionEscape,
                "live scope storage is missing from its allocation list",
            );
        }
        let mut shared = BTreeMap::<Ptr, u64>::new();
        for (index, source) in self.allocations.indexed() {
            if source.state != AllocState::Live {
                continue;
            }
            let adopted = inside.contains(&AllocId(index as u32));
            if adopted && !source.extent.resources.is_empty() {
                return fail(V::TypeConfusion, "sealed storage contains resource cells");
            }
            for (&offset, cell) in &source.extent.cells {
                let Cell::Data {
                    ptr,
                    counted,
                    borrow,
                } = cell
                else {
                    continue;
                };
                // An outside source matters only if it reaches adopted storage;
                // unrelated frame/heap corruption belongs to the normal audit.
                if !adopted && !inside.contains(&ptr.alloc) {
                    continue;
                }
                source.extent.check_range(offset, word)?;
                if !source.extent.all_init(offset, word) {
                    return fail(
                        V::UninitializedRead,
                        "sealed boundary has an uninitialized pointer cell",
                    );
                }
                let target = self.check(*ptr)?;
                target.extent.check_range(ptr.offset, 0)?;
                if target.state != AllocState::Live {
                    return fail(
                        V::UseAfterFree,
                        "sealed boundary reaches storage being drained",
                    );
                }
                if !adopted {
                    return fail(
                        V::RegionEscape,
                        "outside physical cell reaches sealed storage",
                    );
                }
                if inside.contains(&ptr.alloc) {
                    continue;
                }
                if target.owner != Owner::Heap
                    || !matches!(target.count, Some(Count::Root(_)))
                    || !*counted
                    || borrow.is_some()
                    || ptr.offset != 0
                {
                    return fail(
                        V::RegionEscape,
                        "sealed storage retains an ordinary outgoing edge",
                    );
                }
                let occurrences = shared.entry(*ptr).or_default();
                *occurrences = occurrences
                    .checked_add(1)
                    .ok_or_else(|| err(V::CountMismatch, "Shared occurrence count overflow"))?;
            }
        }
        for (ptr, occurrences) in &shared {
            let Some(Count::Root(count)) = self.check(*ptr)?.count else {
                unreachable!()
            };
            if count < *occurrences {
                return fail(
                    V::CountMismatch,
                    "sealed Shared cells exceed actual root credits",
                );
            }
        }
        Ok(shared)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (Ledger, Ptr, Ptr, Ptr, Ptr) {
        let mut ledger = Ledger::default();
        ledger.regions.push(Region {
            instance: RegionInstanceId(0),
            live: true,
            allocations: Snapshot::default(),
            budget: None,
        });
        let first = ledger.alloc(Owner::Region(0), None, 16, 16, 8, None);
        let second = ledger.alloc(Owner::Region(0), None, 16, 16, 8, None);
        ledger.regions[0].allocations.push(first.alloc);
        ledger.regions[0].allocations.push(second.alloc);
        let shared = ledger.alloc(Owner::Heap, None, 8, 8, 8, Some(Count::Root(2)));
        let outside = ledger.alloc(Owner::Frame, None, 8, 8, 8, None);
        let data = |ptr, counted| Cell::Data {
            ptr,
            counted,
            borrow: None,
        };
        ledger
            .get_mut(first.alloc)
            .extent
            .put_cell(0, 8, data(second, false));
        ledger
            .get_mut(first.alloc)
            .extent
            .put_cell(8, 8, data(shared, true));
        ledger
            .get_mut(second.alloc)
            .extent
            .put_cell(0, 8, data(shared, true));
        (ledger, first, second, shared, outside)
    }
    fn rejected(ledger: &Ledger, kind: V) {
        assert_eq!(ledger.sealed_cells(0, 8).unwrap_err().kind, kind);
    }

    #[test]
    fn sealed_cells_count_repeated_shared_roots_without_acquiring_any_credit() {
        let (ledger, _, _, shared, _) = fixture();
        assert_eq!(
            ledger.sealed_cells(0, 8).unwrap(),
            BTreeMap::from([(shared, 2)])
        );
        assert_eq!(ledger.get(shared.alloc).count, Some(Count::Root(2)));
    }
    #[test]
    fn sealed_cells_refuse_ordinary_outgoing_heap_and_accept_its_closed_control() {
        let (mut ledger, first, second, _, _) = fixture();
        ledger.sealed_cells(0, 8).unwrap();
        let heap = ledger.alloc(Owner::Heap, None, 8, 8, 8, Some(Count::Finite(1)));
        ledger.get_mut(first.alloc).extent.put_cell(
            0,
            8,
            Cell::Data {
                ptr: heap,
                counted: true,
                borrow: None,
            },
        );
        rejected(&ledger, V::RegionEscape);
        ledger.get_mut(first.alloc).extent.put_cell(
            0,
            8,
            Cell::Data {
                ptr: second,
                counted: false,
                borrow: None,
            },
        );
        ledger.sealed_cells(0, 8).unwrap();
    }
    #[test]
    fn sealed_cells_refuse_external_borrowed_and_owning_incoming_cells() {
        for counted in [false, true] {
            let (mut ledger, first, _, _, outside) = fixture();
            ledger.sealed_cells(0, 8).unwrap();
            ledger.get_mut(outside.alloc).extent.put_cell(
                0,
                8,
                Cell::Data {
                    ptr: first,
                    counted,
                    borrow: (!counted).then_some(crate::exec::Borrow::Caller),
                },
            );
            rejected(&ledger, V::RegionEscape);
            ledger.get_mut(outside.alloc).extent.cells.clear();
            ledger.sealed_cells(0, 8).unwrap();
        }
    }
    #[test]
    fn sealed_cells_refuse_stale_generation_and_missing_shared_credit() {
        let (ledger, first, second, shared, _) = fixture();
        ledger.sealed_cells(0, 8).unwrap();
        let mut stale = ledger.clone();
        stale.get_mut(first.alloc).extent.put_cell(
            0,
            8,
            Cell::Data {
                ptr: Ptr {
                    generation: second.generation + 1,
                    ..second
                },
                counted: false,
                borrow: None,
            },
        );
        rejected(&stale, V::StaleGeneration);
        let mut missing = ledger.clone();
        missing.get_mut(shared.alloc).count = Some(Count::Root(1));
        rejected(&missing, V::CountMismatch);
        missing.get_mut(shared.alloc).count = Some(Count::Root(2));
        missing.sealed_cells(0, 8).unwrap();
    }
    #[test]
    fn sealed_cells_refuse_unlisted_foreign_owned_and_resource_storage() {
        let (ledger, first, _, _, _) = fixture();
        ledger.sealed_cells(0, 8).unwrap();
        for mutation in 0..4 {
            let mut bad = ledger.clone();
            match mutation {
                0 => {
                    bad.alloc(Owner::Region(0), None, 8, 8, 8, None);
                }
                1 => bad.get_mut(first.alloc).owner = Owner::Region(1),
                2 => {
                    bad.get_mut(first.alloc)
                        .extent
                        .resources
                        .insert(vec![], ResourceId(0));
                }
                3 => bad.regions[0].allocations.push(first.alloc),
                _ => unreachable!(),
            }
            rejected(
                &bad,
                match mutation {
                    2 => V::TypeConfusion,
                    3 => V::Malformed,
                    _ => V::RegionEscape,
                },
            );
        }
    }
    #[test]
    fn sealed_cells_refuse_dead_scope_and_nonowning_shared_boundary_cells() {
        let (ledger, first, _, shared, _) = fixture();
        ledger.sealed_cells(0, 8).unwrap();
        let mut dead = ledger.clone();
        dead.regions[0].live = false;
        rejected(&dead, V::LiveRegion);
        for borrowed in [false, true] {
            let mut bad = ledger.clone();
            bad.get_mut(first.alloc).extent.put_cell(
                8,
                8,
                Cell::Data {
                    ptr: shared,
                    counted: !borrowed,
                    borrow: Some(crate::exec::Borrow::Caller),
                },
            );
            rejected(&bad, V::RegionEscape);
        }
    }
    #[test]
    fn sealed_cells_refuse_unsettled_storage_and_invalid_pointer_cells() {
        let (ledger, first, second, shared, _) = fixture();
        for mutation in 0..5 {
            let mut bad = ledger.clone();
            let expected = match mutation {
                0 => {
                    bad.get_mut(first.alloc).state = AllocState::Dying;
                    V::UseAfterFree
                }
                1 => {
                    bad.get_mut(first.alloc).state = AllocState::Raw(TokenId(0));
                    V::UseAfterFree
                }
                2 => {
                    bad.get_mut(first.alloc).extent.put_cell(
                        0,
                        8,
                        Cell::Data {
                            ptr: Ptr {
                                offset: 17,
                                ..second
                            },
                            counted: false,
                            borrow: None,
                        },
                    );
                    V::OutOfExtent
                }
                3 => {
                    Arc::make_mut(&mut bad.get_mut(first.alloc).extent.init)[0] = false;
                    V::UninitializedRead
                }
                4 => {
                    bad.get_mut(shared.alloc).state = AllocState::Dead;
                    V::UseAfterFree
                }
                _ => unreachable!(),
            };
            rejected(&bad, expected);
        }
        assert_eq!(ledger.sealed_cells(0, 0).unwrap_err().kind, V::Malformed);
        ledger.sealed_cells(0, 8).unwrap();
    }
}
