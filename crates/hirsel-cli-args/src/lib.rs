//! Command-line mechanics shared by the CLI and compiler tools.

#[cfg(test)]
hirsel_guard::install!();

pub use lexopt;
use lexopt::{Arg, Error, Parser};
use std::path::PathBuf;

/// Parse errors are tooling errors, never language diagnostics.
pub fn parse_or_exit<T>(result: Result<T, Error>) -> T {
    result.unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(2)
    })
}

/// Arguments for creating a package; paths preserve the operating system spelling.
pub struct NewOptions {
    pub directory: PathBuf,
    pub library: bool,
}
impl NewOptions {
    pub fn parse(parser: &mut Parser) -> Result<Self, Error> {
        let (mut directory, mut library) = (None, false);
        while let Some(arg) = parser.next()? {
            match arg {
                Arg::Long("lib") if !library => library = true,
                Arg::Value(path) if directory.is_none() => directory = Some(PathBuf::from(path)),
                arg => return Err(arg.unexpected()),
            }
        }
        Ok(Self {
            directory: directory.ok_or("new requires a directory")?,
            library,
        })
    }
}

/// Values are optional so callers retain their own defaults and can distinguish
/// an explicit zero seed (artifact replay) from an omitted seed.
#[derive(Default)]
pub struct CommonOptions {
    pub root: Option<PathBuf>,
    pub hash_seed: Option<u64>,
    pub shard: Option<Shard>,
}
/// An owned option tag releases lexopt's borrow before consuming its value.
#[derive(Clone, Copy)]
pub enum CommonOption {
    Root,
    HashSeed,
    Shard,
    /// The tools that take common options are bug-catching harnesses, which
    /// always run V: they refuse `--no-verify`.
    NoVerify,
}
impl CommonOption {
    pub fn from_arg(arg: &Arg<'_>) -> Option<Self> {
        match arg {
            Arg::Long("root") => Some(Self::Root),
            Arg::Long("hash-seed") => Some(Self::HashSeed),
            Arg::Long("shard") => Some(Self::Shard),
            Arg::Long("no-verify") => Some(Self::NoVerify),
            _ => None,
        }
    }
}
impl CommonOptions {
    pub fn parse(&mut self, option: CommonOption, parser: &mut Parser) -> Result<(), Error> {
        match option {
            CommonOption::Root => {
                self.root = Some(parser.value().map_err(|_| "--root requires a path")?.into());
            }
            CommonOption::HashSeed => {
                self.hash_seed = Some(
                    parser
                        .value()
                        .ok()
                        .and_then(|value| value.to_str()?.parse().ok())
                        .ok_or("--hash-seed requires an unsigned integer")?,
                );
            }
            CommonOption::Shard => {
                self.shard = Some(
                    parser
                        .value()
                        .ok()
                        .and_then(|value| Shard::parse(value.to_str()?))
                        .ok_or("--shard requires <index>/<count>, 1 <= index <= count")?,
                );
            }
            CommonOption::NoVerify => {
                return Err("--no-verify is refused: harnesses always run V".into());
            }
        }
        Ok(())
    }
}

/// A stable split of the corpus by repository-relative path, so a gate can run
/// it as `count` independent parts whose counts sum to the whole run's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shard {
    /// One-based.
    pub index: u64,
    pub count: u64,
}
impl Shard {
    pub fn parse(value: &str) -> Option<Shard> {
        let (index, count) = value.split_once('/')?;
        let (index, count) = (index.parse().ok()?, count.parse().ok()?);
        (1..=count)
            .contains(&index)
            .then_some(Shard { index, count })
    }
    /// FNV-1a over the path's bytes: the same split on every host and build.
    pub fn holds(&self, path: &str) -> bool {
        let hash = path.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
        });
        hash % self.count == self.index - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<CommonOptions, Error> {
        let mut options = CommonOptions::default();
        let mut parser = Parser::from_args(args.iter().copied());
        while let Some(arg) = parser.next()? {
            let Some(option) = CommonOption::from_arg(&arg) else {
                return Err(arg.unexpected());
            };
            options.parse(option, &mut parser)?;
        }
        Ok(options)
    }

    #[test]
    fn new_arguments_require_one_directory_and_accept_lib_in_either_position() {
        for args in [["--lib", "demo"], ["demo", "--lib"]] {
            let options = NewOptions::parse(&mut Parser::from_args(args)).unwrap();
            assert!(options.library);
            assert_eq!(options.directory, PathBuf::from("demo"));
        }
        for args in [
            vec![],
            vec!["--lib"],
            vec!["demo", "extra"],
            vec!["demo", "--lib", "--lib"],
            vec!["--unknown", "demo"],
        ] {
            assert!(NewOptions::parse(&mut Parser::from_args(args)).is_err());
        }
    }

    #[test]
    fn common_options_preserve_explicit_zero_and_last_value() {
        let absent = parse(&[]).unwrap();
        assert!(absent.hash_seed.is_none());
        let options = parse(&[
            "--root",
            "checkout",
            "--hash-seed",
            "3",
            "--hash-seed",
            "0",
            "--shard",
            "2/3",
        ])
        .unwrap();
        assert_eq!(options.root, Some(PathBuf::from("checkout")));
        assert_eq!(options.hash_seed, Some(0));
        assert_eq!(options.shard, Some(Shard { index: 2, count: 3 }));
        let options = parse(&["--root=checkout", "--hash-seed=0", "--shard=2/3"]).unwrap();
        assert_eq!(options.root, Some(PathBuf::from("checkout")));
        assert_eq!(options.hash_seed, Some(0));
        assert_eq!(options.shard, Some(Shard { index: 2, count: 3 }));
    }

    #[cfg(unix)]
    #[test]
    fn root_preserves_non_utf8_bytes() {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};
        let path = OsString::from_vec(b"/tmp/root-\xff".to_vec());
        let mut parser = Parser::from_args([OsString::from("--root"), path.clone()]);
        let mut options = CommonOptions::default();
        let arg = parser.next().unwrap().unwrap();
        let option = CommonOption::from_arg(&arg).unwrap();
        options.parse(option, &mut parser).unwrap();
        assert_eq!(options.root, Some(PathBuf::from(path)));
    }
}
