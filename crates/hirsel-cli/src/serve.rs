//! The resident compiler (docs/building.md, "The resident compiler"):
//! `hirsel serve` keeps compile sessions for one workspace behind a Unix
//! socket, and `check`, `build` and `run` hand their command to it when it is
//! up, so an edit re-checks only what it changed.
//!
//! The client passes its standard input, output and error to the server over
//! the socket (`SCM_RIGHTS`, as Mercurial's chg command server attaches its
//! client's stdio: contrib/chg/hgclient.c, `attachio`), with its arguments,
//! working directory and environment. The server runs the command exactly as
//! the one-shot CLI does, on those descriptors, and answers with its exit
//! code; so every byte a command writes is the byte a fresh process writes.
//! A server refuses (and the client runs the command itself) a client built
//! from other compiler sources, or whose environment differs from
//! the server's own, since the compiler reads some of it once per process.
//!
//! The socket is `<runtime dir>/hirsel/<identity>-<workspace>.sock`. The
//! workspace is the nearest ancestor of the entry holding `.git`, else the
//! entry's directory. A client starts the workspace's server when none runs
//! (`HIRSEL_DAEMON=off` never uses one, and test launchers set it, so tests
//! stay hermetic). A server exits after `--idle` seconds without a request
//! (`HIRSEL_SERVE_IDLE`, default 900).
use std::ffi::OsString;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[path = "serve_candidate.rs"]
mod candidate;
#[path = "serve_watch.rs"]
mod watch_events;

const MAGIC: &[u8; 4] = b"HRS1";
const DEFAULT_IDLE: u64 = 900;
/// The commands a server runs; the rest always run in their own process.
const COMMANDS: [&str; 3] = ["check", "build", "run"];

fn identity() -> String {
    use sha2::Digest;
    let mut hash = sha2::Sha256::new();
    hash.update(hirsel_driver::COMPILER_IDENTITY);
    hash.update(include_bytes!("main.rs"));
    hash.update(include_bytes!("serve.rs"));
    hash.update(include_bytes!("serve_watch.rs"));
    hash.update(include_bytes!("serve_candidate.rs"));
    hash.finalize()[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The workspace a path belongs to.
pub fn workspace(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let start = if absolute.is_dir() {
        absolute.clone()
    } else {
        absolute.parent().map(Path::to_path_buf).unwrap_or_default()
    };
    let root = start
        .ancestors()
        .find(|a| a.join(".git").exists())
        .unwrap_or(&start);
    std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf())
}

/// The workspace's socket for this compiler.
pub fn socket(workspace: &Path) -> PathBuf {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(workspace.as_os_str().as_encoded_bytes());
    let name: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
    runtime_dir().join(format!("{}-{name}.sock", identity()))
}
fn runtime_dir() -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!("hirsel-{}", unsafe { libc::getuid() }))
        });
    base.join("hirsel")
}

/// Process-wide settings must match: never mutate a multithreaded process's environment.
fn compatible_environment(env: &[(String, String)]) -> Vec<(String, String)> {
    let mut vars: Vec<_> = env.iter().cloned().collect();
    vars.sort();
    vars
}
fn environment() -> Option<Vec<(String, String)>> {
    std::env::vars_os()
        .map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
        .collect()
}

// Reading Linux's proc status avoids changing the process-wide umask while
// another thread may create a file. An unsupported host uses the cold path.
fn umask() -> Option<u32> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    u32::from_str_radix(
        status
            .lines()
            .find_map(|line| line.strip_prefix("Umask:"))?
            .trim(),
        8,
    )
    .ok()
}

pub fn candidate_client(args: &[OsString]) -> u8 {
    candidate::client(args)
}

// ---- the client ----

/// Run `args` (the arguments after the program name) on the workspace's
/// server when the command is one a server runs and a server takes it: its
/// exit code, else None and the caller runs the command itself.
pub fn client(args: &[OsString]) -> Option<u8> {
    let mode = std::env::var("HIRSEL_DAEMON").unwrap_or_default();
    if mode == "off" {
        return None;
    }
    let argv: Vec<String> = args
        .iter()
        .map(|a| a.to_str().map(str::to_owned))
        .collect::<Option<_>>()?;
    let separator = argv.iter().position(|a| a == "--").unwrap_or(argv.len());
    let flags = &argv[..separator];
    // Measurements and watching are this process's own.
    if flags
        .iter()
        .any(|a| a.starts_with("--timings") || a == "--mem-usage" || a == "--watch")
    {
        return None;
    }
    let command = flags.iter().find(|a| !a.starts_with("--"))?;
    if !COMMANDS.contains(&command.as_str()) {
        return None;
    }
    // A program reading a terminal must be in its foreground process group.
    if command == "run" && unsafe { libc::isatty(0) } == 1 {
        return None;
    }
    let entry = entry_argument(&flags[flags.iter().position(|a| a == command)? + 1..])?;
    let socket = socket(&workspace(Path::new(&entry)));
    let stream = match UnixStream::connect(&socket) {
        Ok(stream) => stream,
        Err(_) => start(&socket, &workspace(Path::new(&entry)))?,
    };
    request(stream, argv)
}

