//! Live resource discovery for every keyed recorder. The home adapter and the
//! concurrent service find identities with this one walk, so a run records the
//! same footprints whichever of them performs its requests.
use super::*;
use crate::io_keys::{Key, Kind, Registry};

/// The admitted storage and grants, discovered before the first request, even
/// before a language grant projects them. Unknown writers may alias any filesystem.
/// The registry logs identities for the footprint of each recorded request.
pub(super) fn admit<S: Streams>(core: &RealCore<S>) -> Registry {
    let mut registry = Registry::default();
    registry.record();
    let [output, stderr, input] = core.streams.storages();
    registry.sink(u64::MAX - 1, output);
    registry.sink(u64::MAX - 2, stderr);
    registry.sink(u64::MAX - 3, input);
    for reserved in [u64::MAX - 1, u64::MAX - 2, u64::MAX - 3] {
        registry.forget(reserved);
    }
    for (handle, grant) in core.state.handles.rows.sorted() {
        discover(core, &mut registry, handle, grant);
    }
    registry
}

/// The request words a call's publication reads afterwards: the call consumes
/// the request.
#[derive(Clone, Copy)]
pub(super) struct Publication {
    published: Published,
    tls_split: bool,
    first_new: u64,
}

#[derive(Clone, Copy)]
enum Published {
    Split(u64),
    Alias(u64),
    Join(u64),
    Other,
}

impl Publication {
    pub(super) fn of<S: Streams>(core: &RealCore<S>, request: &Request) -> Self {
        Self {
            published: match request {
                Request::NetSplit { stream, .. } => Published::Split(*stream),
                Request::NarrowFile { file, .. }
                | Request::ReadOnlyDir { directory: file }
                | Request::NarrowDir {
                    directory: file, ..
                } => Published::Alias(*file),
                Request::NetJoin { read, .. } => Published::Join(*read),
                _ => Published::Other,
            },
            tls_split: matches!(request, Request::NetSplit { stream, .. }
                if matches!(core.state.handles.rows.get(stream), Some(crate::handles::Grant::Stream { tls: true, .. }))),
            first_new: core.state.handles.next(),
        }
    }
    /// Register the identities the call's result established. Closed physical
    /// words never get reused, so resource metadata outlives a consuming result.
    pub(super) fn publish<S: Streams>(
        self,
        core: &RealCore<S>,
        registry: &mut Registry,
        result: &Result<Completion, AdapterError>,
    ) {
        if let Published::Split(stream) = self.published
            && let Ok(Completion::Halves(Ok((read, write)))) = result
        {
            if self.tls_split {
                let (parent, _) = registry.identity(stream).expect("registered TLS stream");
                registry.bind(*read, parent, Kind::Other);
                registry.bind(*write, parent, Kind::Other);
            } else {
                registry.split(stream, *read, *write);
            }
        }
        if let Published::Alias(file) = self.published
            && let Ok(Completion::OptionalFile(Some(to)) | Completion::Handle(Ok(to))) = result
        {
            registry.alias(file, *to);
        }
        if let Published::Join(read) = self.published
            && let Some((key, kind)) = registry.identity(read)
            && let Ok(Completion::NetHandle(Ok(joined))) = result
        {
            let parent = match kind {
                Kind::Half { stream } => stream,
                _ => key,
            };
            registry.bind(*joined, parent, Kind::Other);
        }
        for handle in self.first_new..core.state.handles.next() {
            if registry.identity(handle).is_none()
                && let Some(grant) = core.state.handles.rows.get(&handle)
            {
                discover(core, registry, handle, grant);
            }
        }
    }
}

