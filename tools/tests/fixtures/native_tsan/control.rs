use std::io::Write;
use std::process::{Command, Stdio};

pub fn emitted_access_control(object: &[u8]) {
    let mut child = Command::new("python3")
        .args(["-c", include_str!("control.py")])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(object).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("2 native TSan controls passed"),
        "{output:?}"
    );
}
