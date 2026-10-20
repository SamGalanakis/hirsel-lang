// Use the app's Unicode16 tables, rather than silently changing its accepted
// tags and search semantics with a different library's Unicode version.
use std::{env, fs, path::PathBuf};
fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let repository = root.ancestors().nth(4).unwrap();
    let mut generated = String::new();
    for (file, names) in [
        (
            "string_case_tables.hir",
            &[
                "FULL_LOWER",
                "FULL_LOWER_DATA",
                "CASEFOLD",
                "CASEFOLD_DATA",
                "CASED",
                "CASE_IGNORABLE",
            ][..],
        ),
        ("unicode_property_tables.hir", &["LETTERS", "DIGITS"][..]),
    ] {
        let path = repository.join("stdlib/std@1").join(file);
        println!("cargo:rerun-if-changed={}", path.display());
        let source = fs::read_to_string(path).unwrap();
        for name in names {
            let prefix = format!("const {name}: Bytes = ");
            let literal = source
                .lines()
                .find_map(|line| line.strip_prefix(&prefix))
                .unwrap();
            generated.push_str(&format!("const {name}: &[u8] = {literal};\n"));
        }
    }
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("unicode16.rs"),
        generated,
    )
    .unwrap();
}
