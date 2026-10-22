//! Structured serial candidates on the existing resident socket. Framing is
//! serve.rs's bounded split-frame JSON, without the chg descriptor adapter.
use super::*;
use hirsel_driver::candidate::{
    CandidateWorkspace, Limits,
    protocol::{self, Connection, FRAME_BYTES},
};
use serde_json::{Value, json};

pub(super) const MAGIC: &[u8; 4] = b"HRC1";
const INPUT_IDLE: Duration = Duration::from_secs(30);

pub(super) fn serve(
    mut stream: UnixStream,
    first: Value,
    fds: Vec<OwnedFd>,
    ours: &[(String, String)],
    root: &Path,
) -> Result<(), ()> {
    // Candidate connections never attach descriptors or mutate process state.
    // The legacy busy flag excludes global command activity for their lifetime.
    let environment = first["env"].as_array().and_then(|a| {
        a.iter()
            .map(|pair| Some((pair[0].as_str()?.to_owned(), pair[1].as_str()?.to_owned())))
            .collect::<Option<Vec<_>>>()
    });
    if !fds.is_empty()
        || environment
            .as_deref()
            .map(compatible_environment)
            .as_deref()
            != Some(ours)
    {
        let _ = stream.write_all(b"R");
        return Ok(());
    }
    let _ = stream.set_read_timeout(Some(INPUT_IDLE));
    let _ = stream.set_write_timeout(Some(INPUT_IDLE));
    let mut config = hirsel_engine::CompileConfig::default();
    // One active candidate consumes the entire existing per-definition budget.
    // NoneStore bounds store retention; no unbounded shared MemStore is added.
    config.store = std::sync::Arc::new(hirsel_engine::store::NoneStore);
    let images = std::env::var_os("HIRSEL_IMAGE")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .map(|p| p.with_file_name("hirsel-std.image"))
        })
        .and_then(|path| hirsel_engine::StdImage::open(&path).ok())
        .into_iter()
        .collect();
    let workspace = CandidateWorkspace::new(config, images, Default::default(), Limits::default())
        .map_err(|_| ())?;
    let mut connection = Connection::new(workspace, root.to_owned()).map_err(|_| ())?;
    let mut value = first;
    loop {
        let reply =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| connection.handle(&value)));
        let failed = reply.is_err();
        let reply =
            reply.unwrap_or_else(|_| json!({"version":connection.version(),"error":"ToolFailure"}));
        if write_frame(&mut stream, &reply).is_err() {
            return Ok(());
        }
        if failed {
            return Err(());
        }
        // The request cap refuses new admission while preserving live handles.
        // Idle/disconnect drains them; the host then retires its global domain.
        let Some((magic, next, fds)) = read_wire(&stream) else {
            return Ok(());
        };
        if &magic != MAGIC || !fds.is_empty() {
            let _ = write_frame(
                &mut stream,
                &json!({"version":connection.version(),"error":"InvalidFrame"}),
            );
            return Ok(());
        }
        value = next;
    }
}