/// The entry path among a command's arguments: its first operand, skipping
/// the values of the options that take one.
fn entry_argument(args: &[String]) -> Option<String> {
    const VALUED: [&str; 13] = [
        "--backend",
        "--target",
        "--proofs",
        "--trust",
        "-o",
        "--run",
        "--grants",
        "--record",
        "--revocations",
        "--replay",
        "--max-steps",
        "--cores",
        "--simulation",
    ];
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if VALUED.contains(&arg.as_str()) {
            args.next();
        } else if !arg.starts_with('-') {
            return Some(arg.clone());
        }
    }
    None
}

fn request(mut stream: UnixStream, argv: Vec<String>) -> Option<u8> {
    let cwd = std::env::current_dir()
        .ok()?
        .into_os_string()
        .into_string()
        .ok()?;
    let body = serde_json::json!({
        "identity": identity(),
        "cwd": cwd,
        "argv": argv,
        "env": environment()?,
        "umask": umask()?,
    })
    .to_string();
    let mut frame = MAGIC.to_vec();
    frame.extend((body.len() as u32).to_le_bytes());
    frame.extend(body.as_bytes());
    send_with_fds(&stream, &frame, &[0, 1, 2]).ok()?;
    // `A` accepts; `R` (refused) and `B` (busy) leave the command to us.
    let mut status = [0u8; 1];
    stream.read_exact(&mut status).ok()?;
    if status[0] != b'A' {
        return None;
    }
    // Accepted: the server writes the command's output from here on, so the
    // command is the server's to finish.
    let mut code = [0u8; 2];
    match stream.read_exact(&mut code) {
        Ok(()) if code[0] == b'X' => Some(code[1]),
        _ => {
            eprintln!("hirsel: the resident compiler stopped before the command finished");
            Some(3)
        }
    }
}

/// Start a server for `workspace` in its own session and wait for its socket.
fn start(socket: &Path, workspace: &Path) -> Option<UnixStream> {
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe().ok()?;
    let mut command = std::process::Command::new(exe);
    command
        .arg("serve")
        .arg("--workspace")
        .arg(workspace)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().ok()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Ok(stream) = UnixStream::connect(socket) {
            return Some(stream);
        }
        if let Ok(Some(_)) = child.try_wait() {
            return UnixStream::connect(socket).ok();
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    None
}

fn send_with_fds(stream: &UnixStream, bytes: &[u8], fds: &[RawFd]) -> std::io::Result<()> {
    let space = unsafe { libc::CMSG_SPACE(std::mem::size_of_val(fds) as u32) } as usize;
    let mut control = vec![0usize; space.div_ceil(std::mem::size_of::<usize>())];
    let mut iov = libc::iovec {
        iov_base: bytes.as_ptr() as *mut libc::c_void,
        iov_len: bytes.len(),
    };
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = space as _;
    unsafe {
        let header = libc::CMSG_FIRSTHDR(&message);
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN(std::mem::size_of_val(fds) as u32) as _;
        std::ptr::copy_nonoverlapping(
            fds.as_ptr().cast::<u8>(),
            libc::CMSG_DATA(header),
            std::mem::size_of_val(fds),
        );
    }
    let sent = unsafe { libc::sendmsg(stream.as_raw_fd(), &message, libc::MSG_NOSIGNAL) };
    if sent < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // The rest of a frame too long for one message follows without descriptors.
    (&*stream).write_all(&bytes[sent as usize..])
}

// ---- the server ----

/// `hirsel serve [--workspace <dir>] [--idle <seconds>]`.
pub fn serve(args: &[OsString]) -> u8 {
    let mut workspace_dir = None;
    let mut idle = std::env::var("HIRSEL_SERVE_IDLE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_IDLE);
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match (arg.to_str(), args.next()) {
            (Some("--workspace"), Some(dir)) => workspace_dir = Some(PathBuf::from(dir)),
            (Some("--idle"), Some(seconds)) => {
                match seconds.to_str().and_then(|s| s.parse().ok()) {
                    Some(seconds) => idle = seconds,
                    None => return usage(),
                }
            }
            _ => return usage(),
        }
    }
    // Own a process group before accepting run requests. Disconnect cleanup
    // must never signal the invoking shell's process group.
    unsafe {
        if libc::getpgrp() != libc::getpid() && libc::setpgid(0, 0) != 0 {
            eprintln!("hirsel: serve: {}", std::io::Error::last_os_error());
            return 3;
        }
    }
    let workspace = workspace(&workspace_dir.unwrap_or_else(|| PathBuf::from(".")));
    let socket = socket(&workspace);
    let listener = match bind(&socket) {
        Ok(Some(listener)) => listener,
        Ok(None) => {
            eprintln!(
                "hirsel: serve: a server already runs at {}",
                socket.display()
            );
            return 0;
        }
        Err(error) => {
            eprintln!("hirsel: serve: {}: {error}", socket.display());
            return 3;
        }
    };
    eprintln!(
        "hirsel: serving {} at {} (idle timeout {idle} s)",
        workspace.display(),
        socket.display()
    );
    let startup = environment().unwrap_or_default();
    hirsel_driver::session::enable_resident(8);
    hirsel_engine::free_sessions(true);
    let code = run_server(&listener, Duration::from_secs(idle), &startup, &workspace);
    let _ = std::fs::remove_file(&socket);
    code
}
fn usage() -> u8 {
    eprintln!("usage: hirsel serve [--workspace <dir>] [--idle <seconds>]");
    2
}

/// Bind the socket, replacing a stale one; None when a live server holds it.
fn bind(socket: &Path) -> std::io::Result<Option<UnixListener>> {
    let dir = socket.parent().unwrap();
    std::fs::create_dir_all(dir)?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    // Serialize stale-socket replacement with other starting servers.
    use std::os::unix::fs::OpenOptionsExt;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(socket.with_extension("lock"))?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    match UnixListener::bind(socket) {
        Ok(listener) => Ok(Some(listener)),
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            if UnixStream::connect(socket).is_ok() {
                return Ok(None);
            }
            std::fs::remove_file(socket)?;
            UnixListener::bind(socket).map(Some)
        }
        Err(e) => Err(e),
    }
}

