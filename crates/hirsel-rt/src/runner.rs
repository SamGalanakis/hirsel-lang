//! The runner's launcher (link design §2). A runner is one executable per target and runtime
//! variant, linked from this archive by the kit line with `main` an alias of
//! [`hirsel_runner_main`] and every runtime entry exported. It runs a program *image*: a shared
//! object holding only the program's units and the same launcher objects (`main`) an executable
//! links, which binds its runtime imports to the runner's one runtime. No executable is written
//! for the run.
//!
//! `hirsel-runner exec <tooling> <image> <argv...>` runs one image in this process, with
//! `argv` as the program's arguments (the first naming the program).
//! `hirsel-runner serve` is a fork server on its standard input, a Unix stream socket: each
//! request forks one child that runs one image, and the server reports the child's process ID
//! and then its wait status. The server is single-threaded, admits no run, starts no runtime,
//! runs no program code and maps no image itself; only its children do.
//!
//! A served child, in order: joins its own process group; takes the request's standard streams in
//! place of the server's control socket, closing the server's descriptors; restores the
//! server's startup signal mask (the server changes no disposition, so the child's are the ones
//! an executable launched like the server has); enters the request's directory and installs its
//! environment; checks that the image was linked for this runner (its `Hirsel` note holds this
//! runner's build ID) without executing it; `dlopen`s it `RTLD_NOW | RTLD_LOCAL`; and calls its
//! `main` with the request's arguments, which ignores SIGPIPE and enters
//! [`crate::launch::hirsel_rt_v2_main`] exactly as the executable's does, then exits with its
//! status through `exit`, so the runtime's sinks and the C streams flush as when an
//! executable's `main` returns. A signal death stays a signal death. `exec` does the same from
//! the directory check on, in the process group, directory, environment and streams its spawner
//! gave it, as an executable has them.
//!
//! Until the image's `main` runs, a failure is the runner's (a tooling outcome, never a program
//! observation): the child writes `image-load <reason>` to the request's tooling path and exits
//! with [`TOOLING_EXIT`] without running any program code.
use std::ffi::{CStr, CString, c_char, c_int, c_void};

/// The exit status of a runner child that ran no program code; its tooling path holds why.
pub const TOOLING_EXIT: c_int = 125;
/// The ELF note owner and type naming the runner an image was linked for: its description is
/// that runner's GNU build ID.
pub const NOTE_OWNER: &[u8] = b"Hirsel\0";
pub const NOTE_TYPE: u32 = 1;
/// The fork server's replies: `[tag, i32 little-endian]`.
pub const REPLY_STARTED: u8 = b'P';
pub const REPLY_STATUS: u8 = b'S';

/// One run: where a failure before the image's `main` is reported, the image, the working
/// directory and environment (the server's own when absent) and the program's arguments.
struct Request {
    tooling: CString,
    image: CString,
    directory: Option<CString>,
    environment: Option<Vec<CString>>,
    arguments: Vec<CString>,
    /// Standard input, output and error, received from the requester.
    streams: Option<[c_int; 3]>,
}

/// The runner's entry, aliased as its `main`.
///
/// # Safety
/// `argv` holds `argc` NUL-terminated strings, as the C runtime passes `main` its arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_runner_main(argc: c_int, argv: *const *const c_char) -> c_int {
    let count = usize::try_from(argc).unwrap_or(0);
    // SAFETY: caller contract.
    let arguments: Vec<&CStr> = (0..count)
        .map(|i| unsafe { CStr::from_ptr(*argv.add(i)) })
        .collect();
    match arguments.get(1).map(|a| a.to_bytes()) {
        Some(b"exec") if arguments.len() >= 5 => {
            let request = Request {
                tooling: arguments[2].into(),
                image: arguments[3].into(),
                directory: None,
                environment: None,
                arguments: arguments[4..].iter().map(|&a| a.into()).collect(),
                streams: None,
            };
            // SAFETY: this process runs nothing else.
            unsafe { child(&request, None) }
        }
        Some(b"serve") if arguments.len() == 2 => serve(0),
        _ => {
            let usage = b"usage: hirsel-runner exec <tooling> <image> <argv...> | serve\n";
            // SAFETY: a plain write of a live buffer.
            unsafe { libc::write(2, usage.as_ptr().cast(), usage.len()) };
            2
        }
    }
}

