//! Supplemental row B: Rust Drop on success and explicit failure-result returns.
use std::io::{self, Write};
#[cfg(feature = "mimalloc")]
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;
#[cfg(not(feature = "mimalloc"))]
#[global_allocator]
static ALLOCATOR: std::alloc::System = std::alloc::System;
#[cfg(feature = "stats")]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(feature = "stats")]
static DROPS: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "stats")]
static COPIES: AtomicU64 = AtomicU64::new(0);
fn copied(_n: u64) {
    #[cfg(feature = "stats")]
    COPIES.fetch_add(_n, Ordering::Relaxed);
}
struct Resource {
    id: u8,
    closed: bool,
    trace: bool,
}
impl Drop for Resource {
    fn drop(&mut self) {
        assert!(!self.closed);
        self.closed = true;
        #[cfg(feature = "stats")]
        DROPS.fetch_add(1, Ordering::Relaxed);
        if self.trace {
            io::stdout().write_all(&[self.id]).unwrap();
        }
    }
}
fn attempt(i: u64, modulus: u64, trace: bool) -> Result<[u8; 16], ()> {
    let _a = Resource {
        id: b'8',
        closed: false,
        trace,
    };
    let _b = Resource {
        id: b'7',
        closed: false,
        trace,
    };
    let source = std::array::from_fn::<_, 16, _>(|j| (i % 127 + j as u64) as u8);
    let mut transaction = source;
    copied(16);
    for value in &mut transaction {
        *value = value.checked_add(1).unwrap();
    }
    if i % modulus == 0 {
        return Err(());
    }
    copied(16);
    Ok(transaction)
}
fn digest(bytes: &[u8]) -> u64 {
    bytes.iter().fold(14695981039346656037u64, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(1099511628211)
    })
}
fn workload(input: &str, trace: bool) -> String {
    let n: u64 = input.trim().parse().unwrap();
    assert!(n > 0 && n <= 1000000);
    let modulus = if cfg!(feature = "heavy") { 2 } else { 17 };
    let mut total = 0u64;
    for i in 0..n {
        let result = attempt(i, modulus, trace);
        let failed = result.is_err();
        assert_eq!(failed, i % modulus == 0);
        let destination = result.unwrap_or([0; 16]);
        let mut h = i;
        for event in [1u64, 2, 3, 4, if failed { 6 } else { 5 }, 7, 8] {
            h = h.wrapping_mul(131).wrapping_add(event);
        }
        total = total.wrapping_add(h ^ digest(&destination));
    }
    let mut out = String::with_capacity(32);
    use std::fmt::Write;
    writeln!(&mut out, "{total}").unwrap();
    out
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    assert!((5..=7).contains(&args.len()));
    let emit = args.get(5).is_some_and(|s| s == "--emit");
    let trace = args.get(6).is_some_and(|s| s == "--trace");
    assert!(!trace || emit);
    let variant = if cfg!(feature = "mimalloc") {
        "mimalloc"
    } else {
        "system"
    };
    assert_eq!(args[4], variant);
    assert_eq!(
        args[1],
        if cfg!(feature = "heavy") {
            "resource-drops/heavy"
        } else {
            "resource-drops/throughput"
        }
    );
    let input = std::fs::read_to_string(&args[2]).unwrap();
    let rounds: u64 = args[3].parse().unwrap();
    assert!(rounds > 0 && (!emit || rounds == 1));
    let mut sum = 0u64;
    for _ in 0..rounds {
        let out = workload(&input, trace);
        sum = sum.wrapping_add(digest(out.as_bytes()));
        if emit {
            io::stdout().write_all(out.as_bytes()).unwrap();
        }
    }
    if !emit {
        println!("{sum:016x}");
    }
    #[cfg(feature = "stats")]
    eprintln!("{{\"allocation_requests\":{rounds},\"requested_bytes\":{},\"copied_bytes\":{},\"release_requests\":{rounds},\"finalizer_calls\":{}}}", rounds * 32, COPIES.load(Ordering::Relaxed), DROPS.load(Ordering::Relaxed));
}
