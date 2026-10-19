//! The `ld.lld` command a compiler driver would run, derived once per toolchain.
//!
//! Executables link with `ld.lld` directly. Its arguments are the ones the
//! toolchain's compiler driver passes its linker: `<driver> -### <flags> <inputs>
//! -o <output>` prints that command, with placeholder inputs and output that each
//! link fills in. The command is kept in the runtime cache with the filesystem
//! facts the driver's choice depended on: every absolute path it names (crt
//! objects, library directories, the sysroot) and each one's parent directory,
//! as present or absent with its identity. A link reuses the command while every
//! fact still holds and asks the driver again otherwise, so a changed or newly
//! installed GCC, libc or sysroot is seen. The driver's own executable and its
//! directory are facts too.
use super::stages::{NativeError, StageName};
use hirsel_syntax::ContentHash;
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

const MAGIC: &[u8] = b"hirsel.link-line.v1\n";
/// A cached command is a few kilobytes; anything larger is not one.
const LIMIT: u64 = 1 << 20;

#[derive(Clone, Debug, Eq, PartialEq)]
enum Arg {
    Literal(OsString),
    Inputs,
    Output,
}

/// A path's identity as the driver saw it, or its absence.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Fact {
    path: PathBuf,
    identity: Option<[u64; 7]>,
}
impl Fact {
    fn of(path: &Path) -> Self {
        let identity = std::fs::metadata(path).ok().map(|m| {
            [
                m.dev(),
                m.ino(),
                m.size(),
                m.mtime() as u64,
                m.mtime_nsec() as u64,
                m.ctime() as u64,
                m.ctime_nsec() as u64,
            ]
        });
        Self {
            path: path.to_path_buf(),
            identity,
        }
    }
    fn holds(&self) -> bool {
        Self::of(&self.path) == *self
    }
}

/// The linker command of one toolchain, target and flag set.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LinkLine {
    args: Vec<Arg>,
    facts: Vec<Fact>,
}

impl LinkLine {
    /// `lld` linking `inputs` into `output`, with the tree's libraries on the
    /// loader path as the tree's own wrappers run it.
    pub(crate) fn command(&self, lld: &Path, inputs: &[OsString], output: &Path) -> Command {
        let mut command = Command::new(lld);
        for arg in &self.args {
            match arg {
                Arg::Literal(arg) => command.arg(arg),
                Arg::Inputs => command.args(inputs),
                Arg::Output => command.arg(output),
            };
        }
        if let Some(tree) = lld.parent().and_then(Path::parent) {
            let mut path = OsString::from(tree.join("lib"));
            if let Some(existing) = std::env::var_os("LD_LIBRARY_PATH") {
                path.push(":");
                path.push(existing);
            }
            command.env("LD_LIBRARY_PATH", path);
        }
        command
    }

    /// The command `driver` runs to link `before`, the inputs and `after`: this
    /// process's or the cache's while its facts hold, else the driver's. The
    /// cache only saves driver runs: a directory that is not private is not
    /// read or written.
    pub(crate) fn of(
        cache: &Path,
        driver: &Path,
        before: &[OsString],
        after: &[OsString],
    ) -> Result<Self, NativeError> {
        static LINES: Mutex<BTreeMap<String, LinkLine>> = Mutex::new(BTreeMap::new());
        let key = key(driver, before, after);
        let remembered = LINES.lock().unwrap().get(&key).cloned();
        if let Some(line) = remembered.filter(Self::holds) {
            return Ok(line);
        }
        let line = Self::cached(cache, &key, driver, before, after)?;
        LINES.lock().unwrap().insert(key, line.clone());
        Ok(line)
    }

    fn holds(&self) -> bool {
        self.facts.iter().all(Fact::holds)
    }

    /// The cache's line for `key` while its facts hold, else the driver's.
    fn cached(
        cache: &Path,
        key: &str,
        driver: &Path,
        before: &[OsString],
        after: &[OsString],
    ) -> Result<Self, NativeError> {
        let name = format!("{key}.link");
        if let Some(line) = super::runtime_cache::read_private(cache, &name, LIMIT)
            .and_then(|bytes| Self::decode(&bytes))
            .filter(Self::holds)
        {
            return Ok(line);
        }
        let line = Self::derive(driver, before, after)?;
        let _ = super::runtime_cache::write_private(cache, &name, &line.encode());
        Ok(line)
    }

