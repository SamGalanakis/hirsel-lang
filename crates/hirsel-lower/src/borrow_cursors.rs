//! Existing-construct transport for body-local borrowed cursor bindings.
use super::builder::{Builder, Value};
use super::*;
use hirsel_tir::*;
use hirsel_types::{BorrowCursorDecision, Origin, OriginRoot, ResolvedName, UseKind};
impl Builder<'_, '_> {
    pub(super) fn cursor_backed(&self, place: PlaceId) -> bool {
        let mut pending = vec![self.root(place)];
        let mut seen = std::collections::BTreeSet::new();
        while let Some(root) = pending.pop() {
            if !seen.insert(root.0) {
                continue;
            }
            if self.body.borrow_cursors.iter().any(|c| c.carrier == root) {
                return true;
            }
            for instruction in self.body.blocks.iter().flat_map(|b| &b.instructions) {
                if let InstructionKind::Assign { destination, value } = instruction.kind
                    && self.root(destination) == root
                    && let Rvalue::Use(operand) = &self.body.rvalues[value.0 as usize]
                {
                    pending.push(self.root(operand.place));
                }
            }
        }
        false
    }

    fn cursor_cover_place(
        &mut self,
        origin: &Origin,
        fields: &[TypeId],
        span: Span,
    ) -> Result<PlaceId> {
        let mut place = match &origin.root {
            OriginRoot::Local(owner, local) if *owner == self.decisions.lexical => {
                self.lookup(&ResolvedName::Local(*local), &self.owner, span)?
            }
            OriginRoot::Parameter(hirsel_types::ParameterRef::Body { scope, parameter })
                if scope.owner == self.decisions.lexical =>
            {
                self.lookup(&ResolvedName::Parameter(*parameter), &scope.site, span)?
            }
            _ => return self.error(span, "cursor cover is not an enclosing value place"),
        };
        if fields.len() != origin.fields.len() {
            return self.error(span, "missing cursor cover field rows");
        }
        for (field, declared) in origin.fields.iter().zip(fields) {
            let ty = self.part_type(self.ty(place), *declared);
            place = self.project(place, Projection::Field(field.0), ty);
        }
        Ok(place)
    }
    pub(super) fn cursor_binding(
        &mut self,
        value: Value,
        ty: TypeId,
        claim: &BorrowCursorDecision,
        span: Span,
    ) -> Result<Value> {
        if value.usage != UseKind::Borrow {
            return self.error(span, "cursor declaration is not borrowed");
        }
        let cover = claim
            .cover
            .as_ref()
            .map(|cover| {
                cover
                    .iter()
                    .map(|(origin, fields)| self.cursor_cover_place(origin, fields, span))
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?;
        let carrier = self.transport_typed(value, ty, Storage::Temporary, span);
        let initializer = RvalueId(self.body.rvalues.len() as u32 - 1);
        self.body.borrow_cursors.push(BorrowCursor {
            carrier: self.root(carrier.place),
            initializer,
            cover,
        });
        self.retain_view(carrier.place);
        Ok(carrier)
    }
    pub(super) fn cursor_rebind(
        &mut self,
        destination: PlaceId,
        incoming: Value,
        span: Span,
    ) -> Result<()> {
        let root = self.root(destination);
        if !self
            .body
            .borrow_cursors
            .iter()
            .any(|cursor| cursor.carrier == root)
            || !(incoming.usage == UseKind::Borrow
                || incoming.usage == UseKind::Take
                    && self.body.locals[self.root(incoming.place).0 as usize].storage
                        == Storage::Temporary
                    && !self.value_owned(incoming))
            || !self.body.places[destination.0 as usize]
                .projection
                .is_empty()
        {
            return self.error(span, "invalid borrowed cursor replacement plan");
        }
        let parent = self.scope;
        let rhs_scope = ScopeId(self.body.scopes.len() as u32);
        self.body.scopes.push(Scope {
            parent: Some(parent),
            span,
        });
        self.scope = rhs_scope;
        let rhs = self.transport_typed(incoming, self.ty(destination), Storage::Temporary, span);
        let retired_scope = ScopeId(self.body.scopes.len() as u32);
        self.body.scopes.push(Scope {
            parent: Some(parent),
            span,
        });
        self.scope = retired_scope;
        let retired = self.slot(self.ty(destination), Storage::Temporary, span, false);
        self.emit(InstructionKind::StorageLive(self.root(retired)), span);
        self.scope = parent;
        // All RHS/address checks precede this failure-free transfer interval.
        let outgoing = self.consume(Value {
            place: destination,
            usage: UseKind::Take,
            checked_ty: None,
        });
        self.assign(retired, Rvalue::Use(outgoing), span);
        let incoming = self.consume(Value {
            usage: UseKind::Take,
            ..rhs
        });
        let carrier = self.roots[root.0 as usize];
        self.assign(carrier, Rvalue::Use(incoming), span);
        if let Some(origin) = self.body.places[destination.0 as usize].binding.clone() {
            self.bind_origin(carrier, origin, OriginBindingKind::Alias, span);
        }
        let held: std::collections::BTreeSet<_> = self
            .content_loans(destination)
            .into_iter()
            .flat_map(|(_, loans)| loans)
            .collect();
        let home = self.body.locals[root.0 as usize].scope;
        for (loan, scope) in &mut self.state.loans {
            if held.contains(&loan.0) {
                *scope = home;
            }
        }
        self.exit_scope(retired_scope, span);
        self.exit_scope(rhs_scope, span);
        self.end_unheld_loans(span);
        Ok(())
    }
}
