//! `task.isolate` (FAIL-2): the runtime's `isolate.rs`. The `Isolate`
//! terminator moves the running task's failures since the body's record count
//! into the caught chain, its own attributed to its spawn path; the body reads
//! each record's fields through the `$intrinsic.Isolate*` entries. A site's
//! line and column come from the operations model, which holds the sources.
use super::*;

#[derive(Clone)]
pub(super) struct Invocation {
    pub(super) mark: usize,
    pub(super) caught: Option<Vec<(ObservedFailure, Vec<u32>)>>,
}

impl<S: Stage> Machine<'_, S> {
    /// The `Isolate` terminator: catch every failure since `start`.
    pub(super) fn isolate_catch(&mut self, f: &mut Frame, start: ValueId) -> Checked<()> {
        if !f.failing {
            return fail(V::Malformed, "isolate without an active failure");
        }
        let Val::Bits { bits, .. } = self.read_use(f, start)? else {
            return fail(V::TypeConfusion, "an isolation start that is not a count");
        };
        let start = bits as usize;
        if start >= self.failures.len() {
            return fail(V::Malformed, "an isolation that caught no failure");
        }
        let failures = self.failures.split_off(start);
        self.unanchored_failures.retain(|&i| i < start);
        let s = self.scheduler();
        let current = s.current;
        let own = s.task(current)?.path.clone();
        let spawns = s.task_spawns();
        spawns.resize(start + failures.len(), None);
        let spawns = spawns.split_off(start);
        let chain = failures
            .into_iter()
            .zip(spawns)
            .map(|(failure, spawn)| (failure, spawn.unwrap_or_else(|| own.clone())))
            .collect();
        let scope = s
            .task(current)?
            .isolations
            .last_mut()
            .ok_or_else(|| err(V::Malformed, "isolation catch without begin"))?;
        if scope.mark != start || scope.caught.is_some() {
            return fail(V::Malformed, "isolation catch does not match its begin");
        }
        scope.caught = Some(chain);
        f.failing = false;
        Ok(())
    }

    pub(super) fn isolate_operation(
        &mut self,
        f: &mut Frame,
        site: &CallSite,
        name: &str,
        arguments: &[CallArgument],
        normal: Option<EdgeId>,
    ) -> Checked<Control> {
        if name == "$intrinsic.IsolateEnd" {
            let s = self.scheduler();
            let current = s.current;
            s.task(current)?
                .isolations
                .pop()
                .ok_or_else(|| err(V::Malformed, "isolation ended without begin"))?;
            return self.task_return(f, arguments, None, normal);
        }
        let result = site
            .result
            .ok_or_else(|| err(V::Malformed, "an isolation entry without a result"))?;
        let bits = |scalar, bits: u64| Val::Bits {
            scalar,
            bits: u128::from(bits),
        };
        let val = match name {
            "$intrinsic.IsolateBegin" => {
                let count = self.failures.len() as u64;
                let s = self.scheduler();
                let current = s.current;
                s.task(current)?.isolations.push(Invocation {
                    mark: count as usize,
                    caught: None,
                });
                self.task_yield()?;
                bits(ScalarKind::U64, count)
            }
            "$intrinsic.IsolateCount" => bits(ScalarKind::U64, self.isolate_caught()?.len() as u64),
            _ => {
                let index = self.task_word(f, &Self::task_argument(arguments, 1)?)?;
                let (failure, spawn) = usize::try_from(index)
                    .ok()
                    .and_then(|i| self.isolate_caught().ok()?.get(i).cloned())
                    .ok_or_else(|| err(V::Malformed, "an isolated failure index out of range"))?;
                // A site inside a library body, which only a resource failure such as an
                // allocation's reaches, has no source position: it is line and column 0, as
                // the runtime's failure table lists it.
                let position = || -> Checked<(u64, u64)> {
                    Ok(self
                        .ops
                        .failure_source_position(
                            &failure.definition,
                            hirsel_lir::ir::SiteRange {
                                source: failure.source,
                                start: failure.site.0,
                                end: failure.site.1,
                            },
                        )
                        .unwrap_or((0, 0)))
                };
                let value = match name {
                    "$intrinsic.IsolateKind" => OpValue::Bits {
                        scalar: ScalarKind::U32,
                        bits: u128::from(hirsel_core_contract::failure_kind(&failure.rule)),
                    },
                    "$intrinsic.IsolateLine" => OpValue::Bits {
                        scalar: ScalarKind::U64,
                        bits: u128::from(position()?.0),
                    },
                    "$intrinsic.IsolateColumn" => OpValue::Bits {
                        scalar: ScalarKind::U64,
                        bits: u128::from(position()?.1),
                    },
                    "$intrinsic.IsolateRule" => OpValue::ByteSequence(failure.rule.into_bytes()),
                    "$intrinsic.IsolateDefinition" => {
                        OpValue::ByteSequence(failure.definition.to_string().into_bytes())
                    }
                    "$intrinsic.IsolateSpawn" => OpValue::Sequence(
                        spawn
                            .iter()
                            .map(|&ordinal| OpValue::Bits {
                                scalar: ScalarKind::U64,
                                bits: u128::from(ordinal),
                            })
                            .collect(),
                    ),
                    _ => unreachable!("an isolation entry"),
                };
                match value {
                    OpValue::Bits { scalar, bits } => Val::Bits { scalar, bits },
                    value => {
                        let layout = self.value_type(f.unit, result)?.layout;
                        let image = self.operation_image(
                            f,
                            layout,
                            value,
                            arguments,
                            &mut Default::default(),
                        )?;
                        let repr = self.value_type(f.unit, result)?.repr;
                        self.val_from(image, repr, layout)?.0
                    }
                }
            }
        };
        self.task_return(f, arguments, Some((result, val)), normal)
    }

    fn isolate_caught(&mut self) -> Checked<&[(ObservedFailure, Vec<u32>)]> {
        let s = self.scheduler();
        let current = s.current;
        s.task(current)?
            .isolations
            .last()
            .and_then(|scope| scope.caught.as_deref())
            .ok_or_else(|| err(V::Malformed, "isolation read without a caught invocation"))
    }
}
