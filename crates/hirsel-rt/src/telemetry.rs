//! Telemetry capture (`telemetry.capture@1`, `telemetry.null@1`): the observations a run
//! accepts, the span identities its provider supplies and the null provider's simulated
//! exporter. Capture is not delivery: an accepted observation is the request on the tape,
//! and nothing a provider or exporter does changes what the program observes.
#[unsafe(export_name = "hirsel.runtime.telemetry.provider")]
static PROVIDER: crate::runtime_roots::Telemetry = crate::runtime_roots::Telemetry {
    observe_span: crate::tasks::free::host::native::SpanState::observe_linked,
    draw: State::telemetry_draw,
    context: State::telemetry_context,
    execute: State::execute_telemetry,
    sample: State::telemetry_sample,
};

use crate::handles::Grant;
use crate::io::{AdapterError, Completion, IoError, Request, State};
pub use hirsel_kernels::telemetry::{self as kernel, Issued, NullIds, Refusal};

/// Where a live begin's identities come from. Replay takes the recorded ones instead.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Ids {
    /// `telemetry.null@1`: the grant seed's SplitMix64 stream.
    Null(NullIds),
    /// `telemetry.capture@1`: the host's secure generator, never the run's Random.
    Host,
}

/// The null provider's exporter: a queue of `capacity` observations that never drains.
/// An observation that finds it full is dropped; neither changes acceptance.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Exporter {
    pub capacity: Option<u64>,
    pub queued: u64,
    pub dropped: u64,
}
impl Exporter {
    fn offer(&mut self) {
        if self
            .capacity
            .is_some_and(|capacity| self.queued >= capacity)
        {
            self.dropped += 1;
        } else {
            self.queued += 1;
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Capture {
    pub(crate) ids: Ids,
    issued: Issued,
    pub exporter: Exporter,
}
impl Default for Capture {
    fn default() -> Self {
        Self {
            ids: Ids::Null(NullIds::new(0)),
            issued: Issued::default(),
            exporter: Exporter::default(),
        }
    }
}
impl Capture {
    /// The provider a run's grant names: its contract, seed and exporter capacity.
    pub fn of(
        provider: hirsel_run_schema::ProviderContract,
        seed: u64,
        queue: Option<u64>,
    ) -> Self {
        Self {
            ids: match provider {
                hirsel_run_schema::ProviderContract::TelemetryCapture => Ids::Host,
                _ => Ids::Null(NullIds::new(seed)),
            },
            issued: Issued::default(),
            exporter: Exporter {
                capacity: queue,
                ..Exporter::default()
            },
        }
    }
}

/// Whether `request` is a telemetry operation on a Telemetry or Span.
pub(crate) fn is_telemetry(request: &Request) -> bool {
    matches!(
        request,
        Request::TelemetryBegin { .. }
            | Request::TelemetryEvent { .. }
            | Request::TelemetryFinish { .. }
            | Request::TelemetryLog { .. }
            | Request::TelemetryMeasure { .. }
    )
}

impl State {
    fn span_of(&self, telemetry: u64, span: u64) -> Result<(), AdapterError> {
        self.expect(telemetry, Grant::Telemetry)?;
        match self.handles.rows.get(&span) {
            Some(Grant::Span {
                telemetry: owner, ..
            }) if *owner == telemetry => Ok(()),
            _ => Err(AdapterError::InvalidHandle(span)),
        }
    }
    /// A begin's parent trace, re-derived from the request: requests reach a provider only
    /// once their payloads passed the deterministic checks.
    fn begin_parent(
        &self,
        telemetry: u64,
        parent: &[u8],
        spec: &[u8],
    ) -> Result<Option<[u8; 16]>, AdapterError> {
        self.expect(telemetry, Grant::Telemetry)?;
        kernel::bounded(&[spec]).map_err(|_| AdapterError::InvalidCompletion)?;
        kernel::parent(parent).map_err(|_| AdapterError::InvalidCompletion)
    }
    /// A live begin: the provider's identities for the next handle.
    pub(crate) fn telemetry_draw(&mut self, request: &Request) -> Result<Completion, AdapterError> {
        let Request::TelemetryBegin {
            telemetry,
            parent,
            spec,
        } = request
        else {
            return Err(AdapterError::InvalidCompletion);
        };
        let parent = self.begin_parent(*telemetry, parent, spec)?;
        let capture = &mut self.telemetry;
        let (trace, span) = match &mut capture.ids {
            Ids::Null(ids) => capture.issued.choose(parent, |n| ids.draw(n)),
            Ids::Host => {
                let mut failed = false;
                let chosen = capture.issued.choose(parent, |n| {
                    crate::io::RandomSource::System
                        .draw(n as u64)
                        .unwrap_or_else(|_| {
                            failed = true;
                            // A nonzero, never admitted draw ends the search.
                            vec![0; n]
                        })
                });
                if failed {
                    return Err(AdapterError::Unsupported(
                        "the host's secure generator is unavailable for span identities".into(),
                    ));
                }
                chosen
            }
        };
        Ok(Completion::Span {
            handle: self.handles.next(),
            trace,
            span,
        })
    }
    /// Accept a begin's identities, live or recorded: the next handle, admissible IDs.
    pub(crate) fn telemetry_sample(
        &mut self,
        request: &Request,
        input: Completion,
    ) -> Result<Completion, AdapterError> {
        let (
            Request::TelemetryBegin {
                telemetry,
                parent,
                spec,
            },
            Completion::Span {
                handle,
                trace,
                span,
            },
        ) = (request, &input)
        else {
            return Err(AdapterError::InvalidCompletion);
        };
        let parent_trace = self.begin_parent(*telemetry, parent, spec)?;
        if *handle != self.handles.next()
            || !self.telemetry.issued.admits(parent_trace, trace, span)
        {
            return Err(AdapterError::InvalidCompletion);
        }
        self.telemetry.issued.accept(parent_trace, *trace, *span);
        self.handles.insert(Grant::Span {
            telemetry: *telemetry,
            context: kernel::context(trace, span, parent),
        });
        self.telemetry.exporter.offer();
        Ok(input)
    }
    /// The operations whose completion the run derives: projection, span events, finish,
    /// logs, measurements and an abandoning close.
    pub(crate) fn execute_telemetry(
        &mut self,
        request: &Request,
    ) -> Result<Completion, AdapterError> {
        let payload = match request {
            Request::Telemetry { io } => {
                self.expect(*io, Grant::Io)?;
                if !self.projections.contains("telemetry") {
                    return Ok(Completion::Handle(Err(IoError::NotGranted)));
                }
                return Ok(Completion::Handle(Ok(self
                    .handles
                    .insert(Grant::Telemetry))));
            }
            Request::TelemetryEvent {
                telemetry,
                span,
                payload,
            }
            | Request::TelemetryFinish {
                telemetry,
                span,
                status: payload,
            } => {
                self.span_of(*telemetry, *span)?;
                payload
            }
            Request::TelemetryLog { telemetry, payload }
            | Request::TelemetryMeasure { telemetry, payload } => {
                self.expect(*telemetry, Grant::Telemetry)?;
                payload
            }
            Request::Close { handle } => {
                // An implicit drop abandons the span: no outcome is claimed.
                self.handles.rows.remove(handle);
                self.telemetry.exporter.offer();
                return Ok(Completion::Unit(Ok(())));
            }
            _ => return Err(AdapterError::InvalidCompletion),
        };
        kernel::bounded(&[payload]).map_err(|_| AdapterError::InvalidCompletion)?;
        if let Request::TelemetryFinish { span, .. } = request {
            self.handles.rows.remove(span);
        }
        self.telemetry.exporter.offer();
        Ok(Completion::Unit(Ok(())))
    }
    /// A span's context, derived from its begin (no request).
    pub fn telemetry_context(&self, telemetry: u64, span: u64) -> Result<Vec<u8>, AdapterError> {
        self.span_of(telemetry, span)?;
        match self.handles.rows.get(&span) {
            Some(Grant::Span { context, .. }) => Ok(context.clone()),
            _ => Err(AdapterError::InvalidHandle(span)),
        }
    }
}
