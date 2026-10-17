//! Scalar legalization: operations of a Ready unit that the
//! emitters and the heap executor have no single instruction for become
//! explicit checked scalar paths.
//!
//! - **SIMD.** `simd.splat` and `simd.add` calls become lane code with the
//!   semantics independent of hardware width (`Simd`). `add` reads
//!   lane i of each operand, adds the lanes with a checked `Binary`, in
//!   ascending lane order, so the first overflowing lane fails SEM-1 at the
//!   call's own site and no later lane runs. The result is a `Construct` of
//!   the Simd layout's lanes (inline, no allocation).
//! - **Integer division and shift** are already checked `Binary` operations:
//!   `/0`, signed `MIN / -1`, `% 0` and a count outside `0..width` take the
//!   failure edge, and both emitters guard before the instruction, so no
//!   width reaches poison. Legalization leaves them as they are; the legalize
//!   tests execute every width's edges.
//! - **Floats** are exact IEEE `Binary` operations with no reassociation or
//!   contraction; nothing to rewrite.
//! - **i128** values are scalar words of their own width in both emitters;
//!   arithmetic and comparisons stay `Binary`.
//! - **Scalar intrinsics** (LIR schema revision 6). A call of a core library scalar
//!   library operation becomes its instruction, chosen from the call's argument and result kinds:
//!   `int.widen` and `int.truncate` a `Convert`; `int.narrow` a `TryNarrow`
//!   `Convert` and a `Check` of its flag failing SEM-1 at the call's site;
//!   `int.try_narrow` a `TryNarrow` `Convert` whose flag selects `Some` or
//!   `None`; `int.wrapping_add` and `int.saturating_add` an `IntArith`, and
//!   `int.checked_add` a `Checked` one whose overflow flag selects `None`;
//!   `int.div_floor`, `int.mod` and `int.div_exact` a `Binary` failing SEM-1;
//!   `float.min`, `max`, `min_number`, `max_number` and `total_order` a
//!   `Binary` that cannot fail; `float.mul_add` a `MulAdd`; and
//!   `float.to_i32` a `FloatToI32` `Convert` whose status switches to
//!   `Ok(v)`, `Error(NotFinite)` or `Error(OutOfRange)`. The `Option` and
//!   `Result` values are inline variant constructions joined in a block
//!   parameter. Ordinary std reductions stay source calls.
//!
//! Optional rewrites (end of this file) are separate from legalization and
//! off unless a caller runs them: discovery is read-only, a plan commits only
//! where the proof-domain ledger `native-tests/rewrites/ledger.tsv` records
//! its rule and exact domain as proved, and the Ready validator checks the
//! result again.
use crate::PassError;
use crate::import::repr_of;
use crate::insert::{Editor, type_of};
use crate::placement::TASK_HEAP;
use hirsel_lir::ir::{
    AddressBase, AddressInfo, ArgumentValue, ArithMode, BinaryOp, BlockId, BlockKind, CallSite,
    Callee, ConstantData, ConstantId, ConstantValue, ConstructId, ConstructSite, ConstructorKind,
    ConvertMode, DestinationClass, EdgeId, FailureEdge, FailureId, FieldValue, Holding, Inst,
    LirUnit, Placement, PoolRange, ProjectionStep, ReadMode, ScopeId, SlotId, SlotInfo, SlotKind,
    SwitchArm, Symbol, SymbolId, SymbolKind, Terminator, ValueId, ValueRepr,
};
use hirsel_lir::layout_types::{LayoutId, LayoutKind, LayoutTable, ScalarKind};
use hirsel_lir::stage::Ready;
use hirsel_syntax::{DefPath, ItemPath};

/// The SIMD operations this pass expands.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SimdOp {
    Splat,
    Add,
}

/// The scalar library operations (docs/spec/core-library.md) and the instruction each becomes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ScalarOp {
    /// `widen`, `truncate`, `try_narrow` (into an `Option`) and `to_i32`
    /// (into a `Result`).
    Convert(ConvertMode),
    /// `narrow`: `try_narrow`'s conversion and a SEM-1 check of its flag.
    Narrow,
    Arith(ArithMode),
    /// `div_floor`, `mod`, `div_exact`, the float min/max family and
    /// `total_order`.
    Binary(BinaryOp),
    MulAdd,
}

/// What one call site legalizes into.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Site {
    Simd(SimdOp),
    Scalar(ScalarOp),
}

/// Legalize every operation of `unit` that has no direct native form.
pub fn legalize(unit: LirUnit<Ready>, table: &LayoutTable) -> Result<LirUnit<Ready>, PassError> {
    let mut ed = Editor::new(unit);
    let sites: Vec<(BlockId, Site)> = ed
        .unit
        .terminators
        .iter()
        .enumerate()
        .filter_map(|(b, t)| {
            let Terminator::Call { call, .. } = *t else {
                return None;
            };
            let Callee::Operation(op) = ed.unit.tables.calls[call.index()].callee else {
                return None;
            };
            let definition = &ed.unit.tables.operations[op.index()].definition;
            let site = simd_op(definition)
                .map(Site::Simd)
                .or_else(|| scalar_op(definition).map(Site::Scalar))?;
            Some((BlockId(b as u32), site))
        })
        .collect();
    for (b, site) in sites {
        let mut lowering = Lowering { ed: &mut ed, table };
        match site {
            Site::Simd(op) => lowering.simd(b, op)?,
            Site::Scalar(op) => lowering.scalar(b, op)?,
        }
    }
    Ok(ed.finish())
}