struct Frame(Vec<u8>);
impl Write for Frame {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.0.len().saturating_add(bytes.len()) > FRAME_BYTES {
            return Err(std::io::Error::other("candidate reply exceeds frame limit"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn write_frame(stream: &mut UnixStream, value: &Value) -> std::io::Result<()> {
    let mut frame = Frame(Vec::new());
    if serde_json::to_writer(&mut frame, value).is_err() {
        frame.0.clear();
        serde_json::to_writer(
            &mut frame,
            &json!({"version":value["version"].as_u64().unwrap_or(1),"error":"FrameLimit"}),
        )?;
    }
    stream.write_all(MAGIC)?;
    stream.write_all(&(frame.0.len() as u32).to_le_bytes())?;
    stream.write_all(&frame.0)
}
fn read_reply(stream: &mut UnixStream) -> std::io::Result<Value> {
    let mut header = [0; 8];
    stream.read_exact(&mut header[..1])?;
    if matches!(header[0], b'B' | b'R') {
        return Err(std::io::Error::other(
            "candidate server busy or incompatible",
        ));
    }
    stream.read_exact(&mut header[1..])?;
    if &header[..4] != MAGIC {
        return Err(std::io::Error::other("invalid candidate reply"));
    }
    let length = u32::from_le_bytes(header[4..].try_into().unwrap()) as usize;
    if length > FRAME_BYTES {
        return Err(std::io::Error::other("candidate reply exceeds frame limit"));
    }
    let mut body = vec![0; length];
    stream.read_exact(&mut body)?;
    serde_json::from_slice(&body).map_err(std::io::Error::other)
}

/// `hirsel candidate --workspace DIR`: NDJSON stdin/replies, one connection.
/// Transport errors are terminal; there is no disk-check fallback.
pub(super) fn client(args: &[OsString]) -> u8 {
    if args.len() != 2 || args[0] != "--workspace" {
        eprintln!("usage: hirsel candidate --workspace <dir>  # versioned JSON lines on stdin");
        return 2;
    }
    match client_inner(Path::new(&args[1])) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("hirsel: candidate: {e}");
            3
        }
    }
}
fn client_inner(root: &Path) -> std::io::Result<()> {
    use std::io::BufRead;
    let root = workspace(root);
    let path = socket(&root);
    let mut stream = UnixStream::connect(&path)
        .ok()
        .or_else(|| start(&path, &root))
        .ok_or_else(|| std::io::Error::other("candidate server unavailable"))?;
    let env = environment().ok_or_else(|| std::io::Error::other("non-UTF8 environment"))?;
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    loop {
        // Bound before allocating a whole line, including malformed JSON.
        let mut line = Vec::new();
        let n = Read::by_ref(&mut input)
            .take(FRAME_BYTES as u64 + 1)
            .read_until(b'\n', &mut line)?;
        if n == 0 {
            return Ok(());
        }
        if n > FRAME_BYTES {
            return Err(std::io::Error::other(
                "candidate request exceeds frame limit",
            ));
        }
        let mut request: Value =
            protocol::decode_wire_frame(&line).map_err(std::io::Error::other)?;
        if !request.is_object() {
            return Err(std::io::Error::other("candidate request must be an object"));
        }
        request["identity"] = identity().into();
        request["env"] = json!(env);
        // Reject oversized requests locally; never send FrameLimit as an op.
        let mut frame = Frame(Vec::new());
        serde_json::to_writer(&mut frame, &request)?;
        stream.write_all(MAGIC)?;
        stream.write_all(&(frame.0.len() as u32).to_le_bytes())?;
        stream.write_all(&frame.0)?;
        let reply = read_reply(&mut stream)?;
        serde_json::to_writer(&mut output, &reply)?;
        output.write_all(b"\n")?;
        output.flush()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn candidate_frames_split_and_limit_before_allocation() {
        let (mut sender, receiver) = UnixStream::pair().unwrap();
        let value = json!({"identity":identity(),"version":1,"op":"counters"});
        let body = value.to_string();
        sender.write_all(&MAGIC[..2]).unwrap();
        sender.write_all(&MAGIC[2..]).unwrap();
        sender
            .write_all(&(body.len() as u32).to_le_bytes())
            .unwrap();
        sender.write_all(body.as_bytes()).unwrap();
        let (magic, received, fds) = read_wire(&receiver).unwrap();
        assert_eq!(&magic, MAGIC);
        assert_eq!(received, value);
        assert!(fds.is_empty());
        sender.write_all(MAGIC).unwrap();
        sender.write_all(&u32::MAX.to_le_bytes()).unwrap();
        assert!(read_wire(&receiver).is_none());
    }
    #[test]
    fn candidate_reply_roundtrip_and_busy_is_error() {
        let (mut sender, mut receiver) = UnixStream::pair().unwrap();
        write_frame(&mut sender, &json!({"version":1,"ok":{"pending":true}})).unwrap();
        assert_eq!(read_reply(&mut receiver).unwrap()["ok"]["pending"], true);
        sender.write_all(b"B").unwrap();
        assert!(read_reply(&mut receiver).is_err());
        let mut frame = Frame(vec![0; FRAME_BYTES]);
        assert!(frame.write_all(b"x").is_err());
    }
    #[test]
    fn candidate_descriptors_are_refused_without_attachment() {
        let (mut sender, receiver) = UnixStream::pair().unwrap();
        let file = std::fs::File::open("/dev/null").unwrap();
        serve(
            receiver,
            json!({"env":environment().unwrap()}),
            vec![file.into()],
            &compatible_environment(&environment().unwrap()),
            Path::new("/"),
        )
        .unwrap();
        let mut status = [0];
        sender.read_exact(&mut status).unwrap();
        assert_eq!(status, b"R"[..]);
    }
}
