use hirsel_rt::{
    io::{Adapter, AdapterError, Completion, Request, Tree},
    io_real::RealIo,
    io_sim::SimIo,
};
use std::io::Write;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct Output(Arc<Mutex<Vec<u8>>>);
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn case_reporting_records_the_bound_seed_and_replays_before_publication() {
    let root_scratch = tempfile::Builder::new()
        .prefix("hirsel-test-report-")
        .tempdir()
        .unwrap();
    let root = root_scratch.path().to_path_buf();
    let output = Output(Arc::new(Mutex::new(vec![])));
    let mut real = RealIo::grant_with_output(&root, output.clone()).unwrap();
    real.test_context(u64::MAX);
    let success = Request::TestCheckCase {
        test: 1,
        condition: true,
        case: u64::MAX,
    };
    let failure = Request::TestCheckCase {
        test: 1,
        condition: false,
        case: 3,
    };
    assert_eq!(real.call(success.clone()), Ok(Completion::Checked(true)));
    assert!(real.stdout().is_empty());
    assert!(output.0.lock().unwrap().is_empty());
    assert_eq!(real.call(failure.clone()), Ok(Completion::Checked(false)));
    let expected = b"test failure: seed=18446744073709551615 case=3\n";
    assert_eq!(real.stdout(), expected);
    assert_eq!(output.0.lock().unwrap().as_slice(), expected);
    let tape = real.tape().to_vec();
    assert_eq!(crate::support::tape::events(&tape), tape);
    let mut replay = SimIo::new(Tree::default(), tape);
    replay.test_context(u64::MAX);
    assert_eq!(replay.call(success), Ok(Completion::Checked(true)));
    let forged = Request::TestCheckCase {
        test: 1,
        condition: false,
        case: 4,
    };
    assert_eq!(replay.call(forged), Err(AdapterError::ReplayMismatch(1)));
    assert!(replay.stdout().is_empty());
    assert_eq!(replay.position(), 1);
    assert_eq!(replay.call(failure), Ok(Completion::Checked(false)));
    assert_eq!(replay.stdout(), expected);
    replay.finish().unwrap();
    drop(real);
    std::fs::remove_dir(root).unwrap();
}
