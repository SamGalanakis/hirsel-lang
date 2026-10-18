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
}
impl CommonOption {
    pub fn from_arg(arg: &Arg<'_>) -> Option<Self> {
        match arg {
            Arg::Long("root") => Some(Self::Root),
            Arg::Long("hash-seed") => Some(Self::HashSeed),
            Arg::Long("shard") => Some(Self::Shard),
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

    #[test]
    fn missing_and_invalid_common_values_have_stable_errors() {
        for (flag, invalid, message) in [
            (
                "--hash-seed",
                &["-1", "18446744073709551616", "seed"][..],
                "--hash-seed requires an unsigned integer",
            ),
            (
                "--shard",
                &["0/1", "2/1", "1/0", "1/2/3", "one"][..],
                "--shard requires <index>/<count>, 1 <= index <= count",
            ),
            ("--root", &[][..], "--root requires a path"),
        ] {
            assert_eq!(parse(&[flag]).err().unwrap().to_string(), message);
            for value in invalid {
                assert_eq!(parse(&[flag, value]).err().unwrap().to_string(), message);
            }
        }
        assert_eq!(
            parse(&["--hash-seed", "18446744073709551615"])
                .unwrap()
                .hash_seed,
            Some(u64::MAX)
        );
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