    fn derive(driver: &Path, before: &[OsString], after: &[OsString]) -> Result<Self, NativeError> {
        let scratch = super::executable::Scratch::new()?;
        let inputs = scratch.0.join("hirsel-link-inputs.o");
        let output = scratch.0.join("hirsel-link-output");
        std::fs::write(&inputs, b"").map_err(|e| failed(format!("{}: {e}", inputs.display())))?;
        let mut command = Command::new(driver);
        command
            .arg("-###")
            .args(before)
            .arg(&inputs)
            .args(after)
            .arg("-o")
            .arg(&output);
        let printed = command
            .output()
            .map_err(|e| failed(format!("could not run {command:?}: {e}")))?;
        if !printed.status.success() {
            return Err(failed(format!(
                "{command:?} failed with {}: {}",
                printed.status,
                String::from_utf8_lossy(&printed.stderr)
            )));
        }
        let mut facts = vec![Fact::of(driver)];
        if let Some(parent) = driver.parent() {
            facts.push(Fact::of(parent));
        }
        parse(&printed.stderr, &inputs, &output, facts).ok_or_else(|| {
            failed(format!(
                "{command:?} printed no single linker command: {}",
                String::from_utf8_lossy(&printed.stderr)
            ))
        })
    }

    fn encode(&self) -> Vec<u8> {
        let mut bytes = MAGIC.to_vec();
        let mut field = |tag: u8, value: &[u8]| {
            bytes.push(tag);
            bytes.extend((value.len() as u64).to_le_bytes());
            bytes.extend(value);
        };
        for arg in &self.args {
            match arg {
                Arg::Literal(arg) => field(b'A', arg.as_bytes()),
                Arg::Inputs => field(b'I', b""),
                Arg::Output => field(b'O', b""),
            }
        }
        for fact in &self.facts {
            field(b'P', fact.path.as_os_str().as_bytes());
            match fact.identity {
                Some(identity) => field(
                    b'S',
                    &identity
                        .iter()
                        .flat_map(|n| n.to_le_bytes())
                        .collect::<Vec<_>>(),
                ),
                None => field(b'N', b""),
            }
        }
        bytes
    }

    fn decode(bytes: &[u8]) -> Option<Self> {
        let mut rest = bytes.strip_prefix(MAGIC)?;
        let mut args = Vec::new();
        let mut facts = Vec::new();
        let mut path = None;
        while let Some((&tag, tail)) = rest.split_first() {
            let length = u64::from_le_bytes(tail.get(..8)?.try_into().ok()?);
            let value = tail.get(8..8usize.checked_add(usize::try_from(length).ok()?)?)?;
            rest = &tail[8 + value.len()..];
            match tag {
                b'A' => args.push(Arg::Literal(OsString::from_vec(value.to_vec()))),
                b'I' => args.push(Arg::Inputs),
                b'O' => args.push(Arg::Output),
                b'P' if path.is_none() => {
                    path = Some(PathBuf::from(OsString::from_vec(value.to_vec())))
                }
                b'S' | b'N' => {
                    let identity = if tag == b'S' {
                        let words: Vec<u64> = value
                            .chunks_exact(8)
                            .map(|w| u64::from_le_bytes(w.try_into().unwrap()))
                            .collect();
                        Some(<[u64; 7]>::try_from(words).ok()?)
                    } else {
                        None
                    };
                    facts.push(Fact {
                        path: path.take()?,
                        identity,
                    });
                }
                _ => return None,
            }
        }
        let holes = |hole: Arg| args.iter().filter(|a| **a == hole).count();
        (path.is_none() && holes(Arg::Inputs) == 1 && holes(Arg::Output) == 1)
            .then_some(Self { args, facts })
    }
}

fn failed(message: String) -> NativeError {
    NativeError::new(StageName::Link, format!("the linker command: {message}"))
}

/// The variables a GCC or clang driver reads when it chooses linker arguments.
const DRIVER_ENVIRONMENT: &[&str] = &[
    "LIBRARY_PATH",
    "COMPILER_PATH",
    "GCC_EXEC_PREFIX",
    "CCC_OVERRIDE_OPTIONS",
    "CLANG_NO_DEFAULT_CONFIG",
];

fn key(driver: &Path, before: &[OsString], after: &[OsString]) -> String {
    let mut parts: Vec<&[u8]> = vec![driver.as_os_str().as_bytes(), b"before"];
    parts.extend(before.iter().map(|a| a.as_bytes()));
    parts.push(b"after");
    parts.extend(after.iter().map(|a| a.as_bytes()));
    let environment: Vec<(&str, Option<OsString>)> = DRIVER_ENVIRONMENT
        .iter()
        .map(|name| (*name, std::env::var_os(name)))
        .collect();
    for (name, value) in &environment {
        parts.push(name.as_bytes());
        parts.push(
            value
                .as_deref()
                .map_or(b"\0unset" as &[u8], OsStr::as_bytes),
        );
    }
    let hash = ContentHash::of_parts("hirsel.link-line.v1", parts.into_iter());
    hash.0.iter().map(|b| format!("{b:02x}")).collect()
}

