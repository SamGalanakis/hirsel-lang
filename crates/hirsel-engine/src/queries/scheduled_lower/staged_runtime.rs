//! Close runtime size selections from settled summaries. Compile-time calls
//! remain selected by execution, so an unexecuted branch creates no instance.
use super::*;
use hirsel_types::{GenericArgument, NameSite, Size, SizeParameterRef};
use std::collections::VecDeque;

type Bindings = BTreeMap<SizeParameterRef, u64>;
type Frame = (DefId, NameSite, Bindings, Option<Span>);
pub(crate) type Selection = (QueryId, Option<Span>);

impl FrontLowerQueries {
    pub(crate) fn runtime_staged_queries(&self, limit: u64) -> Result<Vec<Selection>, QueryError> {
        if self.staged.declarations.is_empty() {
            return Ok(Vec::new());
        }
        let refused = || {
            QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "runtime staged selection".into(),
                message: "selection exceeds admission or lacks a settled summary".into(),
            })
        };
        let mut pending = VecDeque::<Frame>::new();
        let mut seen = BTreeSet::new();
        let mut selected = Vec::new();
        let mut weight = crate::NativeMetadataWeight::default();
        for path in self.inputs.keys() {
            let signature = self.signature(path)?;
            if matches!(signature.kind, hirsel_types::SignatureKind::Const { .. }) {
                continue;
            }
            let Some(generic) = self
                .front
                .with_settled_summary(path, |s| s.frames[0].generic)
            else {
                continue;
            };
            if !generic {
                weight.add(size_of::<Frame>());
                if weight.bytes() > limit {
                    return Err(refused());
                }
                pending.push_back((DefId::intern(path), NameSite::Item, Bindings::new(), None));
            }
        }
        while let Some((path, scope, bindings, site)) = pending.pop_front() {
            if seen.contains(&(path, scope, bindings.clone())) {
                continue;
            }
            weight.add(size_of::<Frame>() + 16 * size_of::<usize>());
            weight.tree::<SizeParameterRef, u64>(bindings.len());
            if seen.len() >= 65536 || weight.bytes() > limit {
                return Err(refused());
            }
            seen.insert((path, scope, bindings.clone()));
            self.front
                .with_settled_summary(path.path(), |summary| -> Result<(), QueryError> {
                    let frames = &summary.frames;
                    let frame_of = |site: NameSite| {
                        frames.iter().position(|f| f.site == site).map(|i| i as u32)
                    };
                    let function = |mut frame: u32| {
                        while frames[frame as usize].kind == FrameKind::ComptimeRegion {
                            frame = frames[frame as usize].parent.expect("region parent");
                        }
                        frame
                    };
                    let staged = |mut frame: u32| loop {
                        if frames[frame as usize].kind == FrameKind::ComptimeRegion {
                            return true;
                        }
                        match frames[frame as usize].parent {
                            Some(parent) => frame = parent,
                            None => return false,
                        }
                    };
                    let Some(current) = frame_of(scope) else {
                        return Ok(());
                    };
                    for (i, frame) in frames.iter().enumerate() {
                        let i = i as u32;
                        if frame.kind != FrameKind::ComptimeRegion
                            && i != current
                            && frame.parent.map(function) == Some(current)
                            && !staged(i)
                            && !frame.generic
                        {
                            push(
                                &mut pending,
                                &mut weight,
                                limit,
                                (path, frame.site, bindings.clone(), site),
                            )?;
                        }
                    }
                    for (id, declaration) in &self.staged.declarations {
                        if declaration.owner != *path.path()
                            || frame_of(NameSite::Expression(declaration.expression))
                                .and_then(|region| frames[region as usize].parent)
                                .map(function)
                                != Some(current)
                        {
                            continue;
                        }
                        weight.add(
                            size_of::<Selection>() + declaration.inputs.len() * size_of::<u64>(),
                        );
                        if weight.bytes() > limit {
                            return Err(refused());
                        }
                        let arguments = declaration
                            .inputs
                            .iter()
                            .map(|input| bindings.get(input).copied())
                            .collect::<Option<Vec<_>>>()
                            .ok_or_else(refused)?;
                        selected.push((self.staged_query(id.path(), &arguments)?, site));
                    }
                    for call in &summary.calls {
                        if function(call.owning_frame) != current
                            || call.class == CallClass::ComptimeExecuted
                        {
                            continue;
                        }
                        let (callee, binder, mut actuals) = match call.target_frame {
                            None => {
                                if !self.inputs.contains_key(call.callee.path()) {
                                    continue;
                                }
                                (call.callee, NameSite::Item, Bindings::new())
                            }
                            Some(_) => (path, call.binder.site, bindings.clone()),
                        };
                        for (ordinal, argument) in call.arguments.iter().enumerate() {
                            if let GenericArgument::Size(size) = argument {
                                let value = match size {
                                    Size::Constant(n) => Some(*n),
                                    Size::Parameter(parameter) => bindings.get(parameter).copied(),
                                }
                                .ok_or_else(refused)?;
                                actuals.insert(
                                    SizeParameterRef::body(
                                        call.binder,
                                        hirsel_types::SizeParamId(ordinal as u32),
                                    ),
                                    value,
                                );
                            }
                        }
                        push(
                            &mut pending,
                            &mut weight,
                            limit,
                            (callee, binder, actuals, Some(call.site)),
                        )?;
                    }
                    Ok(())
                })
                .ok_or_else(refused)??;
        }
        Ok(selected)
    }
}

fn push(
    pending: &mut VecDeque<Frame>,
    weight: &mut crate::NativeMetadataWeight,
    limit: u64,
    frame: Frame,
) -> Result<(), QueryError> {
    weight.add(size_of::<Frame>());
    weight.tree::<SizeParameterRef, u64>(frame.2.len());
    if weight.bytes() > limit {
        return Err(QueryError::Operational(hirsel_types::OperationalFailure {
            operation: "runtime staged selection".into(),
            message: "selection exceeds admission".into(),
        }));
    }
    pending.push_back(frame);
    Ok(())
}
