//! Shared source observation for fresh and resident request entry points.
//! Reuses CaptureInputs/FrozenInputs (TS-Go's read-once snapshot structure).
//! Only input descriptors and source-path watch metadata survive discovery;
//! no compiler, memo, check result or invalidation graph is retained here.
use crate::loader_inputs::{CaptureInputs, FrozenInputs, InputLimits, Operation};
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::PathBuf,
    sync::Arc,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ObservationWork {
    pub file_reads: u64,
    pub file_bytes: u64,
    pub discoveries: u64,
}
impl ObservationWork {
    fn reads(&mut self, inputs: &FrozenInputs) {
        let (reads, bytes) = inputs.read_work();
        self.file_reads += reads;
        self.file_bytes += bytes;
    }
}

pub struct InputObservation {
    pub inputs: FrozenInputs,
    pub work: ObservationWork,
}
struct Current {
    inputs: FrozenInputs,
    sources: BTreeSet<PathBuf>,
    complete: bool,
}
#[derive(Default)]
pub struct InputObserver {
    current: Option<Current>,
}
impl InputObserver {
    pub fn current(&self) -> Option<&FrozenInputs> {
        self.current.as_ref().map(|current| &current.inputs)
    }

    /// Discover the source/loader footprint initially and after namespace or
    /// loader-input changes. Ordinary source-byte edits refresh the immutable
    /// footprint without constructing a loaded compiler program. Every read,
    /// including absence, is rechecked by content before publishing inputs.
    /// Discovery errors retain their observations and are rediscovered next
    /// time; they are never cached as semantic failures or successful products.
    pub fn observe<E>(
        &mut self,
        limits: InputLimits,
        attempts: usize,
        mut discover: impl FnMut(&CaptureInputs) -> Result<Vec<PathBuf>, E>,
    ) -> io::Result<(Result<(), E>, InputObservation)> {
        let mut work = ObservationWork::default();
        for _ in 0..attempts {
            if let Some(current) = self.current.as_ref().filter(|current| current.complete) {
                let inputs = current.inputs.refresh(limits)?;
                work.reads(&inputs);
                let changed = current.inputs.changed_operations(&inputs);
                let source_only = changed.iter().all(|operation| {
                    let Operation::Read(path) = operation else {
                        return false;
                    };
                    current.sources.contains(path)
                        || current
                            .inputs
                            .known_path(path)
                            .is_some_and(|path| current.sources.contains(&path))
                });
                if source_only {
                    let rechecked = inputs.refresh(limits)?;
                    work.reads(&rechecked);
                    if inputs != rechecked {
                        continue;
                    }
                    self.current.as_mut().unwrap().inputs = inputs.clone();
                    return Ok((Ok(()), InputObservation { inputs, work }));
                }
            }
            let capture = CaptureInputs::new(limits);
            work.discoveries += 1;
            let result = discover(&capture);
            let inputs = capture.freeze();
            work.reads(&inputs);
            let rechecked = inputs.refresh(limits)?;
            work.reads(&rechecked);
            if inputs != rechecked {
                continue;
            }
            let complete = result.is_ok();
            let sources = result
                .as_ref()
                .map(|paths| paths.iter().cloned().collect())
                .unwrap_or_default();
            self.current = Some(Current {
                inputs: inputs.clone(),
                sources,
                complete,
            });
            return Ok((result.map(|_| ()), InputObservation { inputs, work }));
        }
        Err(io::Error::other(
            "inputs kept changing during bounded discovery",
        ))
    }

