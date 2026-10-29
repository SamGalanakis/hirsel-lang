//! V's borrowed input. Leaf rows are decoded on demand and retained only for
//! this validation; no archived Definition, Body or arena vector is rebuilt.
use hirsel_product_codec::{TirBlockView, TirBodyView, TirMatchView, TirView};
use hirsel_tir::*;
use hirsel_types::{SiteRef, TypeId, UniqueSummary};
use std::{cell::OnceCell, ops::Index};

pub(super) enum Rows<'a, T> {
    Live(&'a [T]),
    Indexed {
        slots: Box<[OnceCell<Box<T>>]>,
        read: Box<dyn Fn(usize) -> T + 'a>,
    },
}
impl<'a, T: 'a> Rows<'a, T> {
    fn indexed(len: usize, read: impl Fn(usize) -> T + 'a) -> Self {
        Self::Indexed {
            slots: (0..len).map(|_| OnceCell::new()).collect(),
            read: Box::new(read),
        }
    }
    pub(super) fn len(&self) -> usize {
        match self {
            Self::Live(rows) => rows.len(),
            Self::Indexed { slots, .. } => slots.len(),
        }
    }
    pub(super) fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub(super) fn get(&self, n: usize) -> Option<&T> {
        match self {
            Self::Live(rows) => rows.get(n),
            Self::Indexed { slots, read } => Some(slots.get(n)?.get_or_init(|| Box::new(read(n)))),
        }
    }
    pub(super) fn iter(&self) -> impl DoubleEndedIterator<Item = &T> + ExactSizeIterator + Clone {
        (0..self.len()).map(|n| &self[n])
    }
    pub(super) fn contains(&self, value: &T) -> bool
    where
        T: PartialEq,
    {
        self.iter().any(|row| row == value)
    }
    pub(super) fn first(&self) -> Option<&T> {
        self.get(0)
    }
    pub(super) fn last(&self) -> Option<&T> {
        self.get(self.len().checked_sub(1)?)
    }
}
impl<T> Index<usize> for Rows<'_, T> {
    type Output = T;
    fn index(&self, n: usize) -> &T {
        self.get(n).expect("V checked arena index")
    }
}
impl<'a, 'b, T: 'a> IntoIterator for &'b Rows<'a, T> {
    type Item = &'b T;
    type IntoIter = std::iter::Map<std::ops::Range<usize>, Box<dyn FnMut(usize) -> &'b T + 'b>>;
    fn into_iter(self) -> Self::IntoIter {
        (0..self.len()).map(Box::new(move |n| &self[n]))
    }
}
impl<T: PartialEq> PartialEq<Vec<T>> for Rows<'_, T> {
    fn eq(&self, other: &Vec<T>) -> bool {
        self.iter().eq(other.iter())
    }
}