/// Serve requests on `control` until it ends: 0, or 1 when the control socket failed.
fn serve(control: c_int) -> c_int {
    // SAFETY: reads this thread's signal mask.
    let mask = unsafe {
        let mut mask = std::mem::zeroed();
        libc::pthread_sigmask(libc::SIG_SETMASK, std::ptr::null(), &mut mask);
        mask
    };
    loop {
        let request = match receive(control) {
            Ok(Some(request)) => request,
            Ok(None) => return 0,
            Err(error) => {
                let message = format!("hirsel-runner: {error}\n");
                // SAFETY: a plain write of a live buffer.
                unsafe { libc::write(2, message.as_ptr().cast(), message.len()) };
                return 1;
            }
        };
        // SAFETY: single-threaded; the child only runs `child`.
        let pid = unsafe { libc::fork() };
        if pid == 0 {
            // SAFETY: the forked child runs nothing else.
            unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &mask, std::ptr::null_mut()) };
            unsafe { child(&request, Some(control)) };
        }
        // Both sides set the group, so a kill of the group never races the child's own call.
        if pid > 0 {
            // SAFETY: plain system calls on this server's own child and descriptors.
            unsafe { libc::setpgid(pid, pid) };
        }
        for stream in request.streams.into_iter().flatten() {
            // SAFETY: the server's received copy, which only the child keeps.
            unsafe { libc::close(stream) };
        }
        if pid < 0 {
            return 1;
        }
        if reply(control, REPLY_STARTED, pid).is_err() {
            // The requester is gone: its child is still this server's to reap.
            reap(pid);
            return 1;
        }
        let status = reap(pid);
        if reply(control, REPLY_STATUS, status).is_err() {
            return 1;
        }
    }
}

/// Wait for `pid` and return its raw wait status.
fn reap(pid: libc::pid_t) -> c_int {
    let mut status = 0;
    // SAFETY: waits for this server's own child.
    while unsafe { libc::waitpid(pid, &mut status, 0) } < 0 {
        if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            break;
        }
    }
    status
}

fn reply(control: c_int, tag: u8, value: c_int) -> std::io::Result<()> {
    let mut message = [0u8; 5];
    message[0] = tag;
    message[1..].copy_from_slice(&value.to_le_bytes());
    write_all(control, &message)
}

fn write_all(fd: c_int, mut bytes: &[u8]) -> std::io::Result<()> {
    while !bytes.is_empty() {
        // SAFETY: writes from a live buffer.
        let written = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
        if written < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        bytes = &bytes[written as usize..];
    }
    Ok(())
}

fn read_exact(fd: c_int, mut buffer: &mut [u8]) -> std::io::Result<()> {
    while !buffer.is_empty() {
        // SAFETY: reads into a live buffer.
        let read = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
        if read < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if read == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        buffer = &mut buffer[read as usize..];
    }
    Ok(())
}

