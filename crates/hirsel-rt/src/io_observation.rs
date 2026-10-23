//! Initialized host-result observations shared by execution modes.
//! Capture mapping bytes while the request still owns their backing storage,
//! then identify every initialized result handle before logical substitution.
//! These helpers own no scheduler, adapter, identity map or tape format.
use crate::io::{AdapterError, Completion};
use crate::stop::{Stop, Stopped};

/// A recorded mapping lends owned bytes to both the live caller and replay.
/// Capture while the adapter still owns its mapping, before Ended can publish
/// or another request can close it. Reuse MapView's established byte codec.
pub(crate) fn capture(result: &mut Result<Completion, AdapterError>) -> Stopped<()> {
    if let Ok(Completion::View(view)) = result {
        if let Some(bytes) = &view.bytes {
            if view.length != bytes.len() as u64 {
                return Err(Stop::Refused("host mapping length mismatch".into()));
            }
        } else if view.length == 0 {
            view.bytes = Some(Vec::new());
        } else {
            if view.block == 0 {
                return Err(Stop::Refused("host mapping has no live storage".into()));
            }
            view.bytes = Some(view.contents());
        }
        view.block = 0;
    }
    Ok(())
}

/// The initialized handle fields in an admitted result. Keep this exhaustive
/// allow-list common to recording and replay: unsupported results must never
/// leak a host word just because they lack creation metadata.
pub(crate) fn fields(
    result: &mut Result<Completion, AdapterError>,
) -> Result<[Option<&mut u64>; 2], &'static str> {
    match result {
        Ok(
            Completion::Handle(Ok(word))
            | Completion::ClockGrant(Ok(word))
            | Completion::OptionalFile(Some(word))
            | Completion::NetHandle(Ok(word))
            | Completion::Spawned(Ok(word)),
        )
        | Ok(Completion::Listened(Ok((word, _))) | Completion::Accepted(Ok((word, _))))
        | Ok(Completion::Span { handle: word, .. }) => Ok([Some(word), None]),
        Ok(Completion::Halves(Ok((read, write)))) => Ok([Some(read), Some(write)]),
        Err(AdapterError::InvalidHandle(_)) => Err("host refusal needs a logical handle identity"),
        Err(
            AdapterError::ReplayMismatch(_)
            | AdapterError::MissingInput(_)
            | AdapterError::UnconsumedInput(_)
            | AdapterError::InvalidCompletion
            | AdapterError::RecordingFailed(_)
            | AdapterError::Blocked
            | AdapterError::Unsupported(_)
            | AdapterError::Signaled(_)
            | AdapterError::Stopped,
        )
        | Ok(
            Completion::Input(_)
            | Completion::Env(_)
            | Completion::Monotonic(_)
            | Completion::Wall(_)
            | Completion::Random(_)
            | Completion::Signal(_)
            // Canonical runner payloads and logical run instances contain no
            // host handle words. Ready is the provider's empty acknowledgement.
            | Completion::Runner(_)
            | Completion::Ready
            | Completion::Checked(_)
            | Completion::Entries(_)
            | Completion::Metadata(_)
            | Completion::Addresses(_)
            | Completion::NetBytes(_)
            | Completion::Datagram(_)
            | Completion::Count(_)
            | Completion::NetUnit(_)
            | Completion::Certificates(_)
            | Completion::Protocol(_)
            | Completion::Address(_)
            | Completion::Watch(_)
            | Completion::Unit(_)
            | Completion::Size(_)
            | Completion::Bytes(_)
            | Completion::Wait(_)
            | Completion::Handle(Err(_))
            | Completion::ClockGrant(Err(_))
            | Completion::OptionalFile(None)
            | Completion::NetHandle(Err(_))
            | Completion::Listened(Err(_))
            | Completion::Accepted(Err(_))
            | Completion::Spawned(Err(_))
            | Completion::Halves(Err(_))
            | Completion::Process(_)
            | Completion::Pipeline(_)
            | Completion::ChildUnit(_)
            | Completion::ChildBytes(_)
            | Completion::ChildExit(_)
            | Completion::GroupExit(_),
        ) => Ok([None, None]),
        Ok(Completion::View(view))
            if view.block == 0
                && view
                    .bytes
                    .as_ref()
                    .is_some_and(|bytes| view.length == bytes.len() as u64) =>
        {
            Ok([None, None])
        }
        _ => Err("host result needs an unsupported logical handle codec"),
    }
}