/// A request's auxiliary domains, named by a checked grant's resource.
pub(super) fn domains<S: Streams>(
    core: &RealCore<S>,
    registry: &mut Registry,
    request: &Request,
) -> Vec<(Key, u64, String)> {
    use Request as R;
    match request {
        R::Connect { net, .. }
        | R::ConnectAddr { net, .. }
        | R::Listen { net, .. }
        | R::UdpBind { net, .. }
        | R::UnixConnect { net, .. }
        | R::UnixListen { net, .. }
            if matches!(
                core.state.handles.rows.get(net),
                Some(crate::handles::Grant::Net { .. })
            ) =>
        {
            registry
                .net_ports(*net)
                .map(|key| vec![(key, *net, "ports".into())])
                .unwrap_or_default()
        }
        R::ProcessRun { process, .. }
        | R::ProcessSpawn { process, .. }
        | R::ProcessPipeline { process, .. }
            if matches!(
                core.state.handles.rows.get(process),
                Some(crate::handles::Grant::Process { .. })
            ) =>
        {
            registry
                .inherited_domains()
                .into_iter()
                .enumerate()
                .map(|(index, key)| (key, *process, format!("inherited-{index}")))
                .collect()
        }
        _ => Vec::new(),
    }
}

fn discover<S: Streams>(
    core: &RealCore<S>,
    registry: &mut Registry,
    handle: u64,
    grant: &crate::handles::Grant,
) {
    use crate::handles::Grant as G;
    // Repeated and narrowed grants of the same provider share its draw/observation
    // sequence. In particular random's one generator must not get independent keys.
    let alias = core
        .state
        .handles
        .rows
        .sorted()
        .into_iter()
        .find(|(other, candidate)| {
            *other != handle
                && registry.identity(*other).is_some()
                && match (grant, *candidate) {
                    (G::Random, G::Random)
                    | (G::Stdin, G::Stdin)
                    | (G::MonotonicClock, G::MonotonicClock)
                    | (G::WallClock, G::WallClock)
                    | (G::Signals, G::Signals)
                    | (G::Telemetry, G::Telemetry) => true,
                    (G::Net { world: a, .. }, G::Net { world: b, .. }) => a == b,
                    (G::Process { world: a, .. }, G::Process { world: b, .. }) => a == b,
                    _ => false,
                }
        })
        .map(|(other, _)| other);
    if let Some(other) = alias {
        registry.alias(other, handle);
        return;
    }
    match grant {
        G::Stdout => {
            registry.sink(handle, core.streams.storages()[0]);
        }
        G::Stderr => {
            registry.sink(handle, core.streams.storages()[1]);
        }
        G::Stdin => {
            registry.sink(handle, core.streams.storages()[2]);
        }
        G::File { .. } | G::AtomicFile { .. } | G::Mapped { .. } => {
            let storage = match grant {
                G::File { .. } => core
                    .disk
                    .files
                    .get(&handle)
                    .map(|file| crate::io_keys::Storage::of_descriptor(file.as_raw_fd())),
                G::AtomicFile {
                    temporary,
                    directory,
                    ..
                } => core
                    .disk
                    .prepared
                    .get(&(*directory, temporary.clone()))
                    .map(|file| crate::io_keys::Storage::of_descriptor(file.as_raw_fd())),
                G::Mapped { .. } => core
                    .disk
                    .maps
                    .get(&handle)
                    .map(crate::mapping::Mapping::storage),
                _ => unreachable!(),
            };
            match storage {
                Some(crate::io_keys::Storage::Descriptor { dev, ino, .. }) => {
                    registry.inode(handle, Kind::File, dev, ino);
                }
                // Scripted files have no host descriptor and cannot alias the host filesystem.
                None => {
                    registry.resource(handle, Kind::File);
                }
                _ => {
                    registry.sink(handle, crate::io_keys::Storage::Unknown);
                }
            }
        }
        G::Watcher { .. } | G::Directory { .. } => {
            registry.resource(handle, Kind::File);
        }
        G::Span { telemetry, .. } => registry.alias(*telemetry, handle),
        G::Half { .. } => {} // registered with their parent before result publication
        _ => {
            registry.resource(handle, Kind::Other);
        }
    }
}
