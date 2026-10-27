use super::*;

#[test]
fn ordered_home_reads_live_span_context_from_the_shared_provider() {
    let mut real = RealIo::grant(std::path::Path::new(".")).unwrap();
    real.set_telemetry(crate::telemetry::Capture::default());
    let shared = std::sync::Arc::new(real.into_concurrent().ok().unwrap());
    let mut home = OrderedHome {
        io: shared.clone(),
        current: crate::io_real::HOME_TOKEN,
    };
    assert_eq!(
        home.call(Request::Telemetry { io: 1 }),
        Ok(Completion::Handle(Ok(2)))
    );
    let mut parent = vec![0; 24];
    parent.extend_from_slice(&[1, 7, 7]);
    let Completion::Span {
        handle,
        trace,
        span,
    } = home
        .call(Request::TelemetryBegin {
            telemetry: 2,
            parent,
            spec: b"spec".to_vec(),
        })
        .unwrap()
    else {
        panic!("a telemetry begin must return a span");
    };
    let mut expected = trace.to_vec();
    expected.extend_from_slice(&span);
    expected.extend_from_slice(&[1, 7, 7]);
    let tape = shared.tape();
    assert_eq!(home.telemetry_context(2, handle), Some(expected));
    assert_eq!(home.telemetry_context(1, handle), None);
    assert_eq!(home.telemetry_context(2, handle + 1), None);
    assert_eq!(shared.tape(), tape, "context reads issue no operation");
    home.call(Request::Close { handle }).unwrap();
    assert_eq!(home.telemetry_context(2, handle), None);
}
