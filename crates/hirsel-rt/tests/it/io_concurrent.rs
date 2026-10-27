//! Real-adapter claim/withdraw/quarantine controls over the exhaustive Request inventory.
use hirsel_free::claim::{Claim, Requests};
use hirsel_rt::io::{AdapterError, Request};
use hirsel_rt::io_concurrent::ConcurrentAdapter;
use hirsel_rt::io_real::RealIo;
use std::sync::Arc;

#[test]
fn every_real_request_obeys_claim_cut_at_widths_one_and_eight() {
    let specimens = super::tape_property::requests(0);
    assert!(
        specimens.len() > 100,
        "empty or truncated operation inventory"
    );
    for width in [1, 8] {
        let temp = tempfile::tempdir().unwrap();
        let io = Arc::new(
            RealIo::grant(temp.path())
                .unwrap()
                .into_concurrent()
                .unwrap_or_else(|_| panic!("Send streams")),
        );
        std::thread::scope(|scope| {
            for worker in 0..width {
                let specimens = &specimens;
                let io = &io;
                scope.spawn(move || {
                    for (index, request) in specimens
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| i % width == worker)
                    {
                        // Cut wins first: no request may reach even argument validation.
                        let table = Requests::default();
                        let record = table.prepare(index as u64, request.clone());
                        assert_eq!(table.sweep().withdrawn, [index as u64]);
                        assert!(!record.claim());
                        assert_eq!(record.state(), Claim::Withdrawn);
                        // Claim wins first: the entire owned request remains available after
                        // the cut. Its completion is quarantined; no result is delivered.
                        let table = Requests::default();
                        let record = table.prepare(index as u64, request.clone());
                        assert!(record.claim());
                        assert!(!record.claim(), "duplicate issue");
                        assert_eq!(table.sweep().in_doubt, [index as u64]);
                        let quarantined = io.call_at(worker as u64 + 1, record.buffers().clone());
                        if matches!(
                            request,
                            Request::Ready { .. }
                                | Request::Schedule { .. }
                                | Request::CpuControl { .. }
                                | Request::ProcessTime { .. }
                                | Request::ProcessSettle { .. }
                        ) {
                            assert_eq!(quarantined, Err(AdapterError::InvalidCompletion));
                            assert_eq!(io.preflight(request), Err(AdapterError::InvalidCompletion));
                        }
                        record.finish();
                        assert_eq!(record.state(), Claim::Done);
                    }
                });
            }
        });
        assert!(temp.path().read_dir().unwrap().next().is_none());
        assert!(io.stdout().is_empty());
        assert!(io.stderr().is_empty());
    }
}
