fn qual_native_output(command: &mut Command, root: &std::path::Path) -> std::process::Output {
    use std::os::unix::process::CommandExt;
    let stdout = root.join("child.stdout");
    let stderr = root.join("child.stderr");
    command
        .process_group(0)
        .stdout(fs::File::create(&stdout).unwrap())
        .stderr(fs::File::create(&stderr).unwrap());
    let mut child = command.spawn().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            // Only the process group created by this command is signalled.
            unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
            let _ = child.wait();
            panic!("native CPU qualification exceeded 15s: {command:?}");
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    };
    std::process::Output {
        status,
        stdout: fs::read(stdout).unwrap(),
        stderr: fs::read(stderr).unwrap(),
    }
}