/// One request (`encode_request`): its length and three stream descriptors in one message,
/// then its fields. `None` when the requester closed the socket between requests.
fn receive(control: c_int) -> std::io::Result<Option<Request>> {
    let mut length = [0u8; 4];
    // SAFETY: CMSG_SPACE is a size computation.
    let space = unsafe { libc::CMSG_SPACE(3 * size_of::<c_int>() as u32) } as usize;
    let mut ancillary = vec![0u64; space.div_ceil(8)];
    let mut vector = libc::iovec {
        iov_base: length.as_mut_ptr().cast(),
        iov_len: length.len(),
    };
    // SAFETY: a zeroed msghdr is valid; its buffers are set below and outlive the call.
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &mut vector;
    message.msg_iovlen = 1;
    message.msg_control = ancillary.as_mut_ptr().cast();
    message.msg_controllen = space as _;
    let received = loop {
        // SAFETY: the message's buffers are live.
        let received = unsafe { libc::recvmsg(control, &mut message, libc::MSG_CMSG_CLOEXEC) };
        if received < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
        {
            continue;
        }
        break received;
    };
    if received == 0 {
        return Ok(None);
    }
    if received < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut streams = None;
    // SAFETY: walks the control messages the kernel wrote into `ancillary`.
    unsafe {
        let mut header = libc::CMSG_FIRSTHDR(&message);
        while !header.is_null() {
            if (*header).cmsg_level == libc::SOL_SOCKET && (*header).cmsg_type == libc::SCM_RIGHTS {
                let data = libc::CMSG_DATA(header).cast::<c_int>();
                let count =
                    ((*header).cmsg_len as usize - libc::CMSG_LEN(0) as usize) / size_of::<c_int>();
                if count == 3 {
                    streams = Some([
                        data.read_unaligned(),
                        data.add(1).read_unaligned(),
                        data.add(2).read_unaligned(),
                    ]);
                } else {
                    for i in 0..count {
                        libc::close(data.add(i).read_unaligned());
                    }
                }
            }
            header = libc::CMSG_NXTHDR(&message, header);
        }
    }
    let invalid = |what: &str| std::io::Error::new(std::io::ErrorKind::InvalidData, what);
    if received < 4 {
        read_exact(control, &mut length[received as usize..])?;
    }
    let streams = streams.ok_or_else(|| invalid("a request without its three streams"))?;
    let mut payload = vec![0u8; u32::from_le_bytes(length) as usize];
    read_exact(control, &mut payload)?;
    let mut fields = payload
        .split(|&b| b == 0)
        .map(|field| CString::new(field).expect("split at NUL"));
    // The payload ends with a NUL, so the split ends with one empty piece.
    let mut next = || fields.next().ok_or_else(|| invalid("a truncated request"));
    let tooling = next()?;
    let image = next()?;
    let directory = next()?;
    let count = |field: CString| -> std::io::Result<usize> {
        field
            .to_str()
            .ok()
            .and_then(|text| text.parse().ok())
            .ok_or_else(|| invalid("a malformed request count"))
    };
    let argument_count = count(next()?)?;
    let arguments = (0..argument_count)
        .map(|_| next())
        .collect::<Result<Vec<_>, _>>()?;
    let environment_count = count(next()?)?;
    let environment = (0..environment_count)
        .map(|_| next())
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(Request {
        tooling,
        image,
        directory: Some(directory),
        environment: Some(environment),
        arguments,
        streams: Some(streams),
    }))
}

/// A request as [`hirsel_runner_main`]'s server reads it: its fields, each NUL-terminated
/// (the tooling path, the image, the directory, the argument count, the arguments, the
/// environment count and the `NAME=value` entries), after their length as a little-endian
/// `u32`. The requester sends the length with the three stream descriptors (`SCM_RIGHTS`).
pub fn encode_request(
    tooling: &[u8],
    image: &[u8],
    directory: &[u8],
    arguments: &[&[u8]],
    environment: &[&[u8]],
) -> Vec<u8> {
    let mut payload = Vec::new();
    let mut field = |bytes: &[u8]| {
        payload.extend_from_slice(bytes);
        payload.push(0);
    };
    field(tooling);
    field(image);
    field(directory);
    field(arguments.len().to_string().as_bytes());
    arguments.iter().for_each(|a| field(a));
    field(environment.len().to_string().as_bytes());
    environment.iter().for_each(|e| field(e));
    let mut message = (payload.len() as u32).to_le_bytes().to_vec();
    message.extend(payload);
    message
}

