//! Allocation accounting for retained H/J/I products. Reuse the existing
//! capacity/Arc visitor; local type, symbol and definition IDs remain interned.
use super::Weight;
use crate::{AnalyzedBody, QueryOutput};
use hirsel_check_protocol::body::*;
use hirsel_types::*;
use std::sync::Arc;

pub(crate) fn retained_analyzed_bytes(output: &Arc<QueryOutput<AnalyzedBody>>) -> u64 {
    let mut weight = Weight::default();
    weight.arc(output, |w, output| {
        w.vec(&output.diagnostics, Weight::diagnostic);
        w.flat_vec(&output.dependencies);
        w.analyzed(&output.value);
    });
    weight.bytes
}
pub(crate) fn analyzed_capture_bytes(output: &QueryOutput<AnalyzedBody>) -> Option<u64> {
    let mut weight = Weight::decoded();
    weight.vec(&output.diagnostics, Weight::diagnostic);
    weight.flat_vec(&output.dependencies);
    weight.analyzed(&output.value);
    weight.bytes.checked_mul(4)
}
impl Weight<'_> {
    fn contextual_name(&mut self, name: &ContextualName) {
        match name {
            ContextualName::Constructor(path)
            | ContextualName::Variant { owner: path, .. }
            | ContextualName::Field { owner: path, .. } => self.def_path(path),
        }
    }
    fn resolved_name(&mut self, name: &ResolvedName) {
        if let ResolvedName::Definition(path) = name {
            self.def_path(path);
        }
    }
    fn call_target(&mut self, target: &CallTarget) {
        match target {
            CallTarget::Definition(path) => self.def_path(path),
            CallTarget::Constructor(name) => self.contextual_name(name),
            CallTarget::Value(_) => (),
        }
    }
    fn checked_body(&mut self, value: &CheckedBody) {
        let CheckedBody {
            lexical: _,
            places,
            calls,
            tails,
            bindings,
            patterns,
            resolution,
            expressions,
            instantiations,
            closures,
        } = value;
        self.vec(places, |w, place| {
            match &place.root {
                SourceRoot::Binding { binding, .. } => w.resolved_name(binding),
                SourceRoot::Definition(path) => w.def_path(path),
                SourceRoot::Value(_) => (),
            }
            w.path(&place.projections);
        });
        self.vec(calls, |w, call| {
            w.call_target(&call.target);
            w.vec(&call.arguments, |w, argument| {
                if let Some((_, parameter)) = &argument.parameter {
                    if let Some(name) = &parameter.name {
                        w.add(name.capacity());
                    }
                    if let Some(label) = &parameter.label {
                        w.add(label.capacity());
                    }
                }
            });
            w.flat_vec(&call.generic_arguments);
            w.flat_vec(&call.contextual_contracts);
        });
        self.vec(tails, |w, tail| {
            if let CheckedTailKind::ConstructorHole(path) = &tail.kind {
                w.flat_vec(path);
            }
            w.flat_vec(&tail.owned_loop_arguments);
        });
        self.vec(bindings, |w, binding| w.resolved_name(&binding.binding));
        self.vec(patterns, |w, pattern| {
            w.flat_vec(&pattern.fields);
            match &pattern.kind {
                PatternDecisionKind::Constructor(name) => w.contextual_name(name),
                PatternDecisionKind::Or(alternatives) => w.flat_vec(alternatives),
                _ => (),
            }
        });
        self.resolution(resolution);
        self.vec(expressions, |w, expression| {
            if let Some(destination) = &expression.construction_destination {
                w.destination(destination);
            }
            w.flat_vec(&expression.instantiated_arguments);
        });
        self.vec(instantiations, |w, call| {
            w.def_path(&call.callee);
            w.flat_vec(&call.arguments);
        });
        self.vec(closures, |w, closure| {
            w.vec(&closure.captures, |w, capture| {
                w.resolved_name(&capture.binding)
            });
        });
    }
    fn plans(&mut self, value: &Plans) {
        let Plans {
            resource_patterns,
            pattern_literals,
            unreachable_arms,
            tries,
            loops,
            functions,
            shapes,
            contracts,
            facts,
            arm_fixes,
        } = value;
        self.vec(resource_patterns, |w, (_, types)| w.flat_vec(types));
        self.vec(pattern_literals, |w, (_, literal)| match literal {
            hirsel_syntax::Literal::Integer(s)
            | hirsel_syntax::Literal::Float(s)
            | hirsel_syntax::Literal::Character(s)
            | hirsel_syntax::Literal::Byte(s)
            | hirsel_syntax::Literal::String(s)
            | hirsel_syntax::Literal::Bytes(s) => w.add(s.capacity()),
            hirsel_syntax::Literal::Bool(_) => (),
        });
        self.flat_vec(unreachable_arms);
        self.vec(tries, |w, plan| {
            w.def_path(&plan.result);
            if let Some(target) = &plan.mapper {
                w.call_target(target);
            }
        });
        self.vec(loops, |w, plan| {
            if let ForProtocol::BorrowedSource(path) = &plan.protocol {
                w.def_path(path);
            }
            match &plan.kind {
                ForPlanKind::Cursor { cursor } => {
                    if let Some(association) = &cursor.association {
                        w.def_path(&association.source);
                    }
                    w.def_path(&cursor.open);
                    w.flat_vec(&cursor.open_arguments);
                    w.def_path(&cursor.next);
                    w.flat_vec(&cursor.next_arguments);
                    w.def_path(&cursor.step.definition);
                }
                ForPlanKind::Direct { execution, .. } => match execution {
                    ForExecution::Indexed { length, read } => {
                        if let LoopLength::Definition { definition, .. } = length {
                            w.def_path(definition);
                        }
                        if let LoopRead::MapIndex { advance, .. } = read {
                            w.def_path(advance);
                        }
                    }
                    ForExecution::Advance { operation, step } => {
                        w.def_path(operation);
                        w.def_path(&step.definition);
                    }
                    ForExecution::Range | ForExecution::Recovery => (),
                },
            }
        });
        self.flat_vec(functions);
        self.flat_vec(shapes);
        self.vec(contracts, |w, plan| {
            w.flat_vec(&plan.snapshots);
            w.flat_vec(&plan.parameters);
        });
        self.vec(facts, |w, plan| {
            w.def_path(&plan.predicate);
            w.flat_vec(&plan.arguments);
        });
        self.vec(arm_fixes, |w, fix| {
            w.def_path(&fix.owner);
            w.add(fix.arm.capacity());
            w.add(fix.target.capacity());
        });
    }
    fn ownership(&mut self, value: &OwnershipFacts) {
        let OwnershipFacts {
            inferred_unique,
            unique_failures,
            confined_releases,
            borrow_cursors,
            cursor_rebindings,
            plain_types,
            silent_drop_types,
            field_loans,
            composed_arguments,
            captures,
            call_arguments,
            identity_joins,
            identity_fields,
            physical_projections,
            physical_patterns,
            unique,
            guarded_patterns,
            allocating_mutations,
            replacements,
        } = value;
        self.flat_vec(&inferred_unique.preserves_unique);
        self.flat_vec(&inferred_unique.results);
        self.flat_vec(&inferred_unique.nonretaining);
        self.vec(unique_failures, |w, failure| {
            w.add(failure.message.capacity())
        });
        self.flat_vec(confined_releases);
        self.vec(borrow_cursors, |w, cursor| {
            if let Some(cover) = &cursor.cover {
                w.vec(cover, |w, (origin, types)| {
                    w.origin(origin);
                    w.flat_vec(types);
                });
            }
        });
        self.flat_vec(cursor_rebindings);
        // The production ownership pass and archive restore only insert into
        // these sets. No tombstone/deletion history can hide extra buckets.
        for set in [plain_types, silent_drop_types] {
            if set.capacity() != 0 {
                let buckets = set
                    .capacity()
                    .saturating_mul(8)
                    .div_ceil(7)
                    .checked_next_power_of_two()
                    .unwrap_or(usize::MAX)
                    .max(4);
                self.add(
                    buckets
                        .saturating_mul(size_of::<TypeId>() + 1)
                        .saturating_add(16 + align_of::<TypeId>() - 1),
                );
            }
        }
        self.flat_vec(field_loans);
        self.flat_vec(composed_arguments);
        self.flat_vec(captures);
        self.flat_vec(call_arguments);
        self.vec(identity_joins, |w, (_, _, origin)| w.origin(origin));
        self.vec(identity_fields, |w, (_, _, origin, fields)| {
            w.origin(origin);
            w.flat_vec(fields);
        });
        self.flat_vec(physical_projections);
        self.flat_vec(physical_patterns);
        self.flat_vec(unique);
        self.flat_vec(guarded_patterns);
        self.flat_vec(allocating_mutations);
        self.flat_vec(replacements);
    }
    fn analyzed(&mut self, value: &AnalyzedBody) {
        let AnalyzedBody {
            typed,
            ownership,
            results,
            call_allocations,
        } = value;
        self.arc(typed, |w, typed| {
            let crate::TypedBody {
                syntax,
                checked,
                plans,
                unit: _,
                boolean: _,
            } = typed;
            w.arc(syntax, |w, syntax| w.syntax(syntax));
            w.checked_body(checked);
            w.plans(plans);
        });
        self.ownership(ownership);
        self.flat_vec(results);
        self.flat_vec(call_allocations);
    }
}