fn scalar_op(definition: &DefPath) -> Option<ScalarOp> {
    use ScalarOp as S;
    let path = &definition.0;
    if !path.package.name_is("core") {
        return None;
    }
    Some(match (path.modules.as_slice(), path.name.as_str()) {
        ([m], name) if m == "int" => match name {
            "widen" => S::Convert(ConvertMode::Widen),
            "truncate" => S::Convert(ConvertMode::Truncate),
            "try_narrow" => S::Convert(ConvertMode::TryNarrow),
            "narrow" => S::Narrow,
            "wrapping_add" => S::Arith(ArithMode::Wrapping),
            "saturating_add" => S::Arith(ArithMode::Saturating),
            "checked_add" => S::Arith(ArithMode::Checked),
            "div_floor" => S::Binary(BinaryOp::DivFloor),
            "mod" => S::Binary(BinaryOp::Mod),
            "div_exact" => S::Binary(BinaryOp::DivExact),
            _ => return None,
        },
        ([m], "bits" | "from_bits") if m == "f32" || m == "f64" => {
            S::Convert(ConvertMode::Reinterpret)
        }
        ([m], name) if m == "float" => match name {
            "min" => S::Binary(BinaryOp::Min),
            "max" => S::Binary(BinaryOp::Max),
            "min_number" => S::Binary(BinaryOp::MinNumber),
            "max_number" => S::Binary(BinaryOp::MaxNumber),
            "total_order" => S::Binary(BinaryOp::TotalOrder),
            "mul_add" => S::MulAdd,
            "to_i32" => S::Convert(ConvertMode::FloatToI32),
            "from_int" => S::Convert(ConvertMode::IntToFloat),
            _ => return None,
        },
        _ => return None,
    })
}

fn simd_op(definition: &hirsel_syntax::DefPath) -> Option<SimdOp> {
    let path = &definition.0;
    if !path.package.name_is("core") || path.modules.as_slice() != ["simd"] {
        return None;
    }
    match path.name.as_str() {
        "splat" => Some(SimdOp::Splat),
        "add" => Some(SimdOp::Add),
        _ => None,
    }
}

struct Lowering<'a> {
    ed: &'a mut Editor<Ready>,
    table: &'a LayoutTable,
}

