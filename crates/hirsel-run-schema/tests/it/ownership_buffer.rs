use hirsel_run_schema::ownership_trace::{Buffer, BufferFull};

#[test]
fn ownership_buffer_concurrent_issuers_preserve_every_row_and_local_order() {
    let buffer = Buffer::new(256, (u32::MAX, u32::MAX));
    std::thread::scope(|scope| {
        for task in 0..4 {
            let buffer = &buffer;
            scope.spawn(move || {
                for ordinal in 0..64 {
                    buffer.push((task, ordinal)).unwrap();
                }
            });
        }
    });
    let rows = buffer.into_rows().unwrap();
    assert_eq!(rows.len(), 256);
    let mut next = [0; 4];
    for (task, ordinal) in rows {
        assert_eq!(ordinal, next[task as usize]);
        next[task as usize] += 1;
    }
    assert_eq!(next, [64; 4]);
}

#[test]
fn ownership_buffer_ignored_overflow_cannot_publish_a_truncated_success() {
    let buffer = Buffer::new(2, 0);
    buffer.push(1).unwrap();
    buffer.push(2).unwrap();
    let _ = buffer.push(3);
    assert_eq!(buffer.push(4), Err(BufferFull));
    assert_eq!(buffer.into_rows(), Err(BufferFull));
}

#[test]
fn ownership_buffer_empty_execution_and_zero_capacity_are_distinct_from_lost_rows() {
    assert_eq!(Buffer::new(0, 0).into_rows(), Ok(vec![]));
    let buffer = Buffer::new(0, 0);
    assert_eq!(buffer.push(7), Err(BufferFull));
    assert_eq!(buffer.into_rows(), Err(BufferFull));
    let buffer = Buffer::new(2, 99);
    buffer.push(7).unwrap();
    assert_eq!(buffer.into_rows(), Ok(vec![7]));
}