/// The words of one `-###` command line: double-quoted words with `\` escapes,
/// or bare words, separated by spaces.
fn words(line: &[u8]) -> Option<Vec<Vec<u8>>> {
    let mut words = Vec::new();
    let mut bytes = line.iter().copied().peekable();
    loop {
        while bytes.next_if_eq(&b' ').is_some() {}
        let Some(first) = bytes.next() else {
            return Some(words);
        };
        let mut word = Vec::new();
        if first == b'"' {
            loop {
                match bytes.next()? {
                    b'"' => break,
                    b'\\' => word.push(bytes.next()?),
                    byte => word.push(byte),
                }
            }
        } else {
            word.push(first);
            while let Some(byte) = bytes.next_if(|b| *b != b' ') {
                word.push(byte);
            }
        }
        words.push(word);
    }
}

/// The linker command among a driver's `-###` lines: the last command line (each
/// begins with a space). A GCC driver runs `collect2`, whose LTO plugin options
/// `ld.lld` does not take; this link has no LTO inputs, so they are dropped.
fn parse(printed: &[u8], inputs: &Path, output: &Path, mut facts: Vec<Fact>) -> Option<LinkLine> {
    let line = printed
        .split(|b| *b == b'\n')
        .filter(|line| line.starts_with(b" "))
        .last()?;
    let mut words = words(line)?.into_iter();
    let linker = words.next()?;
    let collect2 = linker.ends_with(b"/collect2") || linker == b"collect2";
    let mut args = Vec::new();
    while let Some(word) = words.next() {
        if collect2 && word == b"-plugin" {
            words.next()?;
            continue;
        }
        if collect2 && word.starts_with(b"-plugin-opt=") {
            continue;
        }
        let word = OsString::from_vec(word);
        args.push(if word == inputs.as_os_str() {
            Arg::Inputs
        } else if word == output.as_os_str() {
            Arg::Output
        } else {
            Arg::Literal(word)
        });
    }
    let named: std::collections::BTreeSet<PathBuf> = args
        .iter()
        .filter_map(|arg| match arg {
            Arg::Literal(arg) => named_path(arg.as_bytes()),
            _ => None,
        })
        .flat_map(|path| {
            let parent = path.parent().map(Path::to_path_buf);
            std::iter::once(path).chain(parent)
        })
        .collect();
    facts.extend(named.iter().map(|path| Fact::of(path)));
    let holes = |hole: Arg| args.iter().filter(|a| **a == hole).count();
    (holes(Arg::Inputs) == 1 && holes(Arg::Output) == 1).then_some(LinkLine { args, facts })
}