impl Lowering<'_> {
    fn layout_kind(&self, id: LayoutId) -> Result<LayoutKind, PassError> {
        self.table
            .layout(id)
            .map(|l| l.kind.clone())
            .ok_or_else(|| PassError::Malformed(format!("no layout {id:?}")))
    }

    fn value_layout(&self, v: ValueId) -> LayoutId {
        let ty = self.ed.unit.values[v.index()].ty;
        self.ed.unit.types[ty.index()].layout
    }

    /// Replace the call ending `b` by its lane code and a jump to its normal
    /// successor.
    fn simd(&mut self, b: BlockId, op: SimdOp) -> Result<(), PassError> {
        let Terminator::Call {
            call,
            normal: Some(normal),
            failure,
        } = self.ed.unit.terminators[b.index()]
        else {
            unreachable!("a call site")
        };
        let site = self.ed.unit.tables.calls[call.index()].clone();
        let Some(result) = site.result else {
            return Err(PassError::Malformed(format!("{op:?} without a result")));
        };
        let simd = self.value_layout(result);
        let LayoutKind::Simd { element, lanes } = self.layout_kind(simd)? else {
            return Err(PassError::Malformed(format!(
                "{op:?} into a {simd:?} that is not Simd"
            )));
        };
        let LayoutKind::Scalar(kind) = self.layout_kind(element)? else {
            return Err(PassError::Malformed(
                "a Simd lane that is not scalar".into(),
            ));
        };
        let mut arguments: Vec<_> = site
            .arguments
            .get(&self.ed.unit.pools.call_arguments)
            .unwrap_or(&[])
            .to_vec();
        arguments.sort_by_key(|a| a.parameter);
        let operands = arguments
            .iter()
            .map(|a| match a.value {
                ArgumentValue::Value(v) => Ok(v),
                ArgumentValue::Address { .. } => Err(PassError::Malformed(format!(
                    "{op:?} with an address argument"
                ))),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let lane_values = match (op, operands.as_slice()) {
            (SimdOp::Splat, &[x]) => vec![x; lanes as usize],
            (SimdOp::Add, &[x, y]) => {
                if !integer(kind) {
                    return Err(PassError::Malformed("simd.add of non-integer lanes".into()));
                }
                self.add(b, failure, simd, element, kind, lanes, x, y)?
            }
            _ => {
                return Err(PassError::Malformed(format!(
                    "{op:?} with {} arguments",
                    operands.len()
                )));
            }
        };
        let fields = PoolRange::push(
            &mut self.ed.unit.pools.fields,
            lane_values
                .iter()
                .enumerate()
                .map(|(i, &value)| FieldValue {
                    field: i as u32,
                    value,
                }),
        );
        let construct = ConstructId(self.ed.unit.tables.constructs.len() as u32);
        self.ed.unit.tables.constructs.push(ConstructSite {
            arena_no_drop: false,
            constructor: ConstructorKind::Sequence,
            layout: simd,
            fields,
            destination: DestinationClass::Heap,
            placement: Some(Placement::Fixed(TASK_HEAP)),
            anchor: site.anchor,
        });
        self.ed.code[b.index()].push(Inst::Construct {
            result,
            construct,
            failure,
        });
        self.ed.unit.terminators[b.index()] = Terminator::Jump(normal);
        Ok(())
    }

    /// Replace the call ending `b` by its scalar instruction (with the
    /// variant selection an `Option` or `Result` result needs) and a jump
    /// to its normal successor.
    fn scalar(&mut self, b: BlockId, op: ScalarOp) -> Result<(), PassError> {
        let Terminator::Call {
            call,
            normal: Some(normal),
            failure,
        } = self.ed.unit.terminators[b.index()]
        else {
            unreachable!("a call site")
        };
        let site = self.ed.unit.tables.calls[call.index()].clone();
        let Some(result) = site.result else {
            return Err(PassError::Malformed(format!("{op:?} without a result")));
        };
        let operands = self.operands(&site, op)?;
        let code = |s: &mut Self, inst: Inst| s.ed.code[b.index()].push(inst);
        let done = Terminator::Jump(normal);
        match (op, operands.as_slice()) {
            (
                ScalarOp::Convert(
                    mode @ (ConvertMode::Widen
                    | ConvertMode::Truncate
                    | ConvertMode::IntToFloat
                    | ConvertMode::Reinterpret),
                ),
                &[x],
            ) => {
                code(
                    self,
                    Inst::Convert {
                        result,
                        flag: None,
                        operand: x,
                        mode,
                    },
                );
                self.ed.unit.terminators[b.index()] = done;
            }
            (ScalarOp::Narrow, &[x]) => {
                let fits = self.bool_value()?;
                code(
                    self,
                    Inst::Convert {
                        result,
                        flag: Some(fits),
                        operand: x,
                        mode: ConvertMode::TryNarrow,
                    },
                );
                let failure = self.like(failure, Some("SEM-1"));
                code(
                    self,
                    Inst::Check {
                        condition: fits,
                        failure,
                    },
                );
                self.ed.unit.terminators[b.index()] = done;
            }
            (ScalarOp::Convert(ConvertMode::TryNarrow), &[x]) => {
                let value = self.payload_value(result, OPTION_SOME)?;
                let fits = self.bool_value()?;
                code(
                    self,
                    Inst::Convert {
                        result: value,
                        flag: Some(fits),
                        operand: x,
                        mode: ConvertMode::TryNarrow,
                    },
                );
                self.option(b, &site, fits, true, value, normal, failure)?;
            }
            (ScalarOp::Arith(ArithMode::Checked), &[x, y]) => {
                let value = self.payload_value(result, OPTION_SOME)?;
                let overflow = self.bool_value()?;
                code(
                    self,
                    Inst::IntArith {
                        result: value,
                        flag: Some(overflow),
                        op: BinaryOp::Add,
                        mode: ArithMode::Checked,
                        left: x,
                        right: y,
                    },
                );
                self.option(b, &site, overflow, false, value, normal, failure)?;
            }
            (ScalarOp::Arith(mode), &[x, y]) => {
                code(
                    self,
                    Inst::IntArith {
                        result,
                        flag: None,
                        op: BinaryOp::Add,
                        mode,
                        left: x,
                        right: y,
                    },
                );
                self.ed.unit.terminators[b.index()] = done;
            }
            (ScalarOp::Binary(op), &[x, y]) => {
                // The integer divisions fail SEM-1; the float operations
                // cannot fail, so their edge is never taken.
                let rule = matches!(op, BinaryOp::DivFloor | BinaryOp::Mod | BinaryOp::DivExact)
                    .then_some("SEM-1");
                let failure = self.like(failure, rule);
                code(
                    self,
                    Inst::Binary {
                        result,
                        op,
                        left: x,
                        right: y,
                        failure,
                    },
                );
                self.ed.unit.terminators[b.index()] = done;
            }
            (ScalarOp::MulAdd, &[x, y, z]) => {
                code(
                    self,
                    Inst::MulAdd {
                        result,
                        left: x,
                        right: y,
                        addend: z,
                    },
                );
                self.ed.unit.terminators[b.index()] = done;
            }
            (ScalarOp::Convert(ConvertMode::FloatToI32), &[x]) => {
                self.to_i32(b, &site, x, normal, failure)?;
            }
            _ => {
                return Err(PassError::Malformed(format!(
                    "{op:?} with {} arguments",
                    operands.len()
                )));
            }
        }
        Ok(())
    }

    /// The call's arguments in declaration order, each a scalar value.
    fn operands(&self, site: &CallSite, op: ScalarOp) -> Result<Vec<ValueId>, PassError> {
        let mut arguments: Vec<_> = site
            .arguments
            .get(&self.ed.unit.pools.call_arguments)
            .unwrap_or(&[])
            .to_vec();
        arguments.sort_by_key(|a| a.parameter);
        arguments
            .iter()
            .map(|a| match a.value {
                ArgumentValue::Value(v)
                    if matches!(self.value_type(v).repr, ValueRepr::Scalar(_)) =>
                {
                    Ok(v)
                }
                _ => Err(PassError::Malformed(format!(
                    "{op:?} with an argument that is not a scalar value"
                ))),
            })
            .collect()
    }

    fn value_type(&self, v: ValueId) -> hirsel_lir::ir::LirType {
        self.ed.unit.types[self.ed.unit.values[v.index()].ty.index()].clone()
    }

    /// A fresh Plain Bool, the flag of a range or overflow test. Every
    /// sealed layout table holds `Scalar(Bool)`.
    fn bool_value(&mut self) -> Result<ValueId, PassError> {
        let layout = self.scalar_layout(ScalarKind::Bool)?;
        let ty = type_of(
            &mut self.ed.unit,
            ValueRepr::Scalar(ScalarKind::Bool),
            layout,
        );
        Ok(self.ed.value(ty, Holding::Plain))
    }

    fn scalar_layout(&self, kind: ScalarKind) -> Result<LayoutId, PassError> {
        self.table
            .layouts
            .iter()
            .position(|l| l.kind == LayoutKind::Scalar(kind))
            .map(|i| LayoutId(i as u32))
            .ok_or_else(|| PassError::Malformed(format!("no {kind:?} layout in the table")))
    }

    /// The layout of field 0 of variant `ordinal` of `variant`'s layout.
    fn payload_layout(&self, variant: LayoutId, ordinal: u32) -> Result<LayoutId, PassError> {
        self.table
            .layout(variant)
            .and_then(|l| l.tag.as_ref())
            .and_then(|t| t.variants.iter().find(|v| v.ordinal == ordinal))
            .and_then(|v| v.fields.iter().find(|f| f.ordinal == 0))
            .map(|f| f.layout)
            .ok_or_else(|| {
                PassError::Malformed(format!("{variant:?} has no field 0 in variant {ordinal}"))
            })
    }

    /// A fresh Plain value of the payload of `result`'s variant `ordinal`.
    fn payload_value(&mut self, result: ValueId, ordinal: u32) -> Result<ValueId, PassError> {
        let layout = self.payload_layout(self.value_type(result).layout, ordinal)?;
        let kind = self.layout_kind(layout)?;
        let ty = type_of(&mut self.ed.unit, repr_of(&kind), layout);
        Ok(self.ed.value(ty, Holding::Plain))
    }

    /// A `core` type symbol, for a variant construction.
    fn core_type(&mut self, site: &CallSite, name: &str) -> SymbolId {
        let Callee::Operation(op) = site.callee else {
            unreachable!("an operation call")
        };
        let package = self.ed.unit.tables.operations[op.index()]
            .definition
            .0
            .package
            .clone();
        let symbol = Symbol {
            kind: SymbolKind::Type,
            definition: DefPath(ItemPath {
                package,
                modules: Vec::new(),
                name: name.into(),
            }),
            instance: None,
        };
        let symbols = &mut self.ed.unit.tables.symbols;
        let index = symbols
            .iter()
            .position(|s| *s == symbol)
            .unwrap_or_else(|| {
                symbols.push(symbol);
                symbols.len() - 1
            });
        SymbolId(index as u32)
    }

    /// `definition`'s variant `variant` of `fields` into `result` at the end
    /// of `b`: an inline construction, failing as the call would.
    #[allow(clippy::too_many_arguments)]
    fn variant(
        &mut self,
        b: BlockId,
        site: &CallSite,
        definition: &str,
        variant: u32,
        result: ValueId,
        fields: &[ValueId],
        failure: FailureId,
    ) {
        let definition = self.core_type(site, definition);
        let fields = PoolRange::push(
            &mut self.ed.unit.pools.fields,
            fields.iter().enumerate().map(|(i, &value)| FieldValue {
                field: i as u32,
                value,
            }),
        );
        let construct = ConstructId(self.ed.unit.tables.constructs.len() as u32);
        self.ed.unit.tables.constructs.push(ConstructSite {
            arena_no_drop: false,
            constructor: ConstructorKind::Variant {
                definition,
                variant,
            },
            layout: self.value_type(result).layout,
            fields,
            destination: DestinationClass::Heap,
            placement: Some(Placement::Fixed(TASK_HEAP)),
            anchor: site.anchor,
        });
        let failure = self.like(failure, None);
        self.ed.code[b.index()].push(Inst::Construct {
            result,
            construct,
            failure,
        });
    }

    /// A fresh value of the call result's type and holding, defined on one
    /// arm and joined into the result.
    fn arm_value(&mut self, result: ValueId) -> ValueId {
        let info = self.ed.unit.values[result.index()].clone();
        self.ed.value(info.ty, info.holding)
    }

    /// End `b` in arms constructing the variants `arms` lists (by the
    /// switch value each takes, `None` for the otherwise arm) and a join
    /// whose parameter is the call's result, then the call's normal edge.
    #[allow(clippy::too_many_arguments)]
    fn join(
        &mut self,
        b: BlockId,
        site: &CallSite,
        result: ValueId,
        normal: EdgeId,
        failure: FailureId,
        arms: &[(&str, u32, Vec<ValueId>)],
    ) -> Vec<EdgeId> {
        let anchor = self.ed.unit.blocks[b.index()].anchor;
        let join = self.ed.block(BlockKind::Body, &[result], anchor);
        self.ed.unit.terminators[join.index()] = Terminator::Jump(normal);
        let mut entries = Vec::new();
        for (definition, variant, fields) in arms {
            let arm = self.ed.block(BlockKind::Body, &[], anchor);
            let value = self.arm_value(result);
            self.variant(arm, site, definition, *variant, value, fields, failure);
            let to_join = self.ed.edge(join, &[value]);
            self.ed.unit.terminators[arm.index()] = Terminator::Jump(to_join);
            entries.push(self.ed.edge(arm, &[]));
        }
        entries
    }

    /// `Some(value)` when `flag == some_when`, else `None`, as the call's
    /// result.
    #[allow(clippy::too_many_arguments)]
    fn option(
        &mut self,
        b: BlockId,
        site: &CallSite,
        flag: ValueId,
        some_when: bool,
        value: ValueId,
        normal: EdgeId,
        failure: FailureId,
    ) -> Result<(), PassError> {
        let result = site.result.expect("a result");
        let edges = self.join(
            b,
            site,
            result,
            normal,
            failure,
            &[
                ("Option", OPTION_SOME, vec![value]),
                ("Option", OPTION_NONE, vec![]),
            ],
        );
        let (yes, no) = if some_when {
            (edges[0], edges[1])
        } else {
            (edges[1], edges[0])
        };
        self.ed.unit.terminators[b.index()] = Terminator::Branch {
            condition: flag,
            yes,
            no,
        };
        Ok(())
    }

    /// `float.to_i32`: the conversion and a switch on its status into
    /// `Ok(v)`, `Error(NotFinite)` or `Error(OutOfRange)`.
    fn to_i32(
        &mut self,
        b: BlockId,
        site: &CallSite,
        x: ValueId,
        normal: EdgeId,
        failure: FailureId,
    ) -> Result<(), PassError> {
        let result = site.result.expect("a result");
        let value = self.payload_value(result, RESULT_OK)?;
        // The status has the payload's own type: i32.
        let status_ty = self.ed.unit.values[value.index()].ty;
        let status = self.ed.value(status_ty, Holding::Plain);
        self.ed.code[b.index()].push(Inst::Convert {
            result: value,
            flag: Some(status),
            operand: x,
            mode: ConvertMode::FloatToI32,
        });
        // FloatIntError = NotFinite | OutOfRange, as Error's payload.
        let error_layout = self.payload_layout(self.value_type(result).layout, RESULT_ERROR)?;
        let error_repr = repr_of(&self.layout_kind(error_layout)?);
        let error_ty = type_of(&mut self.ed.unit, error_repr, error_layout);
        let mut errors = Vec::new();
        for (status, variant) in [(1, NOT_FINITE), (2, OUT_OF_RANGE)] {
            let anchor = self.ed.unit.blocks[b.index()].anchor;
            let arm = self.ed.block(BlockKind::Body, &[], anchor);
            let error = self.ed.value(error_ty, Holding::Plain);
            self.variant(arm, site, "FloatIntError", variant, error, &[], failure);
            errors.push((status, arm, error));
        }
        let edges = self.join(
            b,
            site,
            result,
            normal,
            failure,
            &[
                ("Result", RESULT_OK, vec![value]),
                ("Result", RESULT_ERROR, vec![errors[0].2]),
                ("Result", RESULT_ERROR, vec![errors[1].2]),
            ],
        );
        // Each error arm builds its payload, then enters its Result arm.
        for (i, &(_, arm, _)) in errors.iter().enumerate() {
            self.ed.unit.terminators[arm.index()] = Terminator::Jump(edges[i + 1]);
        }
        let arms: Vec<_> = errors
            .iter()
            .map(|&(status, arm, _)| SwitchArm {
                value: status,
                edge: self.ed.edge(arm, &[]),
            })
            .collect();
        let arms = PoolRange::push(&mut self.ed.unit.pools.switch_arms, arms);
        self.ed.unit.terminators[b.index()] = Terminator::Switch {
            value: status,
            arms,
            otherwise: edges[0],
        };
        Ok(())
    }

    /// Lane sums of `x` and `y`, in ascending lane order, each a checked add
    /// failing SEM-1 at the call's site.
    #[allow(clippy::too_many_arguments)]
    fn add(
        &mut self,
        b: BlockId,
        call_failure: FailureId,
        simd: LayoutId,
        element: LayoutId,
        kind: ScalarKind,
        lanes: u32,
        x: ValueId,
        y: ValueId,
    ) -> Result<Vec<ValueId>, PassError> {
        let simd_ty = type_of(&mut self.ed.unit, ValueRepr::Aggregate, simd);
        let lane_ty = type_of(
            &mut self.ed.unit,
            repr_of(&LayoutKind::Scalar(kind)),
            element,
        );
        let index_ty = self.index_type(element, kind, lanes)?;
        // Each operand in a slot of its own, read lane by lane.
        let mut slots = Vec::new();
        for v in [x, y] {
            let slot = SlotId(self.ed.unit.slots.len() as u32);
            self.ed.unit.slots.push(SlotInfo {
                ty: simd_ty,
                kind: SlotKind::Temporary,
                scope: ScopeId(0),
            });
            let whole = self.address(AddressBase::Slot(slot), &[], simd_ty);
            let version = self.ed.value(simd_ty, Holding::Plain);
            let code = &mut self.ed.code[b.index()];
            code.push(Inst::StorageLive { slot });
            let failure = self.like(call_failure, None);
            let code = &mut self.ed.code[b.index()];
            code.push(Inst::Address {
                address: whole,
                failure,
            });
            code.push(Inst::Store {
                address: whole,
                value: v,
                version,
            });
            slots.push((slot, version));
        }
        let mut sums = Vec::new();
        for lane in 0..lanes {
            let index = self.constant(b, index_ty, u128::from(lane));
            let mut reads = Vec::new();
            for &(slot, version) in &slots {
                let at = self.address(
                    AddressBase::Slot(slot),
                    &[ProjectionStep::Index(index)],
                    lane_ty,
                );
                let read = self.ed.value(lane_ty, Holding::Plain);
                let failure = self.like(call_failure, None);
                let code = &mut self.ed.code[b.index()];
                code.push(Inst::Address {
                    address: at,
                    failure,
                });
                code.push(Inst::Read {
                    result: read,
                    address: at,
                    reaching: version,
                    mode: ReadMode::Bits,
                });
                reads.push(read);
            }
            let sum = self.ed.value(lane_ty, Holding::Plain);
            let failure = self.like(call_failure, Some("SEM-1"));
            self.ed.code[b.index()].push(Inst::Binary {
                result: sum,
                op: BinaryOp::Add,
                left: reads[0],
                right: reads[1],
                failure,
            });
            sums.push(sum);
        }
        for &(slot, _) in &slots {
            self.ed.code[b.index()].push(Inst::StorageDead {
                slot,
                if_live: false,
            });
        }
        Ok(sums)
    }

    /// A lane index type: u64 when the program has it, else the lane type
    /// itself when every lane number fits it.
    fn index_type(
        &mut self,
        element: LayoutId,
        kind: ScalarKind,
        lanes: u32,
    ) -> Result<hirsel_lir::ir::LirTypeId, PassError> {
        if let Some(i) = self
            .table
            .layouts
            .iter()
            .position(|l| l.kind == LayoutKind::Scalar(ScalarKind::U64))
        {
            let u64_layout = LayoutId(i as u32);
            return Ok(type_of(
                &mut self.ed.unit,
                ValueRepr::Scalar(ScalarKind::U64),
                u64_layout,
            ));
        }
        let bits = scalar_bits(kind);
        let signed = matches!(
            kind,
            ScalarKind::I8 | ScalarKind::I16 | ScalarKind::I32 | ScalarKind::I64 | ScalarKind::I128
        );
        let max = if bits >= 64 {
            u128::MAX
        } else {
            (1u128 << (bits - u32::from(signed))) - 1
        };
        if u128::from(lanes - 1) > max {
            return Err(PassError::Unsupported(format!(
                "{lanes} lanes of {kind:?} in a program without u64"
            )));
        }
        Ok(type_of(&mut self.ed.unit, ValueRepr::Scalar(kind), element))
    }

    /// A failure edge of its own (every edge has one use) into the same
    /// cleanup as `failure`, with its dispositions, anchor and `rule`.
    fn like(&mut self, failure: FailureId, rule: Option<&str>) -> FailureId {
        let f = self.ed.unit.tables.failures[failure.index()].clone();
        let edge = self.ed.unit.tables.edges[f.cleanup.index()].clone();
        let arguments = edge
            .arguments
            .get(&self.ed.unit.pools.values)
            .unwrap_or(&[])
            .to_vec();
        let cleanup = self.ed.edge(edge.target, &arguments);
        let dispose = f
            .dispose
            .get(&self.ed.unit.pools.values)
            .unwrap_or(&[])
            .to_vec();
        let dispose = PoolRange::push(&mut self.ed.unit.pools.values, dispose);
        self.ed.unit.tables.failures.push(FailureEdge {
            cleanup,
            rule: rule.map(str::to_string),
            anchor: f.anchor,
            dispose,
        });
        FailureId(self.ed.unit.tables.failures.len() as u32 - 1)
    }

    fn address(
        &mut self,
        base: AddressBase,
        steps: &[ProjectionStep],
        ty: hirsel_lir::ir::LirTypeId,
    ) -> hirsel_lir::ir::AddressId {
        let projection =
            PoolRange::push(&mut self.ed.unit.pools.projections, steps.iter().copied());
        self.ed.unit.addresses.push(AddressInfo {
            source_place: None,
            unchecked: Vec::new(),
            base,
            projection,
            ty,
        });
        hirsel_lir::ir::AddressId(self.ed.unit.addresses.len() as u32 - 1)
    }

    /// A fresh value defined by an integer constant at the end of `b`.
    fn constant(&mut self, b: BlockId, ty: hirsel_lir::ir::LirTypeId, bits: u128) -> ValueId {
        let data = ConstantData {
            ty,
            value: ConstantValue::Integer(bits),
        };
        let constant = match self
            .ed
            .unit
            .tables
            .constants
            .iter()
            .position(|c| *c == data)
        {
            Some(i) => ConstantId(i as u32),
            None => {
                self.ed.unit.tables.constants.push(data);
                ConstantId(self.ed.unit.tables.constants.len() as u32 - 1)
            }
        };
        let v = self.ed.value(ty, Holding::Plain);
        self.ed.code[b.index()].push(Inst::Const {
            result: v,
            constant,
        });
        v
    }
}

/// Variant ordinals of the core `Option`, `Result` and `FloatIntError`
/// (`None() | Some(T)`, `Ok(T) | Error(E)`, `NotFinite() | OutOfRange()`).
const OPTION_NONE: u32 = 0;
const OPTION_SOME: u32 = 1;
const RESULT_OK: u32 = 0;
const RESULT_ERROR: u32 = 1;
const NOT_FINITE: u32 = 0;
const OUT_OF_RANGE: u32 = 1;

fn integer(kind: ScalarKind) -> bool {
    !matches!(
        kind,
        ScalarKind::Bool | ScalarKind::F32 | ScalarKind::F64 | ScalarKind::Pointer
    )
}

fn scalar_bits(kind: ScalarKind) -> u32 {
    match kind {
        ScalarKind::U8 | ScalarKind::I8 => 8,
        ScalarKind::U16 | ScalarKind::I16 => 16,
        ScalarKind::U32 | ScalarKind::I32 | ScalarKind::F32 => 32,
        ScalarKind::U128 | ScalarKind::I128 => 128,
        _ => 64,
    }
}

// ----- optional rewrites ---------------------------------------------------
//
// An optional rewrite is three steps: `discover` reads a unit and
// returns plans without changing anything; `prove` keeps only the plans whose
// rule and proof domain the ledger (`native-tests/rewrites/ledger.tsv`)
// records as proved; `commit` applies the kept plans. Unknown, timeout and
// refuted entries, and domains the ledger does not list, stay off. The Ready
// validator checks the committed unit again; it trusts no plan.

/// The scalar domain a rewrite's proof covers. A theorem for one width or
/// signedness authorizes no other, and none authorizes SIMD lanes or i128 by
/// default.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Domain {
    Integer(ScalarKind),
    Float(ScalarKind),
}

