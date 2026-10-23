//! Bounded deterministic snapshot file inputs, never host errno injection.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

/// Operation keys use the public core-library spelling in serialized maps.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub enum FileOperation {
    #[serde(rename = "file.open")]
    Open,
    #[serde(rename = "file.create")]
    Create,
    #[serde(rename = "file.open_append")]
    OpenAppend,
    #[serde(rename = "file.read")]
    Read,
    #[serde(rename = "file.read_bytes")]
    ReadBytes,
    #[serde(rename = "file.write")]
    Write,
    #[serde(rename = "file.append")]
    Append,
    #[serde(rename = "file.write_bytes")]
    WriteBytes,
    #[serde(rename = "file.create_atomic")]
    CreateAtomic,
    #[serde(rename = "file.write_all")]
    WriteAll,
    #[serde(rename = "file.commit")]
    Commit,
    #[serde(rename = "file.sync")]
    Sync,
    #[serde(rename = "file.put_new")]
    PutNew,
}
impl FileOperation {
    pub fn name(self) -> &'static str {
        match self {
            Self::Open => "file.open",
            Self::Create => "file.create",
            Self::OpenAppend => "file.open_append",
            Self::Read => "file.read",
            Self::ReadBytes => "file.read_bytes",
            Self::Write => "file.write",
            Self::Append => "file.append",
            Self::WriteBytes => "file.write_bytes",
            Self::CreateAtomic => "file.create_atomic",
            Self::WriteAll => "file.write_all",
            Self::Commit => "file.commit",
            Self::Sync => "file.sync",
            Self::PutNew => "file.put_new",
        }
    }
    pub fn bytes(self) -> bool {
        matches!(
            self,
            Self::Read
                | Self::Write
                | Self::Append
                | Self::WriteBytes
                | Self::WriteAll
                | Self::PutNew
        )
    }
    fn partial(self) -> bool {
        matches!(
            self,
            Self::Write | Self::Append | Self::WriteBytes | Self::WriteAll
        )
    }
}

/// FileError data; cancellation carries the core CancelCause ordinal (0..=3).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum FileFaultError {
    Fault(u32),
    Canceled(u32),
    NotFound,
    PermissionDenied,
    AlreadyExists,
    IsDirectory,
    NotDirectory,
    DirectoryNotEmpty,
    CrossDevice,
}
/// One admitted operation. None is an ordinary provider call, useful before a later fault.
/// Byte operations require the exact request count (read max or supplied byte length).
/// Reads never deliver bytes on error. Only write operations support a partial prefix.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileFault {
    pub requested: Option<u64>,
    pub error: Option<FileFaultError>,
    pub partial: u64,
}
pub type FileFaults = BTreeMap<FileOperation, VecDeque<FileFault>>;

/// Validate the complete script before granting the app any capabilities.
pub fn validate_file_faults(scripts: &FileFaults) -> Result<(), String> {
    for (op, queue) in scripts {
        if queue.is_empty() {
            return Err(format!("{}: empty file fault queue", op.name()));
        }
        for step in queue {
            if op.bytes() != step.requested.is_some() {
                return Err(format!(
                    "{}: expected request count exactly for byte operations",
                    op.name()
                ));
            }
            if matches!(step.error, Some(FileFaultError::Canceled(cause)) if cause > 3) {
                return Err(format!("{}: invalid cancellation cause", op.name()));
            }
            if usize::try_from(step.partial).is_err()
                || step.partial > step.requested.unwrap_or(0)
                || (step.partial != 0
                    && (!op.partial()
                        || step.error.is_none()
                        || matches!(step.error, Some(FileFaultError::Canceled(_)))))
            {
                return Err(format!("{}: unsupported partial file fault", op.name()));
            }
        }
    }
    Ok(())
}

/// Check the exact request before consuming an entry. Parked and denied calls do not enter here.
pub fn take_file_fault(
    scripts: &mut FileFaults,
    op: FileOperation,
    requested: Option<u64>,
) -> Result<Option<FileFault>, String> {
    let Some(queue) = scripts.get_mut(&op) else {
        return Ok(None);
    };
    if let Some(step) = queue.front() {
        if step.requested != requested {
            return Err(format!(
                "{}: file fault request count mismatch: expected {:?}, got {:?}",
                op.name(),
                step.requested,
                requested
            ));
        }
    }
    Ok(queue.pop_front())
}
