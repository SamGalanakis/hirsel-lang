//! A run's Io tape is the same whichever engine records it. Each program here switches tasks
//! (spawns, joins, task sets, channels, mutexes, atomics, cancellation, finalizers), and under
//! `random@1` and `pct@1` every scheduling choice is a tape line, so a change to how the native
//! runtime switches tasks that moved any choice or any Io request would show as a byte
//! difference from the interpreter's tape, the sequential reference.
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The schedules: the default, and seeded policies whose every choice is recorded.
const SCHEDULERS: &[Option<(&str, u64)>] = &[None, Some(("random", 1)), Some(("pct", 2))];

/// The events a tape records: every Io request, its completion and every scheduling choice,
/// in order. The header before them names the launch (the host's admission clock, the run
/// record a launcher binds), which differs between `hirsel run` and an executable.
fn events(tape: &Path, what: &str, output: &std::process::Output) -> Vec<u8> {
    let bytes =
        std::fs::read(tape).unwrap_or_else(|e| panic!("{what} wrote no tape ({e}): {output:?}"));
    assert!(bytes.starts_with(hirsel_run_schema::tape::MAGIC));
    let header =
        hirsel_run_schema::tape::read(&bytes, hirsel_run_schema::tape::MAGIC.len(), 0).unwrap();
    // Compare exact binary event frames (including CRC and sequence), excluding
    // admission provenance that legitimately differs between live launches.
    bytes[header.range.end..].to_vec()
}

fn run(command: &mut Command, tape: &Path, what: &str) -> Vec<u8> {
    let _ = std::fs::remove_file(tape);
    let output = command
        .stdin(Stdio::null())
        .output()
        .unwrap_or_else(|e| panic!("{what}: {command:?}: {e}"));
    events(tape, what, &output)
}

