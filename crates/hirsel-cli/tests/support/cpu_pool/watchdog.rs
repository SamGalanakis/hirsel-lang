fn qual_native_output(command: &mut Command, root: &std::path::Path) -> std::process::Output {
    use std::os::unix::process::CommandExt;
    let stdout = root.join("child.stdout");
    let stderr = root.join("child.stderr");
    command
        .process_group(0)
        .stdout(fs::File::create(&stdout).unwrap())
        .stderr(fs::File::create(&stderr).unwrap());
    let mut child = command.spawn().unwrap();
    let pid = child.id() as i32;
    let (send, done) = std::sync::mpsc::sync_channel(1);
    let waiter = std::thread::spawn(move || send.send(child.wait()).unwrap());
    let status = match done.recv_timeout(std::time::Duration::from_secs(15)) {
        Ok(status) => status.unwrap(),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            // Only the process group created by this command is signalled.
            unsafe { libc::kill(-pid, libc::SIGKILL) };
            done.recv().unwrap().unwrap();
            waiter.join().unwrap();
            panic!("native CPU qualification exceeded 15s: {command:?}");
        }
        Err(error) => panic!("native CPU qualification waiter failed: {error}"),
    };
    waiter.join().unwrap();
    std::process::Output {
        status,
        stdout: fs::read(stdout).unwrap(),
        stderr: fs::read(stderr).unwrap(),
    }
}
