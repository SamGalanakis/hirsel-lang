//! Complete file -> std CSV -> task.map_all -> ordered reduction workflows.
#[path = "../support/csvreduce_27/cases.rs"]
mod cases;
#[path = "../support/csvreduce_27/matrix.rs"]
mod matrix;

#[test]
fn csvreduce_27_empty_and_signed() {
    matrix::partition(0);
}
#[test]
fn csvreduce_27_csv_dialect() {
    matrix::partition(1);
}
#[test]
fn csvreduce_27_negative_admission() {
    matrix::partition(2);
}
#[test]
fn csvreduce_27_record_cap() {
    matrix::partition(3);
}
#[test]
fn csvreduce_27_tail_and_sixteen_chunks() {
    matrix::partition(4);
}
#[test]
fn csvreduce_27_rayon_equivalence() {
    matrix::partition(5);
}

#[path = "../support/csvreduce_27/cli.rs"]
mod cli;

#[test]
fn csvreduce_27_cli_file_and_replay() {
    cli::file_and_replay();
}

#[path = "../support/csvreduce_27/production.rs"]
mod production;

#[test]
fn csvreduce_27_production_ordered_replay() {
    production::ordered_replay();
}