pub(super) struct ReadDefinition<'a> {
    pub(super) inferred_unique: UniqueSummary,
    pub(super) path: hirsel_syntax::DefId,
    pub(super) entry: BodyId,
    pub(super) caller_bodies: Rows<'a, BodyId>,
    pub(super) protocol_associations: Rows<'a, ProtocolAssociationReceipt>,
    pub(super) types: Rows<'a, TypeId>,
    pub(super) bodies: Rows<'a, ReadBody<'a>>,
    pub(super) contracts: Rows<'a, (u32, BodyId)>,
}
impl<'a> ReadDefinition<'a> {
    pub(super) fn live(value: &'a Definition) -> Self {
        Self::new(TirView::Live(value)).expect("live TIR bindings are present")
    }
    pub(super) fn new(view: TirView<'a>) -> Option<Self> {
        // Row framing, scalar encodings and every source position were checked
        // when opening the immutable archive. Resolve every declared type
        // reference once into its retained TypeRecords owner. Successful ids
        // remain cached there, so subsequent leaf-row decoding is infallible.
        if let TirView::Archived(archive) = view {
            archive.bind_types()?;
        }
        let header = view.header()?;
        Some(Self {
            inferred_unique: header.inferred_unique,
            path: header.path,
            entry: header.entry,
            caller_bodies: match view {
                TirView::Live(value) => Rows::Live(&value.caller_bodies),
                TirView::Archived(_) => Rows::indexed(view.caller_body_count(), move |n| {
                    view.caller_body(n)
                        .expect("bound checked TIR row")
                        .into_owned()
                }),
            },
            protocol_associations: match view {
                TirView::Live(value) => Rows::Live(&value.protocol_associations),
                TirView::Archived(_) => {
                    Rows::indexed(view.protocol_association_count(), move |n| {
                        view.protocol_association(n)
                            .expect("bound checked TIR row")
                            .into_owned()
                    })
                }
            },
            types: match view {
                TirView::Live(value) => Rows::Live(&value.types),
                TirView::Archived(_) => Rows::indexed(view.type_count(), move |n| {
                    view.type_id(n).expect("bound checked TIR row").into_owned()
                }),
            },
            contracts: match view {
                TirView::Live(value) => Rows::Live(&value.contracts),
                TirView::Archived(_) => Rows::indexed(view.contract_count(), move |n| {
                    view.contract(n)
                        .expect("bound checked TIR row")
                        .into_owned()
                }),
            },
            bodies: Rows::indexed(view.body_count(), move |n| {
                ReadBody::new(view.body(n).expect("checked body directory"))
            }),
        })
    }
}
pub(super) struct ReadBody<'a> {
    pub(super) arena_scope_grant: Option<ArenaScopeGrant>,
    pub(super) parameter_scope: SiteRef,
    pub(super) entry: BlockId,
    pub(super) result: TypeId,
    pub(super) locals: Rows<'a, Local>,
    pub(super) places: Rows<'a, Place>,
    pub(super) scopes: Rows<'a, Scope>,
    pub(super) rvalues: Rows<'a, Rvalue>,
    pub(super) cleanups: Rows<'a, Cleanup>,
    pub(super) cleanup_nodes: Rows<'a, CleanupNode>,
    pub(super) parameters: Rows<'a, LocalId>,
    pub(super) borrow_cursors: Rows<'a, BorrowCursor>,
    pub(super) tail_sites: Rows<'a, TailSite>,
    pub(super) scoped_parameters: Rows<'a, ScopedParameter>,
    pub(super) blocks: Rows<'a, ReadBlock<'a>>,
    pub(super) matches: Rows<'a, ReadMatch<'a>>,
}
impl<'a> ReadBody<'a> {
    fn new(view: TirBodyView<'a>) -> Self {
        let header = view.header().expect("bound checked TIR body header");
        Self {
            arena_scope_grant: header.arena_scope_grant,
            parameter_scope: header.parameter_scope,
            entry: header.entry,
            result: header.result,
            locals: match view {
                TirBodyView::Live(value) => Rows::Live(&value.locals),
                TirBodyView::Archived(_) => Rows::indexed(view.local_count(), move |n| {
                    view.local(n).expect("bound checked TIR row").into_owned()
                }),
            },
            places: match view {
                TirBodyView::Live(value) => Rows::Live(&value.places),
                TirBodyView::Archived(_) => Rows::indexed(view.place_count(), move |n| {
                    view.place(n).expect("bound checked TIR row").into_owned()
                }),
            },
            scopes: match view {
                TirBodyView::Live(value) => Rows::Live(&value.scopes),
                TirBodyView::Archived(_) => Rows::indexed(view.scope_count(), move |n| {
                    view.scope(n).expect("bound checked TIR row").into_owned()
                }),
            },
            rvalues: match view {
                TirBodyView::Live(value) => Rows::Live(&value.rvalues),
                TirBodyView::Archived(_) => Rows::indexed(view.rvalue_count(), move |n| {
                    view.rvalue(n).expect("bound checked TIR row").into_owned()
                }),
            },
            cleanups: match view {
                TirBodyView::Live(value) => Rows::Live(&value.cleanups),
                TirBodyView::Archived(_) => Rows::indexed(view.cleanup_count(), move |n| {
                    view.cleanup(n).expect("bound checked TIR row").into_owned()
                }),
            },
            cleanup_nodes: match view {
                TirBodyView::Live(value) => Rows::Live(&value.cleanup_nodes),
                TirBodyView::Archived(_) => Rows::indexed(view.cleanup_node_count(), move |n| {
                    view.cleanup_node(n)
                        .expect("bound checked TIR row")
                        .into_owned()
                }),
            },
            parameters: match view {
                TirBodyView::Live(value) => Rows::Live(&value.parameters),
                TirBodyView::Archived(_) => Rows::indexed(view.parameter_count(), move |n| {
                    view.parameter(n)
                        .expect("bound checked TIR row")
                        .into_owned()
                }),
            },
            borrow_cursors: match view {
                TirBodyView::Live(value) => Rows::Live(&value.borrow_cursors),
                TirBodyView::Archived(_) => Rows::indexed(view.borrow_cursor_count(), move |n| {
                    view.borrow_cursor(n)
                        .expect("bound checked TIR row")
                        .into_owned()
                }),
            },
            tail_sites: match view {
                TirBodyView::Live(value) => Rows::Live(&value.tail_sites),
                TirBodyView::Archived(_) => Rows::indexed(view.tail_site_count(), move |n| {
                    view.tail_site(n)
                        .expect("bound checked TIR row")
                        .into_owned()
                }),
            },
            scoped_parameters: match view {
                TirBodyView::Live(value) => Rows::Live(&value.scoped_parameters),
                TirBodyView::Archived(_) => {
                    Rows::indexed(view.scoped_parameter_count(), move |n| {
                        view.scoped_parameter(n)
                            .expect("bound checked TIR row")
                            .into_owned()
                    })
                }
            },
            blocks: Rows::indexed(view.block_count(), move |n| {
                ReadBlock::new(view.block(n).expect("checked block directory"))
            }),
            matches: Rows::indexed(view.match_table_count(), move |n| {
                ReadMatch::new(view.match_table(n).expect("checked match directory"))
            }),
        }
    }
    pub fn successors(&self, terminator: &Terminator) -> Vec<BlockId> {
        match terminator {
            Terminator::Goto(b) => vec![*b],
            Terminator::Match { table, from } => self
                .matches
                .get(table.0 as usize)
                .map(|t| {
                    t.arms
                        .iter()
                        .skip(*from as usize)
                        .map(|a| a.guard.unwrap_or(a.target))
                        .collect()
                })
                .unwrap_or_default(),
            Terminator::Branch { yes, no, .. } => vec![*yes, *no],
            Terminator::Switch {
                arms, otherwise, ..
            } => arms.iter().map(|a| a.1).chain([*otherwise]).collect(),
            Terminator::Call { returns, .. } => returns.iter().map(|r| r.next).collect(),
            Terminator::Return { .. } | Terminator::Fail { .. } | Terminator::Unreachable => vec![],
        }
    }

