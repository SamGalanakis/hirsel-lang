//! Runtime r6 Ordered cross-engine leg (runtime-ordered-50). The runtime unit
//! test of the same name checks the model and same-engine native matrices.
use super::ordered_qualification::{CALLED_BEFORE_SEND, corpus_backends};

/// Interpreter output, every LLVM and Cranelift live cell at widths 1/2/4/8
/// with and without forced-migration jitter, reorder off/auto and shared or
/// dedicated commit, and every immutable-input replay cell agree; the two
/// backends' canonical traces are byte-identical.
#[test]
#[ignore = "native Ordered matrix; run on the host to prove widths through eight"]
fn runtime50_ordered_serial_prefix_matrix() {
    for (program, expected, operations) in [
        (
            include_str!("../../../../conformance/CLO/CLO-3-child-atomic-capture-run.hir"),
            &b""[..],
            &["atomic.store", "task.end"][..],
        ),
        (
            CALLED_BEFORE_SEND,
            &b""[..],
            &["channel.send", "channel.receive", "task.end"][..],
        ),
    ] {
        let traces = corpus_backends(program, expected, &["llvm", "cranelift"]);
        assert_eq!(traces.len(), 2);
        assert_eq!(traces[0], traces[1], "LLVM and Cranelift canonical traces");
        let text = std::str::from_utf8(&traces[0]).unwrap();
        for operation in operations {
            assert!(text.contains(operation), "{operation} missing from {text}");
        }
    }
}