/// The absolute host path an argument names, after a `-L` or `--sysroot=` prefix.
fn named_path(arg: &[u8]) -> Option<PathBuf> {
    let path = arg
        .strip_prefix(b"-L")
        .or_else(|| arg.strip_prefix(b"--sysroot="))
        .unwrap_or(arg);
    path.starts_with(b"/")
        .then(|| PathBuf::from(OsString::from_vec(path.to_vec())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn root(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("hirsel-link-line-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    /// A driver that prints a clang-style `-###` link command naming `crt` and
    /// counts its runs in `runs`.
    fn driver(root: &Path, crt: &Path) -> PathBuf {
        let driver = root.join("bin/cc");
        std::fs::create_dir_all(driver.parent().unwrap()).unwrap();
        let script = [
            "#!/bin/sh".to_string(),
            format!("echo run >> {}", root.join("runs").display()),
            "for a; do out=\"$a\"; case \"$a\" in *.o) in=\"$a\";; esac; done".into(),
            "cat >&2 <<EOF".into(),
            "clang version 0".into(),
            format!(
                " \"/tree/ld.lld\" \"-pie\" \"-o\" \"$out\" \"{}\" \"-L{}\" \"$in\" \"-lc\" \"quote\\\"d\"",
                crt.display(),
                crt.parent().unwrap().display()
            ),
            "EOF".into(),
        ];
        std::fs::write(&driver, script.join("\n") + "\n").unwrap();
        std::fs::set_permissions(&driver, std::fs::Permissions::from_mode(0o755)).unwrap();
        driver
    }

    fn runs(root: &Path) -> usize {
        std::fs::read_to_string(root.join("runs")).map_or(0, |s| s.lines().count())
    }

    #[test]
    fn a_link_line_is_reused_until_a_path_the_driver_named_changes() {
        let root = root("reuse");
        let lib = root.join("gcc/13");
        std::fs::create_dir_all(&lib).unwrap();
        let crt = lib.join("crtbeginS.o");
        std::fs::write(&crt, b"crt").unwrap();
        let driver = driver(&root, &crt);
        let cache = root.join("cache");
        let flags = [OsString::from("-lm")];
        let cached = |flags: &[OsString]| {
            LinkLine::cached(&cache, &key(&driver, &[], flags), &driver, &[], flags).unwrap()
        };
        let first = cached(&flags);
        assert_eq!(runs(&root), 1);
        let command = first.command(
            Path::new("/tree/bin/ld.lld"),
            &["a.o".into(), "b.a".into()],
            Path::new("/out/program"),
        );
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(
            args,
            [
                "-pie",
                "-o",
                "/out/program",
                crt.to_str().unwrap(),
                &format!("-L{}", lib.display()),
                "a.o",
                "b.a",
                "-lc",
                "quote\"d",
            ]
            .map(OsStr::new)
        );
        // Unchanged facts: the cached line, without the driver.
        assert_eq!(cached(&flags), first);
        assert_eq!(runs(&root), 1);
        // Other flags are another line.
        cached(&[]);
        assert_eq!(runs(&root), 2);
        // A named object rewritten in place, or a new sibling installation the
        // driver might prefer, makes the driver choose again.
        std::fs::write(&crt, b"new crt").unwrap();
        cached(&flags);
        assert_eq!(runs(&root), 3);
        cached(&flags);
        assert_eq!(runs(&root), 3);
        std::fs::create_dir(root.join("gcc/14")).unwrap();
        cached(&flags);
        assert_eq!(runs(&root), 4);
        // A corrupt or foreign cache entry is derived again, never used.
        let name = format!("{}.link", key(&driver, &[], &flags));
        std::fs::write(cache.join(&name), b"hirsel.link-line.v1\nA").unwrap();
        let again = cached(&flags);
        assert_eq!((again.args, runs(&root)), (first.args.clone(), 5));
        std::fs::set_permissions(cache.join(&name), std::fs::Permissions::from_mode(0o644))
            .unwrap();
        cached(&flags);
        assert_eq!(runs(&root), 6);
        cached(&flags);
        assert_eq!(runs(&root), 6);
        // This process remembers the line, so a cache directory that is not
        // private, which is never read or written, costs one driver run.
        let shared = root.join("shared");
        std::fs::create_dir(&shared).unwrap();
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o755)).unwrap();
        let remembered = [OsString::from("-lremembered")];
        LinkLine::of(&shared, &driver, &[], &remembered).unwrap();
        LinkLine::of(&shared, &driver, &[], &remembered).unwrap();
        assert_eq!(runs(&root), 7);
        assert_eq!(std::fs::read_dir(&shared).unwrap().count(), 0);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn gcc_lines_drop_the_lto_plugin_and_keep_everything_else() {
        let printed = b"Using built-in specs.\n /arm/libexec/collect2 -plugin /arm/liblto_plugin.so \"-plugin-opt=/arm/lto-wrapper\" \"-plugin-opt=-fresolution=/tmp/cc.res\" \"--sysroot=/arm/libc\" -X -EL -maarch64linux -o /s/out /arm/libc/usr/lib/crt1.o /s/in.o -lgcc --push-state --as-needed -lgcc_s --pop-state\nCOLLECT_GCC_OPTIONS='-o'\n";
        let line = parse(
            printed,
            Path::new("/s/in.o"),
            Path::new("/s/out"),
            Vec::new(),
        )
        .unwrap();
        let literal = |s: &str| Arg::Literal(s.into());
        assert_eq!(
            line.args,
            [
                literal("--sysroot=/arm/libc"),
                literal("-X"),
                literal("-EL"),
                literal("-maarch64linux"),
                literal("-o"),
                Arg::Output,
                literal("/arm/libc/usr/lib/crt1.o"),
                Arg::Inputs,
                literal("-lgcc"),
                literal("--push-state"),
                literal("--as-needed"),
                literal("-lgcc_s"),
                literal("--pop-state"),
            ]
        );
        let paths: Vec<_> = line
            .facts
            .iter()
            .map(|f| f.path.to_str().unwrap())
            .collect();
        assert_eq!(
            paths,
            [
                "/arm",
                "/arm/libc",
                "/arm/libc/usr/lib",
                "/arm/libc/usr/lib/crt1.o"
            ]
        );
        // Without exactly one input and one output hole there is no line.
        assert!(
            parse(
                printed,
                Path::new("/other.o"),
                Path::new("/s/out"),
                Vec::new()
            )
            .is_none()
        );
        assert_eq!(LinkLine::decode(&line.encode()), Some(line));
    }
}