    /// Walk a cleanup without allocating or expanding its shared suffix.
    pub fn cleanup_actions(&self, id: CleanupId) -> impl Iterator<Item = &CleanupAction> {
        let mut next = self.cleanups[id.0 as usize].head;
        std::iter::from_fn(move || {
            let node = self.cleanup_nodes.get(next?.0 as usize)?;
            next = node.parent;
            Some(&node.action)
        })
    }
}
pub(super) struct ReadBlock<'a> {
    pub(super) instructions: Rows<'a, Instruction>,
    pub(super) terminator: Terminator,
}
impl<'a> ReadBlock<'a> {
    fn new(view: TirBlockView<'a>) -> Self {
        Self {
            instructions: match view {
                TirBlockView::Live(value) => Rows::Live(&value.instructions),
                TirBlockView::Archived(_) => Rows::indexed(view.instruction_count(), move |n| {
                    view.instruction(n)
                        .expect("bound checked instruction")
                        .into_owned()
                }),
            },
            terminator: view
                .terminator()
                .expect("bound checked terminator")
                .into_owned(),
        }
    }
}
pub(super) struct ReadMatch<'a> {
    pub(super) scrutinee: PlaceId,
    pub(super) patterns: Rows<'a, MatchPattern>,
    pub(super) arms: Rows<'a, MatchArm>,
}
impl<'a> ReadMatch<'a> {
    fn new(view: TirMatchView<'a>) -> Self {
        Self {
            scrutinee: view.scrutinee().expect("checked match scrutinee"),
            patterns: match view {
                TirMatchView::Live(value) => Rows::Live(&value.patterns),
                TirMatchView::Archived(_) => Rows::indexed(view.pattern_count(), move |n| {
                    view.pattern(n).expect("bound checked pattern").into_owned()
                }),
            },
            arms: match view {
                TirMatchView::Live(value) => Rows::Live(&value.arms),
                TirMatchView::Archived(_) => Rows::indexed(view.arm_count(), move |n| {
                    view.arm(n).expect("bound checked arm").into_owned()
                }),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::support::{self, Fixture};
    use hirsel_artifact_store::{Contents, Memory};
    use hirsel_product_codec::{CheckedTir, TypeRecords};
    use std::sync::Arc;

    #[test]
    fn archived_semantic_v_derives_the_same_violations_and_reads_as_live_v() {
        let mut fixtures = vec![
            Fixture::unit(),
            Fixture::loan(),
            Fixture::call(),
            Fixture::string(),
        ];
        let mut wrong_type = Fixture::unit();
        wrong_type.definition.bodies[0].rvalues[0] = Rvalue::Constant(Constant::Integer(9));
        assert!(
            !crate::validate(
                &wrong_type.definition,
                support::interner(),
                &wrong_type.signatures,
                &mut vec![]
            )
            .violations
            .is_empty()
        );
        fixtures.push(wrong_type);
        let mut missing_member = Fixture::unit();
        missing_member.definition.types = Arc::new([]);
        fixtures.push(missing_member);
        let mut accepted = 0;
        for fixture in fixtures {
            let mut live_reads = vec![];
            let live = crate::validate(
                &fixture.definition,
                support::interner(),
                &fixture.signatures,
                &mut live_reads,
            );
            accepted += usize::from(live.violations.is_empty());
            let store = Arc::new(Memory::default());
            let records = TypeRecords::new(support::session(), store.clone());
            let bytes = records
                .encode_tir(&fixture.definition, |_| Some((support::path("main"), 0)))
                .unwrap();
            let reference = store.put_content(CheckedTir::DOMAIN, &bytes).unwrap();
            let archive = CheckedTir::open(
                store.get_content(reference).unwrap(),
                reference,
                records,
                |_, _| Some(hirsel_span::Anchor::default()),
            )
            .unwrap();
            let mut reads = vec![];
            let actual = crate::validate_rows(
                TirView::Archived(&archive),
                support::interner(),
                &fixture.signatures,
                &mut reads,
            )
            .unwrap();
            assert_eq!(actual.violations, live.violations);
            assert_eq!(reads, live_reads);
        }
        assert!(accepted > 0);
    }
}
