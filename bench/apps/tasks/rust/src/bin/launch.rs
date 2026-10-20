//! Tiny pre-exec timestamp marker, inside perf but outside the server lifetime.
use std::{env, fs, os::unix::process::CommandExt, process::Command};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let marker = args.next().ok_or("missing marker")?;
    let executable = args.next().ok_or("missing executable")?;
    let mut clock = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: clock is writable timespec storage for CLOCK_MONOTONIC.
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut clock) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let pid = if env::var_os("TASKS_PERF_WRAPPER").is_some() {
        // SAFETY: getppid has no pointer arguments or preconditions.
        unsafe { libc::getppid() as u32 }
    } else {
        std::process::id()
    };
    fs::write(
        marker,
        format!(
            "{{\"pid\":{pid},\"started_ns\":{}}}",
            clock.tv_sec as u64 * 1_000_000_000 + clock.tv_nsec as u64
        ),
    )?;
    Err(Command::new(executable).args(args).exec().into())
}