/// Run `request` in this process, never returning: the image's `main`'s status, or a tooling
/// failure before it ran.
///
/// # Safety
/// This process runs nothing else: a fork server's child, or `exec`.
unsafe fn child(request: &Request, control: Option<c_int>) -> ! {
    // SAFETY: plain system calls on this process.
    unsafe {
        // A served child is its own group, so a kill of the group reaches its descendants and
        // never the server. `exec` keeps the group its spawner chose, as an executable does.
        if control.is_some() {
            libc::setpgid(0, 0);
        }
        if let Some(streams) = request.streams {
            for (target, stream) in streams.into_iter().enumerate() {
                if libc::dup2(stream, target as c_int) < 0 {
                    tooling(
                        request,
                        "the request's streams",
                        &std::io::Error::last_os_error(),
                    );
                }
            }
            for stream in streams {
                if stream > 2 {
                    libc::close(stream);
                }
            }
        }
        if let Some(control) = control.filter(|&fd| fd > 2) {
            libc::close(control);
        }
        if let Some(directory) = &request.directory
            && libc::chdir(directory.as_ptr()) != 0
        {
            tooling(
                request,
                "the working directory",
                &std::io::Error::last_os_error(),
            );
        }
    }
    if let Some(environment) = &request.environment {
        // The environment the runtime and the program read, as exec would install it. Leaked:
        // it is the process's for the rest of its life.
        let mut entries: Vec<*mut c_char> = environment
            .iter()
            .map(|entry| entry.clone().into_raw())
            .collect();
        entries.push(std::ptr::null_mut());
        // SAFETY: single-threaded; no reader runs concurrently.
        unsafe { libc::environ = Box::leak(entries.into_boxed_slice()).as_mut_ptr() };
    }
    match std::fs::read(path(&request.image)) {
        Ok(bytes) => match note(&bytes) {
            Ok(linked) if Some(linked) == build_id().as_deref() => {}
            Ok(_) => tooling(request, "the image", &"it was linked for another runner"),
            Err(reason) => tooling(request, "the image", &reason),
        },
        Err(error) => tooling(request, "the image", &error),
    }
    // SAFETY: the image's initialization is the program's own (its units and launcher objects
    // only); it runs here, in the child.
    let handle = unsafe { libc::dlopen(request.image.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
    if handle.is_null() {
        tooling(request, "dlopen", &dl_error());
    }
    // SAFETY: the symbol is the image's exported `main`.
    let main = unsafe { libc::dlsym(handle, c"main".as_ptr()) };
    if main.is_null() {
        tooling(request, "dlsym main", &dl_error());
    }
    // SAFETY: the image's `main` is the launcher's `int main(int, char **)`.
    let main: extern "C" fn(c_int, *const *const c_char) -> c_int =
        unsafe { std::mem::transmute::<*mut c_void, _>(main) };
    let mut arguments: Vec<*const c_char> = request.arguments.iter().map(|a| a.as_ptr()).collect();
    let count = arguments.len() as c_int;
    arguments.push(std::ptr::null());
    let status = main(count, arguments.as_ptr());
    // SAFETY: as returning from an executable's `main`.
    unsafe { libc::exit(status) }
}

fn dl_error() -> String {
    // SAFETY: dlerror returns null or this thread's last message.
    let message = unsafe { libc::dlerror() };
    if message.is_null() {
        "unknown error".into()
    } else {
        // SAFETY: a NUL-terminated message.
        unsafe { CStr::from_ptr(message) }
            .to_string_lossy()
            .into_owned()
    }
}

/// Report a failure before the image's `main` ran and end the child.
fn tooling(request: &Request, what: &str, error: &dyn std::fmt::Display) -> ! {
    let report = format!("image-load {what}: {error}\n");
    let _ = std::fs::write(path(&request.tooling), report);
    // SAFETY: ends this child without running anything else.
    unsafe { libc::_exit(TOOLING_EXIT) }
}

fn path(text: &CStr) -> &std::path::Path {
    use std::os::unix::ffi::OsStrExt;
    std::path::Path::new(std::ffi::OsStr::from_bytes(text.to_bytes()))
}

/// The description of `notes`' first note owned by `owner` with type `kind`.
fn find_note<'a>(mut notes: &'a [u8], owner: &[u8], kind: u32) -> Option<&'a [u8]> {
    let word = |bytes: &[u8], at: usize| -> Option<usize> {
        Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?) as usize)
    };
    while notes.len() >= 12 {
        let (name_size, description_size) = (word(notes, 0)?, word(notes, 4)?);
        let note_type = word(notes, 8)? as u32;
        let name_end = 12 + name_size.next_multiple_of(4);
        let end = name_end + description_size.next_multiple_of(4);
        let name = notes.get(12..12 + name_size)?;
        let description = notes.get(name_end..name_end + description_size)?;
        if name == owner && note_type == kind {
            return Some(description);
        }
        notes = notes.get(end..)?;
    }
    None
}

