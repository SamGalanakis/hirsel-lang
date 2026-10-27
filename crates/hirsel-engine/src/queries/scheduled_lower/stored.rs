//! Ordinary durable variants for admitted Lower and constant producers.
//! The variant index follows the front producer's exact-read lookup. V still
//! validates every restored body and owns execution-reuse authority.
use super::*;
use crate::store::{Entry, VariantRef};

#[derive(serde::Serialize, serde::Deserialize)]
struct Record {
    descriptor: Reference,
    reads_decode_bytes: u64,
    constant_decode_bytes: u64,
    lower: Option<(DefinitionDecodeLayout, u64)>,
}

fn stored_key(key: &EntryKey) -> EntryKey {
    EntryKey(
        ContentHash::of_parts("hirsel.front.lower-variants.v1", [key.0.0.as_slice()]),
        key.1,
        key.2,
    )
}

impl FrontLowerQueries {
    pub(super) fn remember_stored(
        &self,
        id: QueryId,
        key: &EntryKey,
        descriptor: Reference,
        outcome: &Outcome,
    ) {
        let (lower, constant_decode_bytes) = match &outcome.result {
            Ok(Value::Lower(lower)) => (
                Some((
                    lower.definition_decode_layout,
                    lower.constant_selection_bytes,
                )),
                0,
            ),
            Ok(Value::Constant(value)) => (None, value.working_bytes()),
            Err(_) => return,
        };
        // Executed outputs without verifier-owned evidence are deliberately
        // recomputed. An ordinary variant cannot grant execution authority.
        if self.verify == Verify::On && outcome.executed && outcome.execution.is_none() {
            return;
        }
        let record = Record {
            descriptor,
            reads_decode_bytes: outcome.reads_decode_bytes,
            constant_decode_bytes,
            lower,
        };
        let Ok(bytes) = bincode::serialize(&record) else {
            return;
        };
        let key = stored_key(key);
        let Some(variant) = VariantRef::publish(&*self.store, &key, outcome.reads.clone(), &bytes)
        else {
            return;
        };
        let mut entry = Entry::load(&*self.store, &key);
        entry.insert(variant);
        self.store
            .put_query(key.2, &key, entry.action(&key), &entry.encode_for(&key));
        debug_assert!(matches!(id.kind, FRONT_LOWER_KIND | FRONT_EVALUATE_KIND));
    }

    pub(super) fn load_stored(
        &self,
        id: QueryId,
        path: &DefPath,
        key: &EntryKey,
    ) -> Option<Result<ProductRef, Reference>> {
        let storage = stored_key(key);
        let entry = Entry::load(&*self.store, &storage);
        let variant =
            entry.resolve_current(&*self.store, &storage, |read| self.observe(path, read))?;
        // This outer record contains only fixed-size references and bounds.
        let record: Record = bincode::deserialize(&variant.payload).ok()?;
        let domain = if id.kind == FRONT_LOWER_KIND {
            LOWER_DESCRIPTOR
        } else {
            CONSTANT_DESCRIPTOR
        };
        if record.descriptor.domain != domain
            || record.descriptor.len > self.weight.scratch
            || record.reads_decode_bytes > self.weight.scratch
            || record.constant_decode_bytes > self.weight.scratch
        {
            return None;
        }
        let descriptor = self.store.get_content(record.descriptor)?;
        let (value, reads, depth, executed) = if id.kind == FRONT_LOWER_KIND {
            let (layout, constant_selection_bytes) = record.lower?;
            if layout.total()? > self.weight.scratch {
                return None;
            }
            let (definition, body, diagnostics, dependencies, materialized, reads, depth, executed):
                reads::Descriptor = self.records.decode(&self.anchors, &descriptor)?;
            if definition.path() != path
                || body.domain != crate::NATIVE_ORIGINAL_BODY
                || body.len > self.weight.scratch
                || materialized.domain != LOWER_MATERIALIZED
                || materialized.len > self.weight.scratch
            {
                return None;
            }
            let body_bytes = self.store.get_content(body)?;
            let decoded: DefinitionRef = self.records.decode(&self.anchors, &body_bytes)?;
            if decoded.path != definition {
                return None;
            }
            self.store.get_content(materialized)?;
            let mut verified = None;
            let mut acceptance = None;
            if self.verify == Verify::On {
                let (report, ran, accepted, evidence) = self
                    .snapshot
                    .validate_source_artifact(&decoded, |anchor| self.anchors.record_owner(anchor));
                if ran {
                    timings::count(Count::VRuns);
                    self.count(path, |work| work.validation_runs += 1);
                }
                if hirsel_validate::tooling_limit_error(&report).is_some()
                    || !report.violations.is_empty()
                {
                    return None;
                }
                acceptance = accepted;
                if let Some(evidence) = evidence
                    && let Some(cache) = self.store.verifier()
                    && let Some(output) = cache.definition_output_from_evidence(
                        &decoded,
                        self.snapshot.types().clone(),
                        self.snapshot.source_verification_policy(),
                        self.snapshot.validation(&path.0.package),
                        &evidence,
                    )
                    && let Some((authority, bytes)) = cache.compact_definition_output(&output)
                    && bytes.len() as u64 <= self.weight.scratch
                    && let Some(metadata) = self.store.put_content(LOWER_VERIFICATION, &bytes)
                {
                    verified = Some(StoredVerification {
                        authority,
                        metadata,
                    });
                }
            }
            (
                Value::Lower(Box::new(LowerProduct {
                    definition,
                    body,
                    diagnostics,
                    dependencies: dependencies.into(),
                    archived_metadata: None,
                    verified,
                    acceptance,
                    materialized,
                    definition_decode_layout: layout,
                    constant_selection_bytes,
                })),
                reads,
                depth,
                executed,
            )
        } else {
            if record.lower.is_some() {
                return None;
            }
            let (value, substitutions, reads, depth, executed): (
                Materialized,
                Vec<(hirsel_syntax::ExprId, Materialized)>,
                Reads,
                DepthEvidence<Span>,
                bool,
            ) = self.records.decode(&self.anchors, &descriptor)?;
            let mut constants = Constants::new(value, substitutions);
            constants.set_working_bytes(record.constant_decode_bytes);
            (Value::Constant(Box::new(constants)), reads, depth, executed)
        };
        if reads != variant.reads
            || depth.first.len() != dependency_limit(self.limits.call_depth) as usize + 1
        {
            return None;
        }
        let execution_subject = persistent::evaluation_subject(key, &descriptor);
        let execution = if self.verify == Verify::On && executed {
            Some(
                self.store
                    .verifier()?
                    .execution(execution_subject, |package| {
                        self.snapshot.validation_input_digest(package)
                    })?,
            )
        } else {
            None
        };
        let outcome = Outcome {
            result: Ok(value),
            reads,
            reads_decode_bytes: record.reads_decode_bytes,
            depth: depth.into(),
            executed,
            execution,
            execution_subject,
        };
        self.remember_carry(id, key, &descriptor, &outcome);
        Some(self.publish(id, key, domain, &descriptor, outcome, false))
    }
}
