#[test]
fn a_pass() {}

#[test]
#[ignore = "the reporting regression deliberately executes a failing body"]
fn b_fail() {
    panic!("fixture failure output");
}

#[test]
fn c_hangs() {
    if std::env::var_os("HIRSEL_REPORT_FIXTURE_HANG").is_some() {
        loop {
            std::thread::park();
        }
    }
}
