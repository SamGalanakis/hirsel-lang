//! Create packages using the ordinary schema-1 manifest and explicit launch grants.
use hirsel_cli_args::NewOptions;
use hirsel_engine::ToolError;

const TEST: &str = "\n@test\nfn greeting_is_not_empty(t: Test) {\n  test.check(t, string.byte_len(greeting()) > 0)\n}\n";
const LIBRARY: &str = "pub fn greeting() -> String {\n  \"Hello, Hirsel!\"\n}\n";
const MAIN: &str = "\npub type AppError = Grant(GrantError) | Output(OutputError)\n\npub fn main(rt: Io) -> Result[(), AppError] {\n  let output = io.stdout(rt) ? Grant\n  out.print(output, f\"{greeting()}\\n\") ? Output\n  Ok(())\n}\n";

pub fn create(options: &NewOptions) -> Result<(), ToolError> {
    let directory = &options.directory;
    // Resolve everything that can fail before creating files.
    let name = directory
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| ToolError::Runtime("new requires a directory name that is a package identifier (letters, digits and underscores)".into()))?;
    hirsel_engine::validate_package_name(name)?;
    let std = hirsel_engine::toolchain_std_manifest()?;
    if directory.try_exists()? && std::fs::read_dir(directory)?.next().is_some() {
        return Err(ToolError::Runtime(format!(
            "new refuses non-empty directory {}",
            directory.display()
        )));
    }
    let mut manifest = serde_json::json!({
        "schema": 1,
        "package": {"name": name, "version": "0"},
        "sources": [{"path":"main.hir","module":[]}],
        "dependencies": [{"manifest":std,"package":{"name":"std","version":"1"},"expose":[]}],
    });
    if !options.library {
        manifest["entry"] = serde_json::json!({"module":[],"name":"main"});
    }
    let source = format!("{LIBRARY}{}{TEST}", if options.library { "" } else { MAIN });
    std::fs::create_dir_all(directory)?;
    // create_new also protects against replacing a file created after the emptiness check.
    for (name, contents) in [
        (
            "hirsel.json",
            format!("{}\n", serde_json::to_string_pretty(&manifest).unwrap()),
        ),
        ("main.hir", source),
        (
            "grants.json",
            "{\"io\":{\"stdout\":\"stdout.bytes@1\",\"dirs\":[]}}\n".into(),
        ),
    ] {
        use std::io::Write;
        std::fs::File::options()
            .write(true)
            .create_new(true)
            .open(directory.join(name))?
            .write_all(contents.as_bytes())?;
    }
    Ok(())
}