/// Serve until `idle` passes without a request. One thread runs commands,
/// one at a time (a command owns the process's descriptors, directory and
/// environment while it runs); another accepts connections and answers busy
/// (`B`) at once while a command runs, so a client never waits on another's
/// command and runs its own in-process instead.
fn run_server(
    listener: &UnixListener,
    idle: Duration,
    startup: &[(String, String)],
    root: &Path,
) -> u8 {
    use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
    use std::sync::{Arc, mpsc};
    let ours = compatible_environment(startup);
    let busy = Arc::new(AtomicBool::new(false));
    let (send, receive) = mpsc::channel::<UnixStream>();
    let Ok(acceptor) = listener.try_clone() else {
        return 3;
    };
    {
        let busy = busy.clone();
        std::thread::spawn(move || {
            for stream in acceptor.incoming() {
                let Ok(mut stream) = stream else { continue };
                if busy.swap(true, SeqCst) {
                    let _ = stream.write_all(b"B");
                } else if send.send(stream).is_err() {
                    return;
                }
            }
        });
    }
    loop {
        let Ok(stream) = receive.recv_timeout(idle) else {
            return 0;
        };
        // A request that panicked may have left a session half-updated:
        // answer it, then stop, and the next client starts afresh.
        let served = serve_one(stream, &ours, root);
        busy.store(false, SeqCst);
        match served {
            Err(()) => return 3,
            // Drain the connection before retiring process-global append-only
            // Symbol/DefId tables. Live handles are never silently evicted.
            Ok(true) => return 0,
            Ok(false) => {}
        }
    }
}

struct Request {
    cwd: PathBuf,
    argv: Vec<String>,
    env: Vec<(String, String)>,
    fds: Vec<OwnedFd>,
    umask: u32,
}

