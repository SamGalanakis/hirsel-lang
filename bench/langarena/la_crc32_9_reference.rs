// LangArena buffer_hash_crc32.rs:4-18 at 3c9f84d, MIT (./LICENSE).
// The recurrence is unchanged. The adapter reads hex vectors or exercises the
// frozen full CRC config without the unrelated upstream benchmark registry.
use std::io::{self, BufRead};

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFFFFFFu32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            if (crc & 1) != 0 {
                crc = (crc >> 1) ^ 0xEDB88320u32;
            } else {
                crc >>= 1;
            }
        }
    }
    crc ^ 0xFFFFFFFFu32
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("vectors") {
        for line in io::stdin().lock().lines() {
            let line = line.unwrap();
            let bytes: Vec<_> = (0..line.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&line[i..i + 2], 16).unwrap())
                .collect();
            println!("{}", crc32(&bytes));
        }
    } else {
        let mut seed = 42u64;
        let data: Vec<_> = (0..1_000_000)
            .map(|_| {
                seed = (seed * 3877 + 29573) % 139968;
                (seed * 256 / 139968) as u8
            })
            .collect();
        let mut checksum = 0u32;
        for _ in 0..180 {
            checksum = checksum.wrapping_add(crc32(std::hint::black_box(&data)));
        }
        assert_eq!(checksum, 2318723244);
        println!("{checksum}");
    }
}
