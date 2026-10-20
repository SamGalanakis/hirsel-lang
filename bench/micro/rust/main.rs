use std::collections::HashMap;
use std::fmt::Write;

fn map_u64(n: u64, salt: u64) -> u64 {
    let mut m: HashMap<u64, u64> = HashMap::new();
    for i in 0..n {
        m.insert((i + salt).wrapping_mul(0x9e3779b97f4a7c15), i);
    }
    let mut sum = 0u64;
    for i in 0..n {
        if let Some(v) = m.get(&(i + salt).wrapping_mul(0x9e3779b97f4a7c15)) {
            sum = sum.wrapping_add(*v);
        }
    }
    sum.wrapping_add(m.len() as u64)
}

struct Xo {
    s: [u64; 4],
}
fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e3779b97f4a7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
    z ^ (z >> 31)
}
impl Xo {
    fn seed(v: u64) -> Xo {
        let mut s = v;
        Xo {
            s: [
                splitmix(&mut s),
                splitmix(&mut s),
                splitmix(&mut s),
                splitmix(&mut s),
            ],
        }
    }
    fn next(&mut self) -> u64 {
        let s = &mut self.s;
        let out = s[0].wrapping_add(s[3]).rotate_left(23).wrapping_add(s[0]);
        let t = s[1] << 17;
        s[2] ^= s[0];
        s[3] ^= s[1];
        s[1] ^= s[2];
        s[0] ^= s[3];
        s[2] ^= t;
        s[3] = s[3].rotate_left(45);
        out
    }
}

fn map_str(n: u64, salt: u64) -> u64 {
    let vocab: Vec<String> = (0..4096u64).map(|i| format!("word{i}")).collect();
    let mut g = Xo::seed(42 + salt);
    let mut counts: HashMap<String, u64> = HashMap::new();
    for _ in 0..n {
        let w = &vocab[(g.next() % 4096) as usize];
        if let Some(c) = counts.get_mut(w) {
            *c += 1
        } else {
            counts.insert(w.clone(), 1);
        }
    }
    let mut sum = 0u64;
    for (k, w) in vocab.iter().enumerate() {
        sum = sum.wrapping_add(
            counts
                .get(w)
                .copied()
                .unwrap_or(0)
                .wrapping_mul(k as u64 + 1),
        );
    }
    sum
}

fn strbuild(n: u64, salt: u64) -> u64 {
    let mut out = String::new();
    for i in 0..n {
        write!(out, "{},", i + salt).unwrap();
    }
    let mut h: u64 = 14695981039346656037;
    for &b in out.as_bytes() {
        h = (h ^ b as u64).wrapping_mul(1099511628211);
    }
    h
}

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];
fn compress(st: &mut [u32; 8], block: &[u8]) {
    let mut w = [0u32; 64];
    for i in 0..16 {
        w[i] = u32::from_be_bytes(block[i * 4..i * 4 + 4].try_into().unwrap());
    }
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *st;
    for i in 0..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ (!e & g);
        let t1 = h
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(K[i])
            .wrapping_add(w[i]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);
        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    for (s, v) in st.iter_mut().zip([a, b, c, d, e, f, g, h]) {
        *s = s.wrapping_add(v);
    }
}
fn sha256(data: &[u8]) -> [u8; 32] {
    let mut st = [
        0x6a09e667u32,
        0xbb67ae85,
        0x3c6ef372,
        0xa54ff53a,
        0x510e527f,
        0x9b05688c,
        0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut chunks = data.chunks_exact(64);
    for c in &mut chunks {
        compress(&mut st, c);
    }
    let rem = chunks.remainder();
    let mut last = [0u8; 128];
    last[..rem.len()].copy_from_slice(rem);
    last[rem.len()] = 0x80;
    let total = if rem.len() + 9 > 64 { 128 } else { 64 };
    last[total - 8..total].copy_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for c in last[..total].chunks_exact(64) {
        compress(&mut st, c);
    }
    let mut out = [0u8; 32];
    for (i, w) in st.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&w.to_be_bytes());
    }
    out
}
fn sha(n: u64, salt: u64) -> u64 {
    let data: Vec<u8> = (0..n).map(|i| (i * 31 + salt) as u8).collect();
    let d = sha256(&data);
    let mut h = 0u64;
    for i in 0..8 {
        h = h << 8 | d[i] as u64;
    }
    h
}

fn sort(n: u64, salt: u64) -> u64 {
    let mut g = Xo::seed(7 + salt);
    let mut xs: Vec<u64> = (0..n).map(|_| g.next()).collect();
    xs.sort_unstable_by(|a, b| a.cmp(b));
    let mut h = 0u64;
    for &x in &xs {
        h = h.wrapping_mul(31).wrapping_add(x);
    }
    h
}

fn map_remove(n: u64, salt: u64) -> u64 {
    let mut m: HashMap<u64, u64> = HashMap::new();
    for i in 0..n {
        m.insert((i + salt).wrapping_mul(0x9e3779b97f4a7c15), i);
    }
    let mut sum = 0u64;
    for i in 0..n {
        if let Some(v) = m.remove(&(i + salt).wrapping_mul(0x9e3779b97f4a7c15)) {
            sum = sum.wrapping_add(v);
        }
    }
    sum.wrapping_add(m.len() as u64)
}

fn channel(n: u64) -> u64 {
    let (ping, inbox) = std::sync::mpsc::channel::<u64>();
    let (pong, replies) = std::sync::mpsc::channel::<u64>();
    let worker = std::thread::spawn(move || {
        for _ in 0..n {
            pong.send(inbox.recv().unwrap() + 1).unwrap();
        }
    });
    let mut sum = 0u64;
    for i in 0..n {
        ping.send(i).unwrap();
        sum = sum.wrapping_add(replies.recv().unwrap());
    }
    worker.join().unwrap();
    sum
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let which = a[1].as_str();
    let n: u64 = a[2].parse().unwrap();
    if which == "channel" {
        println!("{}", channel(n));
        return;
    }
    if which == "print" {
        for i in 0..n {
            println!("line {i}");
        }
        return;
    }
    let rounds: u64 = a[3].parse().unwrap();
    let mut sum = 0u64;
    for r in 0..rounds {
        sum = sum.wrapping_add(match which {
            "map_u64" => map_u64(n, r),
            "map_str" | "map_str_upsert" => map_str(n, r),
            "strbuild" => strbuild(n, r),
            "sha" => sha(n, r),
            "sort" => sort(n, r),
            "map_remove" => map_remove(n, r),
            _ => panic!(),
        });
    }
    println!("{sum}");
}