fn serve_one(mut stream: UnixStream, ours: &[(String, String)], root: &Path) -> Result<bool, ()> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let Some((magic, value, fds)) = read_wire(&stream) else {
        let _ = stream.write_all(b"R");
        return Ok(false);
    };
    if &magic == candidate::MAGIC {
        return candidate::serve(stream, value, fds, ours, root).map(|_| true);
    }
    let Some(request) = legacy_request(value, fds) else {
        let _ = stream.write_all(b"R");
        return Ok(false);
    };
    if compatible_environment(&request.env) != ours
        || request.fds.len() != 3
        || Some(request.umask) != umask()
    {
        let _ = stream.write_all(b"R");
        return Ok(false);
    }
    if stream.write_all(b"A").is_err() {
        return Ok(false);
    }
    let _ = stream.set_read_timeout(None);
    // A run ends with its client: a client that goes away (interrupted,
    // killed) takes the program it ran with it, which here is this server
    // and the process group it leads.
    let running = request
        .argv
        .iter()
        .take_while(|a| a.as_str() != "--")
        .find(|a| !a.starts_with("--"))
        .is_some_and(|a| a == "run");
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    if running && let Ok(mut watch) = stream.try_clone() {
        let done = done.clone();
        std::thread::spawn(move || {
            let mut byte = [0u8; 1];
            let ended = matches!(watch.read(&mut byte), Ok(0) | Err(_));
            if ended && !done.load(std::sync::atomic::Ordering::SeqCst) {
                unsafe { libc::kill(-libc::getpid(), libc::SIGKILL) };
            }
        });
    }
    let argv: Vec<OsString> = request.argv.iter().map(OsString::from).collect();
    let started = Instant::now();
    let outcome = with_request(&request, || {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::execute_resident(argv)
        }))
    });
    done.store(true, std::sync::atomic::Ordering::SeqCst);
    let code = outcome.as_ref().map_or(101, |code| *code);
    eprintln!(
        "hirsel: serve: {} exited {code} in {:.1} ms",
        request.argv.join(" "),
        started.elapsed().as_secs_f64() * 1e3
    );
    let _ = stream.write_all(&[b'X', code]);
    let _ = stream.shutdown(std::net::Shutdown::Both);
    outcome.map(|_| false).map_err(|_| ())
}

#[cfg(test)]
fn read_request(stream: &UnixStream) -> Option<Request> {
    let (magic, value, fds) = read_wire(stream)?;
    (&magic == MAGIC).then_some(())?;
    legacy_request(value, fds)
}

fn read_wire(stream: &UnixStream) -> Option<([u8; 4], serde_json::Value, Vec<OwnedFd>)> {
    let mut buffer = [0u8; 8];
    let (received, fds) = recv_with_fds(stream, &mut buffer).ok()?;
    let mut bytes = buffer[..received].to_vec();
    while bytes.len() < 8 {
        let mut more = [0u8; 8];
        let remaining = 8 - bytes.len();
        let n = (&*stream).read(&mut more[..remaining]).ok()?;
        if n == 0 {
            return None;
        }
        bytes.extend(&more[..n]);
    }
    if &bytes[..4] != MAGIC && &bytes[..4] != candidate::MAGIC {
        return None;
    }
    let length = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    if length > 8 * 1024 * 1024 {
        return None;
    }
    while bytes.len() < 8 + length {
        let mut more = vec![0u8; 8 + length - bytes.len()];
        let n = (&*stream).read(&mut more).ok()?;
        if n == 0 {
            return None;
        }
        bytes.extend(&more[..n]);
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes[8..8 + length]).ok()?;
    if value["identity"].as_str()? != identity() {
        return None;
    }
    Some((bytes[..4].try_into().ok()?, value, fds))
}

fn legacy_request(value: serde_json::Value, fds: Vec<OwnedFd>) -> Option<Request> {
    let strings = |v: &serde_json::Value| -> Option<Vec<String>> {
        v.as_array()?
            .iter()
            .map(|s| s.as_str().map(str::to_owned))
            .collect()
    };
    Some(Request {
        cwd: PathBuf::from(value["cwd"].as_str()?),
        argv: strings(&value["argv"])?,
        env: value["env"]
            .as_array()?
            .iter()
            .map(|pair| Some((pair[0].as_str()?.to_owned(), pair[1].as_str()?.to_owned())))
            .collect::<Option<_>>()?,
        fds,
        umask: value["umask"].as_u64()?.try_into().ok()?,
    })
}