impl Domain {
    /// The ledger's spelling: `int:u32`, `float:f64`.
    pub fn key(self) -> String {
        match self {
            Domain::Integer(k) => format!("int:{}", scalar_name(k)),
            Domain::Float(k) => format!("float:{}", scalar_name(k)),
        }
    }
}

fn scalar_name(kind: ScalarKind) -> &'static str {
    match kind {
        ScalarKind::U8 => "u8",
        ScalarKind::U16 => "u16",
        ScalarKind::U32 => "u32",
        ScalarKind::U64 => "u64",
        ScalarKind::U128 => "u128",
        ScalarKind::I8 => "i8",
        ScalarKind::I16 => "i16",
        ScalarKind::I32 => "i32",
        ScalarKind::I64 => "i64",
        ScalarKind::I128 => "i128",
        ScalarKind::F32 => "f32",
        ScalarKind::F64 => "f64",
        ScalarKind::Bool => "bool",
        ScalarKind::Pointer => "pointer",
    }
}

/// A ledger entry's verdict. Only `Proved` turns a rewrite on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Status {
    Proved,
    Unknown,
    Timeout,
    Refuted,
}

/// One proof-domain ledger row: rule, domain, the failure and layout
/// assumptions the proof makes, its evidence and its verdict.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LedgerEntry {
    pub rule: String,
    pub domain: String,
    pub failure: String,
    pub layout: String,
    pub evidence: String,
    pub status: Status,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Ledger(pub Vec<LedgerEntry>);

