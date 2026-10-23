//! Telemetry capture in the simulated world (`telemetry.null@1`, CAP-1/2, DROP-2/3,
//! CONC-1/3): span identities come from the configured seed through the identity rules
//! the native runtime shares (`hirsel_kernels::telemetry`). An accepted observation is a
//! scheduling point with no cancellation check; a payload refusal issues nothing.
use super::dispatch::{arg, variant};
use super::simulation::Grant;
use super::{Library, error, ok};
use crate::{Error, OperationCall, OperationContext, OperationResult, Value, invalid};
use hirsel_kernels::telemetry as kernel;

fn payload(v: &Value) -> Result<&[u8], Error> {
    match v {
        Value::Bytes(bytes) | Value::ArenaBytes { bytes, .. } => Ok(bytes),
        _ => Err(invalid("telemetry payload")),
    }
}
fn refused(refusal: kernel::Refusal) -> Value {
    error(variant(refusal.ordinal(), vec![]))
}

impl Library {
    /// The Span `s` of Telemetry `t` (its brand, CAP-2).
    fn span_of(&self, t: u64, s: &Value) -> Result<u64, Error> {
        let span = self.simulation.id(s, "Span")?;
        match self.simulation.grants.get(&span) {
            Some(Grant::Span { telemetry, .. }) if *telemetry == t => Ok(span),
            _ => Err(invalid("span of another Telemetry")),
        }
    }
    pub(super) fn telemetry_call(
        &mut self,
        name: &str,
        call: &OperationCall<'_>,
        context: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        let a = &*call.arguments;
        if name == "io.telemetry" {
            self.tasks.yield_now = true;
            self.simulation.id(arg(a, 0)?, "Io")?;
            return Ok(OperationResult::returned(
                if self.simulation.config.projections.contains("telemetry") {
                    ok(self
                        .simulation
                        .handle("Telemetry", Grant::Root("Telemetry".into())))
                } else {
                    error(variant(0, vec![]))
                },
            ));
        }
        let t = self.simulation.id(arg(a, 0)?, "Telemetry")?;
        if name == "capture.context" {
            // Derived from the begin's completion: no request and no scheduling point.
            let span = self.span_of(t, arg(a, 1)?)?;
            let Some(Grant::Span { context, .. }) = self.simulation.grants.get(&span) else {
                return Err(invalid("telemetry span"));
            };
            return Ok(OperationResult::returned(Value::Bytes(context.clone())));
        }
        if name == "capture.begin" {
            let (parent, spec) = (payload(arg(a, 1)?)?, payload(arg(a, 2)?)?);
            let parent_trace = match kernel::bounded(&[spec]).and_then(|()| kernel::parent(parent))
            {
                Ok(trace) => trace,
                Err(refusal) => return Ok(OperationResult::returned(refused(refusal))),
            };
            let (trace, span) = self.simulation.telemetry_ids(parent_trace)?;
            let mut input = t.to_le_bytes().to_vec();
            input.extend_from_slice(&(parent.len() as u64).to_le_bytes());
            input.extend_from_slice(parent);
            input.extend_from_slice(spec);
            let mut output = trace.to_vec();
            output.extend_from_slice(&span);
            self.simulation
                .record("Telemetry", "capture.begin", input, output, context)?;
            self.tasks.yield_now = true;
            let handle = self.simulation.handle(
                "Span",
                Grant::Span {
                    telemetry: t,
                    context: kernel::context(&trace, &span, parent),
                },
            );
            return Ok(OperationResult::returned(ok(handle)));
        }
        let (span, data) = match name {
            "capture.event" | "capture.finish" => {
                (Some(self.span_of(t, arg(a, 1)?)?), payload(arg(a, 2)?)?)
            }
            "capture.log" | "capture.measure" => (None, payload(arg(a, 1)?)?),
            _ => return Err(invalid(format!("telemetry operation {name}"))),
        };
        if let Err(refusal) = kernel::bounded(&[data]) {
            // A refused status still ends the taken span: it is abandoned.
            if name == "capture.finish"
                && let Some(span) = span
            {
                self.abandon(span, context)?;
            }
            return Ok(OperationResult::returned(refused(refusal)));
        }
        let mut input = t.to_le_bytes().to_vec();
        if let Some(span) = span {
            input.extend_from_slice(&span.to_le_bytes());
        }
        input.extend_from_slice(data);
        self.simulation
            .record("Telemetry", name, input, vec![], context)?;
        if name == "capture.finish"
            && let Some(span) = span
        {
            self.simulation.grants.remove(&span);
        }
        self.tasks.yield_now = true;
        Ok(OperationResult::returned(ok(Value::Unit)))
    }
    /// A Span's implicit drop: recorded as an abandonment, with no outcome claimed.
    pub(super) fn abandon(
        &mut self,
        span: u64,
        context: &mut OperationContext<'_>,
    ) -> Result<(), Error> {
        if self.simulation.grants.remove(&span).is_some() {
            self.simulation.record(
                "Telemetry",
                "span.abandon",
                span.to_le_bytes().to_vec(),
                vec![],
                context,
            )?;
        }
        Ok(())
    }
}
