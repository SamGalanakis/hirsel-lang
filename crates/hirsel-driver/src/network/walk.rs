//! A walk pull can cross several empty directories. Keep its advanced cursor
//! across each typed Free host phase, without restarting the effectful prefix.
use super::*;
use hirsel_rt::free_replay::{Call, Poll};
impl HostLibrary {
    pub(super) fn walk_next(
        &mut self,
        call: OperationCall<'_>,
        cx: &mut OperationContext<'_>,
        mut supplied: Option<(R, Result<C, hirsel_rt::io::AdapterError>)>,
    ) -> Result<OperationResult, Error> {
        let mut it = call.arguments[0].clone();
        loop {
            let mut pending = None;
            let walked =
                hirsel_interp::library::walk::pull(&mut it, |directory, path, after, recursive| {
                    let directory = self.id(directory)?;
                    let request = R::ReadDirPage {
                        directory,
                        path: path.into(),
                        after: after.into(),
                        recursive,
                    };
                    let completion = if let Some((expected, completion)) = supplied.take() {
                        if request != expected {
                            return Err(invalid());
                        }
                        completion.map_err(|e| Error::Unavailable(format!("Free walk: {e:?}")))?
                    } else if self.free.is_some() {
                        pending = Some(request);
                        return Err(Error::Blocked(vec![]));
                    } else {
                        self.yield_now = true;
                        self.host(request)?
                    };
                    match completion {
                        C::Entries(result) => Ok(result
                            .map(|entries| {
                                entries
                                    .into_iter()
                                    .map(|e| hirsel_interp::library::walk::Entry {
                                        path: e.path,
                                        kind: e.kind as u32,
                                    })
                                    .collect()
                            })
                            .map_err(file_error)),
                        _ => Err(invalid()),
                    }
                });
            if let Some(request) = pending {
                if !self.library.begin_native_host()? {
                    return Ok(self.library.native_free_cut(cx));
                }
                let mut phase =
                    Call::new(self.free.as_ref().unwrap(), request).map_err(Error::InvalidTir)?;
                match self.free_advance(&mut phase)? {
                    Poll::Complete(completion) => {
                        supplied = Some((phase.request().clone(), completion));
                    }
                    Poll::Cut => return Ok(self.library.native_free_cut(cx)),
                    Poll::Pending => {
                        call.arguments[0] = it;
                        return self.park_walk(phase, call, cx);
                    }
                }
                continue;
            }
            let walked = walked?;
            if supplied.is_some() {
                return Err(invalid());
            }
            let result = self.library.next_iterator(it, walked)?;
            if self.free.is_some() {
                let path = self.library.task_path();
                if self
                    .library
                    .with_free_session(|session| Ok(session.at_cut(&path)))?
                {
                    return Ok(self.library.native_free_cut(cx));
                }
            }
            return Ok(result);
        }
    }
}