impl Ledger {
    /// Tab-separated rows `rule domain failure layout evidence status`
    /// after a header row; `#` lines are comments.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut rows = Vec::new();
        let mut lines = text
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.starts_with('#'));
        match lines.next() {
            Some("rule\tdomain\tfailure\tlayout\tevidence\tstatus") => {}
            other => return Err(format!("ledger header {other:?}")),
        }
        for line in lines {
            let cols: Vec<&str> = line.split('\t').collect();
            let [rule, domain, failure, layout, evidence, status] = cols[..] else {
                return Err(format!("ledger row with {} columns: {line}", cols.len()));
            };
            let status = match status {
                "proved" => Status::Proved,
                "unknown" => Status::Unknown,
                "timeout" => Status::Timeout,
                "refuted" => Status::Refuted,
                other => return Err(format!("ledger status {other:?}")),
            };
            if rows
                .iter()
                .any(|r: &LedgerEntry| r.rule == rule && r.domain == domain)
            {
                return Err(format!("ledger lists {rule} {domain} twice"));
            }
            rows.push(LedgerEntry {
                rule: rule.into(),
                domain: domain.into(),
                failure: failure.into(),
                layout: layout.into(),
                evidence: evidence.into(),
                status,
            });
        }
        Ok(Self(rows))
    }

    /// A rule's verdict on a domain; an unlisted domain is unknown.
    pub fn status(&self, rule: &str, domain: Domain) -> Status {
        let key = domain.key();
        self.0
            .iter()
            .find(|r| r.rule == rule && r.domain == key)
            .map_or(Status::Unknown, |r| r.status)
    }
}