    /// Apply delivered source bytes to the current immutable descriptor, with
    /// the existing alias and allowance validation. No disk is read, and a
    /// failed delivery changes neither this cursor nor already selected graphs.
    pub fn apply_sources(
        &mut self,
        replacements: &BTreeMap<PathBuf, Arc<[u8]>>,
        input_bytes: usize,
    ) -> io::Result<Option<FrozenInputs>> {
        let Some(current) = &mut self.current else {
            return Ok(None);
        };
        if replacements.keys().any(|path| {
            !current.sources.contains(path)
                && !current
                    .inputs
                    .known_path(path)
                    .is_some_and(|path| current.sources.contains(&path))
        }) {
            return Err(io::Error::other(
                "source delivery names an unobserved source",
            ));
        }
        let inputs = current.inputs.replace_sources(replacements, input_bytes)?;
        current.inputs = inputs.clone();
        Ok(Some(inputs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader_inputs::{InputKind, LoaderInputs};
    fn limits() -> InputLimits {
        InputLimits {
            bytes: 65536,
            operations: 100,
        }
    }

    #[test]
    fn source_observation_refreshes_content_but_rediscovers_changed_loader_inputs() {
        let directory =
            std::env::temp_dir().join(format!("hirsel-input-observer-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let source = directory.join("main.hir");
        let sidecar = directory.join("main.hir.json");
        std::fs::write(&source, b"before").unwrap();
        let _ = std::fs::remove_file(&sidecar);
        let discover = |capture: &CaptureInputs| -> io::Result<Vec<PathBuf>> {
            capture.read(&source)?;
            if capture.metadata(&sidecar).is_ok() {
                capture.read(&sidecar)?;
            }
            Ok(vec![source.clone()])
        };
        let mut observer = InputObserver::default();
        let (result, initial) = observer.observe(limits(), 2, discover).unwrap();
        result.unwrap();
        assert_eq!(initial.work.discoveries, 1);
        std::fs::write(&source, b"after!").unwrap();
        let (_, edited) = observer
            .observe(limits(), 2, |_| -> io::Result<Vec<PathBuf>> {
                panic!("a source edit must not rediscover the package graph")
            })
            .unwrap();
        assert_eq!(edited.work.discoveries, 0);
        assert_eq!(edited.work.file_reads, 2);
        assert_eq!(edited.work.file_bytes, 12);
        assert_eq!(&*initial.inputs.read(&source).unwrap(), b"before");
        assert_eq!(&*edited.inputs.read(&source).unwrap(), b"after!");
        std::fs::write(&sidecar, b"{}").unwrap();
        let (_, changed_loader) = observer.observe(limits(), 2, discover).unwrap();
        assert_eq!(changed_loader.work.discoveries, 1);
        assert_eq!(
            changed_loader.inputs.metadata(&sidecar).unwrap(),
            InputKind::File
        );
        assert_eq!(&*changed_loader.inputs.read(&sidecar).unwrap(), b"{}");
        let delivered = [(source.clone(), Arc::<[u8]>::from(b"delivered".as_slice()))]
            .into_iter()
            .collect();
        let explicit = observer.apply_sources(&delivered, 65536).unwrap().unwrap();
        assert_eq!(&*explicit.read(&source).unwrap(), b"delivered");
        assert!(observer.apply_sources(&delivered, 1).is_err());
        assert_eq!(observer.current().unwrap(), &explicit);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn failed_discovery_keeps_watched_absence_and_bounds_refuse_without_replacing_inputs() {
        let directory = std::env::temp_dir().join(format!(
            "hirsel-input-observer-missing-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let missing = directory.join("missing.hir");
        let _ = std::fs::remove_file(&missing);
        let mut observer = InputObserver::default();
        let (result, observed) = observer
            .observe(limits(), 1, |capture| -> io::Result<Vec<PathBuf>> {
                capture.read(&missing)?;
                Ok(vec![missing.clone()])
            })
            .unwrap();
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::NotFound);
        assert!(
            observed
                .inputs
                .operations()
                .any(|operation| operation.path() == missing)
        );
        let prior = observed.inputs.clone();
        let error = observer.observe(
            InputLimits {
                bytes: 0,
                operations: 0,
            },
            0,
            |_| -> io::Result<Vec<PathBuf>> { panic!("zero attempts cannot discover") },
        );
        assert!(error.is_err());
        assert_eq!(observer.current().unwrap(), &prior);
        std::fs::write(&missing, b"repaired").unwrap();
        let (result, repaired) = observer
            .observe(limits(), 1, |capture| -> io::Result<Vec<PathBuf>> {
                capture.read(&missing)?;
                Ok(vec![missing.clone()])
            })
            .unwrap();
        result.unwrap();
        assert_eq!(&*repaired.inputs.read(&missing).unwrap(), b"repaired");
        let prior = repaired.inputs.clone();
        assert!(
            observer
                .observe(
                    InputLimits {
                        bytes: 0,
                        operations: 0
                    },
                    1,
                    |_| -> io::Result<Vec<PathBuf>> { panic!("bounded refresh must refuse first") }
                )
                .is_err()
        );
        assert_eq!(observer.current().unwrap(), &prior);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