fn check_program(path: &str) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir_scratch = tempfile::Builder::new()
        .prefix("hirsel-tape-identity-")
        .tempdir()
        .unwrap();
    let dir = dir_scratch.path().to_path_buf();
    let dir = dir.canonicalize().unwrap();
    let tape = dir.join("tape");
    let program = root.join(path).canonicalize().unwrap();
    // One executable per backend, run under every schedule.
    let executables: Vec<(&str, PathBuf)> = ["llvm", "cranelift"]
        .into_iter()
        .map(|backend| {
            let exe = dir.join(backend);
            let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
                .args(["build", "--backend", backend])
                .arg(&program)
                .arg("-o")
                .arg(&exe)
                .output()
                .unwrap();
            assert!(output.status.success(), "{path} {backend}: {output:?}");
            (backend, exe)
        })
        .collect();
    for scheduler in SCHEDULERS {
        let mut inputs = serde_json::json!({"hash_seed": 0, "random_seed": 0, "test_seed": 0});
        if let Some((algorithm, seed)) = scheduler {
            inputs["scheduler"] =
                serde_json::json!({"algorithm": algorithm, "version": 1, "seed": seed});
        }
        let grants = dir.join("grants.json");
        std::fs::write(
            &grants,
            serde_json::json!({
                "inputs": inputs,
                "io": {"stdout": "stdout.bytes@1", "tasks": "tasks.cooperative@1", "dirs": []},
                "args": [],
            })
            .to_string(),
        )
        .unwrap();
        let what = format!("{path} {scheduler:?}");
        let reference = run(
            Command::new(env!("CARGO_BIN_EXE_hirsel"))
                .arg("run")
                .arg(&program)
                .arg("--grants")
                .arg(&grants)
                .arg("--record")
                .arg(&tape),
            &tape,
            &format!("{what} interpreted"),
        );
        for (backend, exe) in &executables {
            let native = run(
                Command::new(exe)
                    .arg("--hirsel-record")
                    .arg(&tape)
                    .arg("--hirsel-grants")
                    .arg(&grants),
                &tape,
                &format!("{what} {backend}"),
            );
            assert_eq!(
                native, reference,
                "{what}: {backend} against the interpreter"
            );
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

macro_rules! tape_case {
    ($name:ident, $path:literal) => {
        #[test]
        fn $name() {
            check_program($path);
        }
    };
}

tape_case!(
    every_engine_records_the_same_tape_under_every_schedule_clo_3_child_atomic_capture_run,
    "conformance/CLO/CLO-3-child-atomic-capture-run.hir"
);
tape_case!(
    every_engine_records_the_same_tape_under_every_schedule_clo_3_child_mutex_capture_run,
    "conformance/CLO/CLO-3-child-mutex-capture-run.hir"
);
tape_case!(
    every_engine_records_the_same_tape_under_every_schedule_clo_4_scope_body_takes_let_mut_list_run,
    "conformance/CLO/CLO-4-scope-body-takes-let-mut-list-run.hir"
);
tape_case!(every_engine_records_the_same_tape_under_every_schedule_clo_4_scope_body_takes_let_mut_reassigned_run, "conformance/CLO/CLO-4-scope-body-takes-let-mut-reassigned-run.hir");
tape_case!(
    every_engine_records_the_same_tape_under_every_schedule_conc_2_child_resource_drop_run,
    "conformance/CONC/CONC-2-child-resource-drop-run.hir"
);
tape_case!(
    every_engine_records_the_same_tape_under_every_schedule_conc_2_detached_result_cleanup_run,
    "conformance/CONC/CONC-2-detached-result-cleanup-run.hir"
);
tape_case!(
    every_engine_records_the_same_tape_under_every_schedule_conc_2_error_result_cancels_run,
    "conformance/CONC/CONC-2-error-result-cancels-run.hir"
);
tape_case!(
    every_engine_records_the_same_tape_under_every_schedule_conc_2_option_result_joins_run,
    "conformance/CONC/CONC-2-option-result-joins-run.hir"
);
tape_case!(
    every_engine_records_the_same_tape_under_every_schedule_conc_2_taken_block_sender_capture_run,
    "conformance/CONC/CONC-2-taken-block-sender-capture-run.hir"
);
tape_case!(
    every_engine_records_the_same_tape_under_every_schedule_conc_2_taken_direct_sender_capture_run,
    "conformance/CONC/CONC-2-taken-direct-sender-capture-run.hir"
);
tape_case!(
    every_engine_records_the_same_tape_under_every_schedule_conc_2_task_all_failing_child_run,
    "conformance/CONC/CONC-2-task-all-failing-child-run.hir"
);
tape_case!(
    every_engine_records_the_same_tape_under_every_schedule_conc_2_task_all_ok_run,
    "conformance/CONC/CONC-2-task-all-ok-run.hir"
);
tape_case!(
    every_engine_records_the_same_tape_under_every_schedule_conc_2_task_select_fast_slow_run,
    "conformance/CONC/CONC-2-task-select-fast-slow-run.hir"
);
tape_case!(
    every_engine_records_the_same_tape_under_every_schedule_conc_2_task_set_detach_all_run,
    "conformance/CONC/CONC-2-task-set-detach-all-run.hir"
);
tape_case!(
    every_engine_records_the_same_tape_under_every_schedule_conc_2_task_set_first_completed_run,
    "conformance/CONC/CONC-2-task-set-first-completed-run.hir"
);
tape_case!(
    every_engine_records_the_same_tape_under_every_schedule_conc_2_task_set_scope_body_capture_run,
    "conformance/CONC/CONC-2-task-set-scope-body-capture-run.hir"
);
tape_case!(
    every_engine_records_the_same_tape_under_every_schedule_conc_2_task_take_capture_freeze_run,
    "conformance/CONC/CONC-2-task-take-capture-freeze-run.hir"
);
tape_case!(
    every_engine_records_the_same_tape_under_every_schedule_conc_3_cancellation_shields_drop_run,
    "conformance/CONC/CONC-3-cancellation-shields-drop-run.hir"
);
tape_case!(
    every_engine_records_the_same_tape_under_every_schedule_conc_3_check_canceled_run,
    "conformance/CONC/CONC-3-check-canceled-run.hir"
);
tape_case!(
    every_engine_records_the_same_tape_under_every_schedule_conc_3_shielded_drop_send_run,
    "conformance/CONC/CONC-3-shielded-drop-send-run.hir"
);
tape_case!(
    every_engine_records_the_same_tape_under_every_schedule_conc_5_counted_payload_update_run,
    "conformance/CONC/CONC-5-counted-payload-update-run.hir"
);
tape_case!(
    every_engine_records_the_same_tape_under_every_schedule_conc_5_owned_payload_mutexes_run,
    "conformance/CONC/CONC-5-owned-payload-mutexes-run.hir"
);
tape_case!(
    every_engine_records_the_same_tape_under_every_schedule_conc_6_owned_record_payload_run,
    "conformance/CONC/CONC-6-owned-record-payload-run.hir"
);
tape_case!(
    every_engine_records_the_same_tape_under_every_schedule_conc_6_receiver_drops_buffer_run,
    "conformance/CONC/CONC-6-receiver-drops-buffer-run.hir"
);
tape_case!(
    every_engine_records_the_same_tape_under_every_schedule_conc_6_suspended_receiver_run,
    "conformance/CONC/CONC-6-suspended-receiver-run.hir"
);
tape_case!(
    every_engine_records_the_same_tape_under_every_schedule_conc_8_await_channel_cycle_run,
    "conformance/CONC/CONC-8-await-channel-cycle-run.hir"
);
tape_case!(
    every_engine_records_the_same_tape_under_every_schedule_conc_8_held_sender_run,
    "conformance/CONC/CONC-8-held-sender-run.hir"
);
tape_case!(
    every_engine_records_the_same_tape_under_every_schedule_conc_8_two_task_channel_run,
    "conformance/CONC/CONC-8-two-task-channel-run.hir"
);
tape_case!(
    every_engine_records_the_same_tape_under_every_schedule_fail_1_track_caller_location_run,
    "conformance/FAIL/FAIL-1-track-caller-location-run.hir"
);
tape_case!(
    every_engine_records_the_same_tape_under_every_schedule_fail_1_track_caller_task_run,
    "conformance/FAIL/FAIL-1-track-caller-task-run.hir"
);
tape_case!(
    every_engine_records_the_same_tape_under_every_schedule_fail_3_track_caller_cleanup_run,
    "conformance/FAIL/FAIL-3-track-caller-cleanup-run.hir"
);
tape_case!(
    every_engine_records_the_same_tape_under_every_schedule_fail_4_owned_payload_dead_run,
    "conformance/FAIL/FAIL-4-owned-payload-dead-run.hir"
);
tape_case!(
    every_engine_records_the_same_tape_under_every_schedule_mem_11_with_tasks_run,
    "conformance/MEM/MEM-11-with-tasks-run.hir"
);
tape_case!(
    every_engine_records_the_same_tape_under_every_schedule_mem_3_borrowed_call_compare_run,
    "conformance/MEM/MEM-3-borrowed-call-compare-run.hir"
);
