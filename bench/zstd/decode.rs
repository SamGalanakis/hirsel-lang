use std::ffi::c_void;
#[link(name = ":libzstd.so.1")]
unsafe extern "C" {
    fn ZSTD_decompress(dst: *mut c_void, cap: usize, src: *const c_void, n: usize) -> usize;
    fn ZSTD_isError(n: usize) -> u32;
}
#[link(name = ":libxxhash.so.0")]
unsafe extern "C" {
    fn XXH64(src: *const c_void, n: usize, seed: u64) -> u64;
}
fn main() {
    let src = std::fs::read(std::env::args().nth(1).unwrap()).unwrap();
    let mut sum = 0u64;
    for i in 0..10 {
        let mut out = Vec::<u8>::with_capacity(20000000);
        unsafe {
            let n = ZSTD_decompress(
                out.as_mut_ptr().cast(),
                out.capacity(),
                src.as_ptr().cast(),
                src.len(),
            );
            assert_eq!(ZSTD_isError(n), 0);
            out.set_len(n);
            if i == 0 {
                sum = XXH64(out.as_ptr().cast(), n, 0);
            }
            sum = sum.wrapping_add(n as u64 + out[0] as u64 + out[n - 1] as u64);
        }
    }
    println!("{sum}");
}
