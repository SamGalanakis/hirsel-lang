use std::{env, path::PathBuf, process::Command};
fn main() {
    let prefix =
        PathBuf::from(env::var_os("LLVM_SYS_231_PREFIX").expect("prepare the pinned LLVM tree"));
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let sources = ["stack_size", "count_loops"];
    let sysroot = prefix.join("sysroot");
    assert!(
        sysroot.join("usr/include/c++/13/vector").is_file(),
        "run python3 tools/buck2/prepare.py for hermetic C++ headers"
    );
    for name in sources {
        let source = manifest.join(format!("bridge/{name}.cpp"));
        println!("cargo:rerun-if-changed={}", source.display());
        let status = Command::new(prefix.join("bin/hirsel-clang++"))
            .args([
                "-std=c++17",
                "-fPIC",
                "-fno-exceptions",
                "-fno-rtti",
                "-nostdinc++",
            ])
            .arg(format!("--sysroot={}", sysroot.display()))
            .arg("-isystem")
            .arg(sysroot.join("usr/include/c++/13"))
            .arg("-isystem")
            .arg(sysroot.join("usr/include/x86_64-linux-gnu/c++/13"))
            .arg("-I")
            .arg(prefix.join("include"))
            .arg("-c")
            .arg(source)
            .arg("-o")
            .arg(out.join(format!("{name}.o")))
            .status()
            .unwrap();
        assert!(status.success(), "LLVM option bridge compilation");
    }
    assert!(
        Command::new(prefix.join("bin/hirsel-llvm-ar"))
            .arg("crs")
            .arg(out.join("libhirsel_llvm_bridge.a"))
            .args(sources.map(|name| out.join(format!("{name}.o"))))
            .status()
            .unwrap()
            .success()
    );
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=hirsel_llvm_bridge");
    println!("cargo:rustc-link-lib=dylib:+verbatim=libstdc++.so.6");
}