fn recv_with_fds(stream: &UnixStream, buffer: &mut [u8]) -> std::io::Result<(usize, Vec<OwnedFd>)> {
    let space = unsafe { libc::CMSG_SPACE((3 * std::mem::size_of::<RawFd>()) as u32) } as usize;
    let mut control = vec![0usize; space.div_ceil(std::mem::size_of::<usize>())];
    let mut iov = libc::iovec {
        iov_base: buffer.as_mut_ptr().cast(),
        iov_len: buffer.len(),
    };
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = space as _;
    let received =
        unsafe { libc::recvmsg(stream.as_raw_fd(), &mut message, libc::MSG_CMSG_CLOEXEC) };
    if received < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut fds = vec![];
    unsafe {
        let mut header = libc::CMSG_FIRSTHDR(&message);
        while !header.is_null() {
            if (*header).cmsg_level == libc::SOL_SOCKET && (*header).cmsg_type == libc::SCM_RIGHTS {
                let data = libc::CMSG_DATA(header).cast::<RawFd>();
                let count = ((*header).cmsg_len as usize - libc::CMSG_LEN(0) as usize)
                    / std::mem::size_of::<RawFd>();
                for i in 0..count {
                    fds.push(OwnedFd::from_raw_fd(std::ptr::read_unaligned(data.add(i))));
                }
            }
            header = libc::CMSG_NXTHDR(&message, header);
        }
    }
    if message.msg_flags & libc::MSG_CTRUNC != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "truncated descriptors",
        ));
    }
    Ok((received as usize, fds))
}

/// Run `f` as the request's process would: on its standard descriptors, in
/// its working directory, under its environment; then restore the server's.
fn with_request<R>(request: &Request, f: impl FnOnce() -> R) -> R {
    let saved: Vec<RawFd> = (0..3)
        .map(|fd| unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) })
        .collect();
    let cwd = std::env::current_dir().ok();
    for (fd, client) in request.fds.iter().enumerate() {
        unsafe { libc::dup2(client.as_raw_fd(), fd as RawFd) };
    }
    let entered = std::env::set_current_dir(&request.cwd).is_ok();
    let result = if entered {
        f()
    } else {
        unreachable_cwd(&request.cwd)
    };
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    for (fd, saved) in saved.into_iter().enumerate() {
        unsafe {
            libc::dup2(saved, fd as RawFd);
            libc::close(saved);
        }
    }
    if let Some(cwd) = cwd {
        let _ = std::env::set_current_dir(cwd);
    }
    result
}
fn unreachable_cwd<R>(cwd: &Path) -> R {
    panic!(
        "hirsel: serve: the client's working directory {} is gone",
        cwd.display()
    )
}
/// `hirsel check --watch <args>`: check, then reconcile current bytes when
/// the kernel delivers an input event. Register before comparing bytes so an
/// edit racing the previous check or watch installation cannot be missed.
pub fn watch(args: Vec<OsString>) -> u8 {
    hirsel_driver::session::enable_resident(1);
    hirsel_engine::free_sessions(true);
    loop {
        let started = Instant::now();
        let code = crate::execute_resident(args.clone());
        eprintln!(
            "hirsel: checked in {} ms (exit {code}); watching for changes",
            started.elapsed().as_millis()
        );
        loop {
            let mut events =
                match watch_events::Watch::new(hirsel_driver::session::resident_watched()) {
                    Ok(events) => events,
                    Err(error) => {
                        eprintln!("hirsel: cannot watch inputs: {error}");
                        return 3;
                    }
                };
            if hirsel_driver::session::resident_changed() {
                break;
            }
            if let Err(error) = events.wait() {
                eprintln!("hirsel: cannot watch inputs: {error}");
                return 3;
            }
            // Reinstall after every wake, including overflow/ignored watches,
            // before the next byte comparison. An atomic save or renamed
            // symlink target cannot leave us attached only to a stale inode.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resident_descriptors_and_split_frames_round_trip() {
        let (sender, receiver) = UnixStream::pair().unwrap();
        let file = std::fs::File::open("/dev/null").unwrap();
        let expected = b"a frame split after its descriptors";
        send_with_fds(&sender, &expected[..7], &[file.as_raw_fd(); 3]).unwrap();
        (&sender).write_all(&expected[7..]).unwrap();
        let mut buffer = [0; 128];
        let (n, fds) = recv_with_fds(&receiver, &mut buffer).unwrap();
        assert_eq!(fds.len(), 3);
        assert_eq!(&buffer[..n], &expected[..n]);
        for fd in fds {
            assert_ne!(
                unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
                0
            );
        }
    }
    #[test]
    fn resident_request_bounds_frame_size_before_allocation() {
        let (sender, receiver) = UnixStream::pair().unwrap();
        let mut bytes = MAGIC.to_vec();
        bytes.extend(u32::MAX.to_le_bytes());
        sender
            .set_write_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        (&sender).write_all(&bytes).unwrap();
        assert!(read_request(&receiver).is_none());
    }
    #[test]
    fn resident_entry_options_do_not_select_an_option_value_as_a_workspace() {
        let args: Vec<_> = [
            "--backend",
            "cranelift",
            "--debug",
            "-o",
            "out",
            "source.hir",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        assert_eq!(entry_argument(&args).as_deref(), Some("source.hir"));
    }
}
