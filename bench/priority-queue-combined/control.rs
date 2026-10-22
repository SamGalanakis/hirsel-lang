//! Rust BinaryHeap controls. Input, root ordering, ties and final drain match topk.hir.
#[cfg(pqcombined_count)]
use std::cmp::Ordering;
#[cfg(not(pqcombined_count))]
use std::cmp::Reverse as Key;
use std::collections::BinaryHeap;
#[cfg(pqcombined_count)]
thread_local! { static CALLS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }
#[inline]
fn count() {
    #[cfg(pqcombined_count)]
    CALLS.with(|c| c.set(c.get() + 1));
}
#[cfg(pqcombined_count)]
#[derive(Eq, PartialEq)]
struct Key(u64);
#[cfg(pqcombined_count)]
impl Ord for Key {
    fn cmp(&self, other: &Self) -> Ordering {
        count();
        other.0.cmp(&self.0)
    }
}
#[cfg(pqcombined_count)]
impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
fn main() {
    let args: Vec<u64> = std::env::args()
        .skip(1)
        .map(|s| s.parse().unwrap())
        .collect();
    let (mode, k, n, mut state) = (args[0], args[1], args[2], args[3]);
    let key = Key;
    let mut heap = BinaryHeap::new();
    let mut next = || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        (state >> 32) % 1024
    };
    for _ in 0..k {
        heap.push(key(next()));
    }
    let mut sum = 0u64;
    for _ in 0..n {
        let value = next();
        let returned = if mode == 0 {
            match heap.peek_mut() {
                Some(mut root) => {
                    count();
                    if value <= root.0 {
                        value
                    } else {
                        std::mem::replace(&mut *root, key(value)).0
                    }
                }
                None => value,
            }
        } else if heap.is_empty() {
            heap.push(key(value));
            0
        } else {
            let mut root = heap.peek_mut().unwrap();
            std::mem::replace(&mut *root, key(value)).0
        };
        sum = sum.wrapping_mul(31).wrapping_add(returned);
    }
    while let Some(item) = heap.pop() {
        sum = sum.wrapping_mul(31).wrapping_add(item.0);
    }
    println!("{sum}");
    #[cfg(pqcombined_count)]
    CALLS.with(|c| eprintln!("calls={}", c.get()));
}
