//! Exact externally written prefixes supplied by the adapter's bounded call.
//! A typed write error does not imply either zero or full output. Validation
//! follows Free host::checked_output and RealIo's counted write/flush path.
use super::*;
use crate::io::{AdapterError, Completion, IoError};

pub(in crate::tasks::ordered) fn project(
    bytes: Option<&[u8]>,
    result: &Result<Completion, AdapterError>,
    count: Option<u64>,
) -> Result<Option<Vec<u8>>, &'static str> {
    match (bytes, count) {
        (None, None) => Ok(None),
        (Some(bytes), Some(count)) if count <= bytes.len() as u64 => {
            let valid = match result {
                Ok(Completion::Unit(Ok(()))) => count == bytes.len() as u64,
                Ok(Completion::Unit(Err(IoError::Canceled(_) | IoError::NotGranted))) => count == 0,
                Ok(Completion::Unit(Err(_))) => true,
                _ => false,
            };
            if !valid {
                return Err("output count differs from completion");
            }
            Ok(Some(bytes[..count as usize].to_vec()))
        }
        _ => Err("missing or invalid output prefix"),
    }
}

/// Capture before the post-effect continuation. The map owns bytes if the
/// issuer's saved frame is abandoned, and failed evidence stays unexportable.
pub(in crate::tasks::ordered) fn record(
    ctx: *mut TaskCtx,
    output: Result<Option<Vec<u8>>, &'static str>,
) {
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
    if !enabled(s) {
        return;
    }
    let path = s.task(unsafe { (*ctx).task_id }).path.clone();
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let log = driver.events.as_mut().unwrap();
    let ordinal = log.ordinals[&path]
        .checked_sub(1)
        .expect("output follows host entry");
    assert!(log.host_outputs.insert((path, ordinal), output).is_none());
}

#[cfg(test)]
mod tests;