/// `x * 2` as `x + x`: the same checked failure (SEM-1, the same edge and
/// site) exactly when the product overflows, since 2x = x + x and both test
/// the same range.
pub const MUL_TWO: &str = "mul-two-as-add";

/// A discovered rewrite: where, which rule, over which domain, and the
/// instruction that replaces the one at `(block, index)`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Plan {
    pub rule: &'static str,
    pub block: BlockId,
    pub index: usize,
    pub domain: Domain,
    pub replacement: Inst,
}

/// Every `x * 2` of a unit, read-only.
pub fn discover(unit: &LirUnit<Ready>, table: &LayoutTable) -> Vec<Plan> {
    let mut plans = Vec::new();
    for (b, block) in unit.blocks.iter().enumerate() {
        let insts = block.instructions.get(&unit.instructions).unwrap_or(&[]);
        for (i, inst) in insts.iter().enumerate() {
            let Inst::Binary {
                result,
                op: BinaryOp::Mul,
                left,
                right,
                failure,
            } = *inst
            else {
                continue;
            };
            let layout = unit.types[unit.values[result.index()].ty.index()].layout;
            let Some(LayoutKind::Scalar(kind)) = table.layout(layout).map(|l| &l.kind) else {
                continue;
            };
            let domain = if integer(*kind) {
                Domain::Integer(*kind)
            } else {
                Domain::Float(*kind)
            };
            let x = match (is_two(unit, left), is_two(unit, right)) {
                (_, true) => left,
                (true, false) => right,
                (false, false) => continue,
            };
            plans.push(Plan {
                rule: MUL_TWO,
                block: BlockId(b as u32),
                index: i,
                domain,
                replacement: Inst::Binary {
                    result,
                    op: BinaryOp::Add,
                    left: x,
                    right: x,
                    failure,
                },
            });
        }
    }
    plans
}

