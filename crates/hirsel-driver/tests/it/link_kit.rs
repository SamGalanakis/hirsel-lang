//! A shipped executable is a pure function of its program and the toolchain:
//! `hirsel build` gives the same bytes, and with `--debug` the same
//! `<build-id>.debug`, on a repeat and from 1, 2 or 4 builds running at once,
//! on both targets, each build in its own scratch directory.
use hirsel_driver::native::{self, Backend, BuildProfile, TargetSpec};
use hirsel_engine::with_compiler_stack;
use std::path::{Path, PathBuf};

const PROGRAM: &str = "type Shape = Circle(u64) | Square(u64)

fn area(item: Shape) -> u64 {
  match item {
    Circle(r) => 3u64 * r * r,
    Square(s) => s * s,
  }
}

pub fn main(rt: Io) -> Result[(), GrantError] {
  let stdout = io.stdout(rt)?
  let shapes: List[Shape] = [Circle(2u64), Square(3u64)]
  let mut total = 0u64
  for item in shapes {
    total += area(item)
  }
  let _ = out.print(stdout, \"shapes\\n\")
  Ok(())
}
";

/// The executable's bytes and, for a debug build, its debug file's name and bytes.
type Built = (Vec<u8>, Option<(String, Vec<u8>)>);

fn build(source: &Path, dir: &Path, target: &TargetSpec, profile: BuildProfile) -> Built {
    std::fs::create_dir_all(dir).unwrap();
    let executable = dir.join("app");
    let source = source.to_path_buf();
    let target = target.clone();
    let built = with_compiler_stack(|| {
        native::build_path_native_backend(
            &source,
            &executable,
            target,
            profile,
            Backend::Llvm,
            None,
            false,
        )
    })
    .unwrap();
    assert!(built.is_ok(), "{built:?}");
    let debug = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|e| e == "debug"))
        .map(|path| {
            (
                path.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read(&path).unwrap(),
            )
        });
    assert_eq!(debug.is_some(), profile == BuildProfile::Debug);
    let bytes = std::fs::read(&executable).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    (bytes, debug)
}

#[test]
fn shipped_links_are_identical_on_repeat_and_under_concurrent_builds_on_both_targets() {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("link-kit-determinism");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let source = root.join("main.hir");
    std::fs::write(&source, PROGRAM).unwrap();
    for target in [
        TargetSpec::x86_64_unknown_linux_gnu(),
        TargetSpec::aarch64_unknown_linux_gnu(),
    ] {
        for profile in [BuildProfile::Default, BuildProfile::Debug] {
            let name = format!("{}-{profile:?}", target.triple);
            let reference = build(&source, &root.join(&name).join("first"), &target, profile);
            let again = build(&source, &root.join(&name).join("again"), &target, profile);
            assert!(again == reference, "{name}: a repeated build differs");
            for jobs in [1, 2, 4] {
                let builds: Vec<Built> = std::thread::scope(|scope| {
                    let handles: Vec<_> = (0..jobs)
                        .map(|job| {
                            let dir = root.join(&name).join(format!("jobs-{jobs}-{job}"));
                            let (source, target) = (&source, &target);
                            scope.spawn(move || build(source, &dir, target, profile))
                        })
                        .collect();
                    handles.into_iter().map(|h| h.join().unwrap()).collect()
                });
                for built in builds {
                    assert!(built == reference, "{name}: a build among {jobs} differs");
                }
            }
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
