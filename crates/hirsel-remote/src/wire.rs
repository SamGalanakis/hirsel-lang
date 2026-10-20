//! The wire form of frames: a little-endian `u64` length, then one canonical codec
//! envelope of core `RunFrame`, compiled from the registry's own declaration. The same
//! codec gives a request digest's grant set and allowlist bytes, so a host recomputes the
//! digest its caller computed (RUN-11).
use crate::data::{DataCodec, DataError};
use crate::frame::{Call, Frame, Malformed, allowlist_data, grant_set_data};
use crate::host::Digests;
use hirsel_types::serialization::SerializableSchema;
use hirsel_types::wire::Limits;
use std::io::{self, Read, Write};

/// The largest frame either side reads by default: the codec's own byte budget.
pub const DEFAULT_FRAME_BYTES: u64 = 64 * 1024 * 1024;

/// Why bytes are not a frame.
#[derive(Debug)]
pub enum WireError {
    Io(io::Error),
    /// The length prefix exceeds the reader's budget.
    TooLarge(u64),
    Codec(DataError),
    Malformed(Malformed),
}
impl From<io::Error> for WireError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// `RunFrame`'s codec and the request digest's parts.
pub struct Wire {
    frame: DataCodec,
    grants: DataCodec,
    allowlist: DataCodec,
    limit: u64,
}
impl Wire {
    /// The codecs of core `RunFrame`, `GrantSet` and `Option[List[String]]`.
    pub fn new(
        frame: &SerializableSchema,
        grants: &SerializableSchema,
        allowlist: &SerializableSchema,
        limit: u64,
    ) -> Result<Self, DataError> {
        Ok(Self {
            frame: DataCodec::new(frame)?,
            grants: DataCodec::new(grants)?,
            allowlist: DataCodec::new(allowlist)?,
            limit,
        })
    }
    /// The largest frame this side reads.
    pub fn limit(&self) -> u64 {
        self.limit
    }
    fn limits(&self) -> Limits {
        Limits {
            bytes: usize::try_from(self.limit).unwrap_or(usize::MAX),
            ..Limits::default()
        }
    }
    /// The frame's codec envelope.
    pub fn encode(&self, frame: &Frame) -> Result<Vec<u8>, WireError> {
        self.frame
            .encode(&frame.data(), self.limits())
            .map_err(WireError::Codec)
    }
    pub fn decode(&self, bytes: &[u8]) -> Result<Frame, WireError> {
        let data = self
            .frame
            .decode(bytes, self.limits())
            .map_err(WireError::Codec)?;
        Frame::from_data(data).map_err(WireError::Malformed)
    }
    /// Write one length-prefixed frame.
    pub fn write(&self, out: &mut dyn Write, frame: &Frame) -> Result<(), WireError> {
        let bytes = self.encode(frame)?;
        out.write_all(&(bytes.len() as u64).to_le_bytes())?;
        out.write_all(&bytes)?;
        out.flush()?;
        Ok(())
    }
    /// Read one length-prefixed frame; None at a clean end of stream before a frame.
    pub fn read(&self, input: &mut dyn Read) -> Result<Option<Frame>, WireError> {
        let mut len = [0u8; 8];
        let mut filled = 0;
        while filled < 8 {
            match input.read(&mut len[filled..])? {
                0 if filled == 0 => return Ok(None),
                0 => return Err(WireError::Io(io::ErrorKind::UnexpectedEof.into())),
                n => filled += n,
            }
        }
        let len = u64::from_le_bytes(len);
        if len > self.limit {
            return Err(WireError::TooLarge(len));
        }
        let mut bytes = vec![0; len as usize];
        input.read_exact(&mut bytes)?;
        self.decode(&bytes).map(Some)
    }
}
impl Digests for Wire {
    fn digest(&self, call: &Call) -> [u8; 32] {
        let grants = self
            .grants
            .encode(&grant_set_data(&call.grants), Limits::default())
            .unwrap_or_default();
        let allowlist = self
            .allowlist
            .encode(
                &allowlist_data(call.definitions.as_deref()),
                Limits::default(),
            )
            .unwrap_or_default();
        hirsel_run_schema::request_digest(
            call.form,
            &call.path,
            &call.fingerprint,
            &call.code,
            &grants,
            &allowlist,
            &call.argument,
        )
    }
}