/// The runner build ID in an image file's `Hirsel` note, read without loading it, or why the
/// file is no image: not ELF64 little-endian (both targets), a loadable segment past its end
/// (a truncated file, which mapping would fault on) or no such note.
fn note(image: &[u8]) -> Result<&[u8], &'static str> {
    let half = |at: usize| -> Option<usize> {
        Some(u16::from_le_bytes(image.get(at..at + 2)?.try_into().ok()?) as usize)
    };
    let double = |at: usize| -> Option<usize> {
        usize::try_from(u64::from_le_bytes(image.get(at..at + 8)?.try_into().ok()?)).ok()
    };
    if image.get(..6) != Some(b"\x7fELF\x02\x01") {
        return Err("it is not an ELF64 little-endian file");
    }
    let malformed = "its program headers are malformed";
    let (table, size, count) = (
        double(0x20).ok_or(malformed)?,
        half(0x36).ok_or(malformed)?,
        half(0x38).ok_or(malformed)?,
    );
    let mut found = None;
    for i in 0..count {
        let header = table + i * size;
        let kind = image
            .get(header..header + 4)
            .ok_or(malformed)?
            .try_into()
            .map(u32::from_le_bytes)
            .map_err(|_| malformed)?;
        let (offset, length) = (
            double(header + 8).ok_or(malformed)?,
            double(header + 32).ok_or(malformed)?,
        );
        let contents = offset
            .checked_add(length)
            .and_then(|end| image.get(offset..end));
        match kind {
            libc::PT_LOAD if contents.is_none() => return Err("a segment lies past its end"),
            libc::PT_NOTE => {
                let notes = contents.ok_or("a note lies past its end")?;
                found = found.or(find_note(notes, NOTE_OWNER, NOTE_TYPE));
            }
            _ => {}
        }
    }
    found.ok_or("it names no runner")
}

/// The GNU build-ID note's type (`<elf.h>`).
const NT_GNU_BUILD_ID: u32 = 3;

/// This runner's GNU build ID, from its loaded program headers.
fn build_id() -> Option<Vec<u8>> {
    unsafe extern "C" fn first(
        info: *mut libc::dl_phdr_info,
        _size: libc::size_t,
        found: *mut c_void,
    ) -> c_int {
        // SAFETY: the loader passes a live record for each loaded object; the first is the
        // executable, whose PT_NOTE segments are mapped.
        unsafe {
            let info = &*info;
            for i in 0..usize::from(info.dlpi_phnum) {
                let header = &*info.dlpi_phdr.add(i);
                if header.p_type != libc::PT_NOTE {
                    continue;
                }
                let notes = std::slice::from_raw_parts(
                    (info.dlpi_addr + header.p_vaddr) as *const u8,
                    header.p_memsz as usize,
                );
                if let Some(id) = find_note(notes, b"GNU\0", NT_GNU_BUILD_ID) {
                    *found.cast::<Option<Vec<u8>>>() = Some(id.to_vec());
                    break;
                }
            }
        }
        1
    }
    let mut found: Option<Vec<u8>> = None;
    // SAFETY: the callback writes only `found`, and stops after the first object.
    unsafe { libc::dl_iterate_phdr(Some(first), (&raw mut found).cast()) };
    found
}
