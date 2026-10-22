use super::*;

#[test]
fn canonical_output_prefixes_refuse_missing_mismatched_or_nonoutput_counts() {
    let bytes = Some(&b"abc"[..]);
    for (result, count) in [
        (Ok(Completion::Unit(Ok(()))), None),
        (Ok(Completion::Unit(Ok(()))), Some(2)),
        (Ok(Completion::Unit(Err(IoError::Fault(1)))), Some(4)),
        (Ok(Completion::Unit(Err(IoError::Canceled(7)))), Some(1)),
        (Ok(Completion::Unit(Err(IoError::NotGranted))), Some(1)),
        (Err(AdapterError::InvalidHandle(99)), Some(0)),
        (Ok(Completion::Size(Ok(3))), Some(3)),
    ] {
        assert!(project(bytes, &result, count).is_err());
    }
    assert!(project(None, &Ok(Completion::Unit(Ok(()))), Some(0)).is_err());
    for (result, count, expected) in [
        (Ok(Completion::Unit(Ok(()))), 3, &b"abc"[..]),
        (Ok(Completion::Unit(Err(IoError::Fault(1)))), 2, &b"ab"[..]),
        (Ok(Completion::Unit(Err(IoError::Fault(1)))), 3, &b"abc"[..]),
        (Ok(Completion::Unit(Err(IoError::Canceled(7)))), 0, &b""[..]),
        (Ok(Completion::Unit(Err(IoError::NotGranted))), 0, &b""[..]),
    ] {
        assert_eq!(
            project(bytes, &result, Some(count)).unwrap(),
            Some(expected.to_vec())
        );
    }
}