/// Whether `v` is the constant 2: defined by a `Const`, or read whole from a
/// slot whose reaching version stored such a value, through any chain of
/// temporaries.
fn is_two(unit: &LirUnit<Ready>, v: ValueId) -> bool {
    let two = |c: ConstantId| match unit.tables.constants[c.index()].value {
        ConstantValue::Integer(n) => n == 2,
        ConstantValue::F32(bits) => f32::from_bits(bits) == 2.0,
        ConstantValue::F64(bits) => f64::from_bits(bits) == 2.0,
        _ => false,
    };
    let definition = |v: ValueId| {
        unit.instructions.iter().find(|i| match **i {
            Inst::Const { result, .. } | Inst::Read { result, .. } => result == v,
            Inst::Store { version, .. } => version == v,
            _ => false,
        })
    };
    let mut v = v;
    // Each step goes to an earlier definition, so the walk ends; the bound
    // only guards malformed input.
    for _ in 0..unit.values.len() {
        match definition(v) {
            Some(&Inst::Const { constant, .. }) => return two(constant),
            Some(&Inst::Read {
                address,
                reaching,
                mode: ReadMode::Bits | ReadMode::Take,
                ..
            }) if unit.addresses[address.index()].projection.len == 0 => {
                match definition(reaching) {
                    Some(&Inst::Store { value, .. }) => v = value,
                    _ => return false,
                }
            }
            _ => return false,
        }
    }
    false
}

/// The plans the ledger proves, and the rest with their verdicts.
pub fn prove(plans: Vec<Plan>, ledger: &Ledger) -> (Vec<Plan>, Vec<(Plan, Status)>) {
    let mut on = Vec::new();
    let mut off = Vec::new();
    for plan in plans {
        match ledger.status(plan.rule, plan.domain) {
            Status::Proved => on.push(plan),
            status => off.push((plan, status)),
        }
    }
    (on, off)
}

/// Apply proved plans; each replaces one instruction in place.
pub fn commit(mut unit: LirUnit<Ready>, plans: &[Plan]) -> LirUnit<Ready> {
    for plan in plans {
        let at = unit.blocks[plan.block.index()].instructions.start as usize + plan.index;
        unit.instructions[at] = plan.replacement;
    }
    unit
}
