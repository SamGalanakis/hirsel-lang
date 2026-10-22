//! Explicit admission for nonparking ConcurrentAdapter calls. General native
//! routing, readiness and terminal cuts remain gated. io_keys owns live alias
//! discovery; event projection receives only an owned discovery snapshot.
use super::*;
use crate::io::{AdapterError, Completion, Request};
use crate::io_concurrent::ConcurrentAdapter;
use std::collections::BTreeMap;

type Discovery = BTreeMap<u64, (crate::io_keys::Key, crate::io_keys::Kind)>;
type Discovered = Result<Discovery, &'static str>;

/// Request/result storage outlives a saved Rust continuation. Abandoning its
/// fiber drops this owner with TaskCtx, rather than leaking a stack local.
pub(crate) struct Pending {
    request: Option<Request>,
    result: Option<Result<Completion, AdapterError>>,
}
struct Owner(*mut TaskCtx);
impl Drop for Owner {
    fn drop(&mut self) {
        unsafe {
            (*self.0).pending_ordered_host.take();
        }
    }
}

/// # Safety
/// The running Ordered task owns its role, request and adapter lifetime.
/// This admission excludes requests which may register readiness waits.
pub(crate) unsafe fn request(
    ctx: *mut TaskCtx,
    adapter: &dyn ConcurrentAdapter,
    request: Request,
) -> Stopped<Result<Completion, AdapterError>> {
    if crate::host_waits::may_park(&request) {
        return Err(Stop::Refused(
            "Ordered concurrent readiness is not admitted".into(),
        ));
    }
    let close = matches!(request, Request::Close { .. });
    assert!(unsafe { (*ctx).pending_ordered_host.is_none() });
    unsafe {
        (*ctx).pending_ordered_host = Some(Box::new(Pending {
            request: Some(request),
            result: None,
        }));
    }
    let _owner = Owner(ctx);
    super::begin(ctx, close)?;
    let request = unsafe {
        (*ctx)
            .pending_ordered_host
            .as_ref()
            .unwrap()
            .request
            .as_ref()
            .unwrap() as *const Request
    };
    let driver = engine::driver(ctx);
    let token = driver.next_host_token;
    driver.next_host_token = token.checked_add(1).expect("host request token overflow");
    let observing = driver.events.is_some();
    let cause = unsafe { crate::tasks::canceled(ctx) };
    super::super::events::request_host(ctx, unsafe { &*request });
    let keys = if observing {
        let _segment = unsafe { role::Segment::new(ctx) };
        Some(adapter.keys(unsafe { &*request }))
    } else {
        None
    };
    if let Some(keys) = keys {
        super::super::events::request_keys(ctx, &keys);
    }
    let (result, discovered) = {
        let _segment = unsafe { role::Segment::new(ctx) };
        invoke(
            adapter,
            token,
            cause,
            unsafe {
                (*ctx)
                    .pending_ordered_host
                    .as_mut()
                    .unwrap()
                    .request
                    .take()
                    .unwrap()
            },
            observing,
        )
    };
    // Do not observe a stop between the bounded effect and owned publication.
    // An unsupported discovery refuses admission after preserving raw evidence;
    // it must never certify an incomplete canonical result.
    let discovery = discovered
        .and_then(|discovered| super::super::events::discover_host(ctx, &result, &discovered));
    super::super::events::complete_host(ctx, close, &result);
    discovery.map_err(|error| Stop::Refused(error.into()))?;
    if matches!(result, Err(AdapterError::Blocked)) {
        return Err(Stop::Refused(
            "nonparking concurrent request blocked".into(),
        ));
    }
    unsafe {
        (*ctx).pending_ordered_host.as_mut().unwrap().result = Some(result);
    }
    super::finish(ctx, if close { Next::Keep } else { Next::Yield })?;
    Ok(unsafe {
        (*ctx)
            .pending_ordered_host
            .as_mut()
            .unwrap()
            .result
            .take()
            .unwrap()
    })
}

fn invoke(
    adapter: &dyn ConcurrentAdapter,
    token: u64,
    cause: Option<u32>,
    request: Request,
    observing: bool,
) -> (Result<Completion, AdapterError>, Discovered) {
    let mut result = if let Some(cause) = cause {
        match crate::io_checks::cancel_concurrent(adapter, token, &request, cause) {
            Ok(Some(completion)) => Ok(completion),
            Ok(None) => adapter.call_at(token, request),
            Err(error) => Err(error),
        }
    } else {
        adapter.call_at(token, request)
    };
    let discovered = if observing {
        discover(&mut result, adapter)
    } else {
        Ok(BTreeMap::new())
    };
    adapter.finish(token);
    (result, discovered)
}

fn discover(
    result: &mut Result<Completion, AdapterError>,
    adapter: &dyn ConcurrentAdapter,
) -> Discovered {
    // Invalid-handle errors create nothing; complete_host projects only a
    // previously admitted identity. Unknown words remain unexportable.
    if matches!(result, Err(AdapterError::InvalidHandle(_))) {
        return Ok(BTreeMap::new());
    }
    crate::io_observation::capture(result).map_err(|_| "invalid host mapping")?;
    crate::io_observation::fields(result)?
        .into_iter()
        .flatten()
        .map(|word| {
            adapter
                .identity(*word)
                .map(|identity| (*word, identity))
                .ok_or("created host handle has no discovered resource")
        })
        .collect()
}

#[cfg(test)]
mod tests;
