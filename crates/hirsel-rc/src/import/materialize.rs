//! Decode flat sequences with ordinary list construction. The emitted zero-based
//! Index loop is strength-reduced by flat_walk, including variable-size elements.
use super::*;
use crate::PassError;
use hirsel_types::{BuiltinType, TypeKind};

impl Importer<'_, '_> {
    fn scalar_constant(&mut self, ty: LirTypeId, n: u128) -> ValueId {
        let constant = self.constant(ty, ConstantValue::Integer(n));
        let result = self.value(ty, Holding::Plain);
        self.push(Inst::Const { result, constant });
        result
    }

    pub(super) fn materialize_sequence(
        &mut self,
        source: AddressId,
        encoding: LirTypeId,
        ordinary: LirTypeId,
        source_type: TypeId,
    ) -> R<ValueId> {
        let TypeKind::Builtin {
            kind: BuiltinType::List,
            arguments,
        } = self
            .types
            .get(source_type)
            .expect("source type")
            .kind
            .clone()
        else {
            return unsupported("materializing this ordinary sequence type from flat bytes");
        };
        let [GenericArgument::Type(element_type)] = arguments.as_ref() else {
            return malformed("flat list actuals");
        };
        let element_type = *element_type;
        let element_layout = self.layout(element_type)?;
        let element = self.intern(element_layout);
        let encoded_layout = flat_sequence(self.layouts, self.layout_of(encoding))
            .ok_or_else(|| PassError::Malformed("flat list encoding".into()))?;
        let encoded = self.intern(encoded_layout);
        let LayoutKind::Block { payload } = self.record(ordinary).kind else {
            return malformed("ordinary list block");
        };
        let AddressBase::Slot(source_slot) = self.out.addresses[source.index()].base else {
            return malformed("flat materialization source root");
        };
        let scope = self.out.slots[source_slot.index()].scope;
        let u64_type = self.types.intern(hirsel_types::Type {
            kind: TypeKind::Primitive(hirsel_types::Primitive::U64),
            ..hirsel_types::Type::unit()
        });
        let u64_layout = self.layout(u64_type)?;
        let u64_ty = self.intern(u64_layout);
        let bool_type = self.types.intern(hirsel_types::Type::boolean());
        let bool_layout = self.layout(bool_type)?;
        let bool_ty = self.intern(bool_layout);
        let count = self.value(u64_ty, Holding::Plain);
        self.push(Inst::FlatLength {
            result: count,
            address: source,
        });
        let empty = self.construct(
            ConstructorKind::Sequence,
            payload,
            vec![],
            DestinationClass::Heap,
            ordinary,
        )?;
        let slot = self.new_slot(ordinary, SlotKind::Temporary, scope);
        self.push(Inst::StorageLive { slot });
        self.state.live(slot);
        let list = self.whole(slot);
        let address = self.address(&list)?;
        self.store(&list, address, empty)?;
        // The partially filled list is newer than every source operand. Each
        // push publishes its length only after the element has been installed.
        self.failure.before.insert(
            0,
            Action::Release {
                slot,
                anchor: (INSTRUCTION_DROP, self.statement_action()),
            },
        );
        self.materialize_list_call("list.reserve", &arguments, &list, address, count)?;

        let index_slot = self.new_slot(u64_ty, SlotKind::LoopCarried(slot.0), scope);
        self.push(Inst::StorageLive { slot: index_slot });
        self.state.live(index_slot);
        let index_place = self.whole(index_slot);
        let index_at = self.address(&index_place)?;
        let zero = self.scalar_constant(u64_ty, 0);
        self.store(&index_place, index_at, zero)?;
        let header = self.block(BlockKind::Body);
        let body = self.block(BlockKind::Body);
        let done = self.block(BlockKind::Body);
        let index_version = self.value(u64_ty, Holding::Plain);
        let list_version = self.value(ordinary, Holding::Plain);
        self.out.blocks[header.index()].phis =
            vec![(index_slot, index_version), (slot, list_version)];
        self.goto(header);
        self.versions[index_slot.index()] = Some(index_version);
        self.versions[slot.index()] = Some(list_version);
        let index = self.value(u64_ty, Holding::Plain);
        self.push(Inst::Read {
            result: index,
            address: index_at,
            reaching: index_version,
            mode: ReadMode::Take,
        });
        self.state.take(index_slot, &[]);
        let condition = self.value(bool_ty, Holding::Plain);
        let failure = self.fail(None)?;
        self.push(Inst::Binary {
            result: condition,
            op: BinaryOp::Lt,
            left: index,
            right: count,
            failure,
        });
        let yes = self.edge(body, vec![]);
        let no = self.edge(done, vec![]);
        self.terminate(Terminator::Branch { condition, yes, no });
        self.current = body;
        // Keep the source's slot-rooted path, so flat_walk can prove that this
        // sequence is unchanged and carry one offset around the decode loop.
        let info = self.out.addresses[source.index()].clone();
        let mut path = info
            .projection
            .get(&self.out.pools.projections)
            .unwrap()
            .to_vec();
        path.push(ProjectionStep::Index(index));
        let element_at = AddressId(self.out.addresses.len() as u32);
        self.out.addresses.push(AddressInfo {
            source_place: None,
            unchecked: Vec::new(),
            base: info.base,
            projection: PoolRange::push(&mut self.out.pools.projections, path),
            ty: encoded,
        });
        let failure = self.fail(None)?;
        self.push(Inst::Address {
            address: element_at,
            failure,
        });
        let value = if encoded == element && self.bits(element) {
            let AddressBase::Slot(source_slot) = info.base else {
                return malformed("flat materialization source root");
            };
            let reaching = self.reaching(source_slot)?;
            let result = self.value(element, Holding::Plain);
            self.push(Inst::Read {
                result,
                address: element_at,
                reaching,
                mode: ReadMode::Bits,
            });
            result
        } else if let LayoutKind::Block { payload } = self.record(element).kind
            && matches!(
                self.layouts.record(payload).kind,
                LayoutKind::Sequence { .. }
            )
        {
            self.materialize_sequence(element_at, encoded, element, element_type)?
        } else {
            let result = self.fresh(element);
            let failure = self.fail(None)?;
            self.push(Inst::FlatCopy {
                result,
                address: element_at,
                layout: encoded_layout,
                failure,
            });
            if self.holds(result) {
                self.failure.pending.push(result);
            }
            result
        };
        self.materialize_list_call("list.push", &arguments, &list, address, value)?;
        let one = self.scalar_constant(u64_ty, 1);
        let next = self.value(u64_ty, Holding::Plain);
        let failure = self.fail(None)?;
        self.push(Inst::Binary {
            result: next,
            op: BinaryOp::Add,
            left: index,
            right: one,
            failure,
        });
        self.store(&index_place, index_at, next)?;
        let feedback = self.edge(header, vec![]);
        self.terminate(Terminator::Jump(feedback));
        self.current = done;
        self.versions[slot.index()] = Some(list_version);
        self.versions[index_slot.index()] = Some(index_version);
        self.failure.before.remove(0);
        let result = self.operand_at(UseKind::Take, &list, address, None)?;
        for s in [slot, index_slot] {
            self.push(Inst::StorageDead {
                slot: s,
                if_live: false,
            });
            self.state.dead(s);
            self.versions[s.index()] = None;
        }
        Ok(result)
    }

    fn materialize_list_call(
        &mut self,
        name: &str,
        actuals: &[GenericArgument],
        list: &PlaceRef,
        address: AddressId,
        value: ValueId,
    ) -> R<()> {
        let path = hirsel_syntax::DefPath::core(name);
        let body = crate::library::body_index(&path).expect("list construction body");
        let request = crate::library::Request::operation(
            self.program,
            body,
            &path,
            actuals.to_vec(),
        )?;
        let direct = self.require(request)?;
        let symbol = self.symbol(
            direct.symbol.kind,
            &direct.symbol.definition,
            direct.symbol.instance,
        );
        let version = self.value(list.ty, Holding::Plain);
        self.versions[list.slot.index()] = Some(version);
        Inline::direct_call(
            self,
            symbol,
            direct.abi,
            vec![
                CallArgument {
                    parameter: 0,
                    passing: Passing::Mut,
                    value: ArgumentValue::Address { address, version },
                },
                CallArgument {
                    parameter: 1,
                    passing: if name == "list.push" {
                        Passing::Take
                    } else {
                        Passing::Borrow
                    },
                    value: ArgumentValue::Value(value),
                },
            ],
        )?;
        Ok(())
    }
}
