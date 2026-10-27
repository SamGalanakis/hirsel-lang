//! How a dense table position becomes a session id, shared by every id kind
//! (Symbol, DefId, TypeId, Anchor). Normally id = position + 1. Under the
//! `id-salt` feature, `salt_ids(seed)` makes each kind multiply that by an odd
//! seed-derived constant, a bijection on the nonzero u32s, so hash order and
//! relative id order both move with the seed while output must not (oracle O1).
use std::num::NonZeroU32;

/// One id kind's assignment rule.
pub struct Scramble {
    #[cfg(feature = "id-salt")]
    kind: &'static str,
    #[cfg(feature = "id-salt")]
    ready: std::sync::Once,
    #[cfg(feature = "id-salt")]
    multiplier: std::sync::atomic::AtomicU32,
    #[cfg(feature = "id-salt")]
    inverse: std::sync::atomic::AtomicU32,
}

impl Scramble {
    /// `kind` names the id kind, so kinds get distinct permutations of one seed.
    #[cfg_attr(not(feature = "id-salt"), allow(unused_variables))]
    pub const fn new(kind: &'static str) -> Self {
        Self {
            #[cfg(feature = "id-salt")]
            kind,
            #[cfg(feature = "id-salt")]
            ready: std::sync::Once::new(),
            #[cfg(feature = "id-salt")]
            multiplier: std::sync::atomic::AtomicU32::new(1),
            #[cfg(feature = "id-salt")]
            inverse: std::sync::atomic::AtomicU32::new(1),
        }
    }

    /// The id for table position `index`. Call it only when assigning.
    pub fn encode(&self, index: u32) -> NonZeroU32 {
        let ordinal = index.checked_add(1).expect("id space exceeds u32");
        #[cfg(feature = "id-salt")]
        let ordinal = {
            use std::sync::atomic::Ordering::Relaxed;
            self.ready.call_once(|| {
                let multiplier = multiplier(seed::fix(), self.kind);
                self.multiplier.store(multiplier, Relaxed);
                self.inverse.store(inverse(multiplier), Relaxed);
            });
            ordinal.wrapping_mul(self.multiplier.load(Relaxed))
        };
        NonZeroU32::new(ordinal).expect("an odd multiplier keeps ids nonzero")
    }

    /// The table position of an id this scramble assigned.
    #[inline]
    pub fn decode(&self, id: NonZeroU32) -> u32 {
        let ordinal = id.get();
        // The id's publication orders this load after the assignment's store.
        #[cfg(feature = "id-salt")]
        let ordinal = ordinal.wrapping_mul(self.inverse.load(std::sync::atomic::Ordering::Relaxed));
        ordinal - 1
    }
}

/// splitmix64 over the seed and the kind's spelling, forced odd.
#[cfg(feature = "id-salt")]
fn multiplier(seed: u64, kind: &str) -> u32 {
    if seed == 0 {
        return 1;
    }
    let mut z = kind.bytes().fold(seed, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
    });
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    (z ^ (z >> 31)) as u32 | 1
}

/// The multiplicative inverse of an odd number modulo 2^32 (Newton's iteration).
#[cfg(feature = "id-salt")]
fn inverse(odd: u32) -> u32 {
    let mut x = odd;
    for _ in 0..5 {
        x = x.wrapping_mul(2u32.wrapping_sub(odd.wrapping_mul(x)));
    }
    x
}

#[cfg(feature = "id-salt")]
mod seed {
    use std::sync::OnceLock;
    static SEED: OnceLock<u64> = OnceLock::new();

    /// The process's seed, fixed at the first id assignment of any kind.
    pub(super) fn fix() -> u64 {
        *SEED.get_or_init(|| 0)
    }

    pub(super) fn set(seed: u64) {
        if SEED.set(seed).is_err() {
            panic!(
                "salt_ids must run once, before any Symbol, DefId, TypeId or Anchor is assigned"
            );
        }
    }
}

/// Permute this process's assignment of Symbol, DefId, TypeId and Anchor ids.
/// Seed 0 is the unsalted assignment. Call it once, before any id is assigned.
#[cfg(feature = "id-salt")]
pub fn salt_ids(seed: u64) {
    seed::set(seed);
}

/// Environment variable carrying a child test run's seed.
#[cfg(feature = "id-salt")]
const SEED_VARIABLE: &str = "HIRSEL_ID_SALT";
#[cfg(feature = "id-salt")]
const OUTPUT_START: &str = "\n<<id-salt output>>\n";
#[cfg(feature = "id-salt")]
const OUTPUT_END: &str = "\n<</id-salt output>>\n";

/// Run test `test` (its full libtest name) once per seed, each in a fresh,
/// concurrent child process of this test binary salted before any id exists, and return each
/// run's `body` output in seed order. Inside such a child, this salts, prints
/// `body`'s output for the parent and returns `None`.
#[cfg(feature = "id-salt")]
pub fn under_salts(
    test: &str,
    seeds: &[u64],
    body: impl FnOnce() -> String,
) -> Option<Vec<String>> {
    under_salts_with_env(test, seeds, &[], body)
}

/// Run the same fresh salted processes with explicit child-only environment.
/// The caller owns any shared fixture for the lifetime of all children.
#[cfg(feature = "id-salt")]
pub fn under_salts_with_env(
    test: &str,
    seeds: &[u64],
    environment: &[(&str, &std::ffi::OsStr)],
    body: impl FnOnce() -> String,
) -> Option<Vec<String>> {
    if let Ok(seed) = std::env::var(SEED_VARIABLE) {
        salt_ids(seed.parse().expect("a numeric id salt"));
        print!("{OUTPUT_START}{}{OUTPUT_END}", body());
        return None;
    }
    let binary = std::env::current_exe().expect("the running test binary");
    let children: Vec<_> = seeds
        .iter()
        .map(|seed| {
            let child = std::process::Command::new(&binary)
                .args([test, "--exact", "--nocapture", "--test-threads=1"])
                .envs(environment.iter().copied())
                .env(SEED_VARIABLE, seed.to_string())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("a salted child run");
            (seed, child)
        })
        .collect();
    let outputs = children
        .into_iter()
        .map(|(seed, child)| {
            let run = child.wait_with_output().expect("a salted child run");
            let stdout = String::from_utf8(run.stdout).expect("UTF-8 child output");
            assert!(
                run.status.success(),
                "salt {seed} run of {test} failed:\n{stdout}\n{}",
                String::from_utf8_lossy(&run.stderr)
            );
            let start = stdout.find(OUTPUT_START).unwrap_or_else(|| {
                panic!("salt {seed} run of {test} printed no output:\n{stdout}")
            }) + OUTPUT_START.len();
            let end = stdout[start..].find(OUTPUT_END).expect("framed output") + start;
            stdout[start..end].to_owned()
        })
        .collect();
    Some(outputs)
}

#[cfg(all(test, feature = "id-salt"))]
mod tests {
    use super::*;

    #[test]
    fn salted_multipliers_are_odd_and_invertible() {
        for seed in [1, 2, 3, u64::MAX] {
            for kind in ["symbol", "def", "type", "anchor"] {
                let m = multiplier(seed, kind);
                assert_eq!(m % 2, 1);
                assert_eq!(m.wrapping_mul(inverse(m)), 1);
            }
        }
        assert_ne!(multiplier(1, "symbol"), multiplier(1, "def"));
        assert_eq!(multiplier(0, "symbol"), 1);
    }
}
