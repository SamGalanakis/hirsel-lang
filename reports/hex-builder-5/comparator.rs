// Go encoding/hex AppendEncode/Encode (ebe2feefc400, hex.go:45-61),
// expressed with Rust Vec reserve's *additional* capacity convention.
use std::io::{self, Write};
use std::sync::atomic::{AtomicU64, Ordering};

struct Allocator;
static ALLOCS: AtomicU64 = AtomicU64::new(0);
static FREES: AtomicU64 = AtomicU64::new(0);
static REQUESTED: AtomicU64 = AtomicU64::new(0);
unsafe impl std::alloc::GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        REQUESTED.fetch_add(layout.size() as u64, Ordering::Relaxed);
        unsafe { std::alloc::System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        FREES.fetch_add(1, Ordering::Relaxed);
        unsafe { std::alloc::System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: std::alloc::Layout, size: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        FREES.fetch_add(1, Ordering::Relaxed);
        REQUESTED.fetch_add(size as u64, Ordering::Relaxed);
        unsafe { std::alloc::System.realloc(ptr, layout, size) }
    }
}
#[cfg(diagnostic)]
#[global_allocator]
static GLOBAL: Allocator = Allocator;

fn main() {
    let size: usize = std::env::args().nth(1).unwrap().parse().unwrap();
    let chunks = if size == 65536 { 1 } else { 2048 };
    let mut data = Vec::new();
    for i in 0..size {
        data.push((i * 37 + 11) as u8);
    }
    let mut out = Vec::new();
    out.extend_from_slice(b"hex: ");
    for _ in 0..chunks {
        let additional = data.len().checked_mul(2).unwrap();
        out.len().checked_add(additional).unwrap();
        out.reserve(additional);
        for byte in &data {
            out.push(b"0123456789abcdef"[usize::from(byte / 16)]);
            out.push(b"0123456789abcdef"[usize::from(byte % 16)]);
        }
    }
    let result = String::from_utf8(out).unwrap();
    io::stdout().write_all(result.as_bytes()).unwrap();
    #[cfg(diagnostic)]
    eprintln!("capacity={}", result.capacity());
    drop(result);
    drop(data);
    #[cfg(diagnostic)]
    eprintln!(
        "allocations={} frees={} requested_bytes={}",
        ALLOCS.load(Ordering::Relaxed),
        FREES.load(Ordering::Relaxed),
        REQUESTED.load(Ordering::Relaxed)
    );
}
