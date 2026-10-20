// A deterministic workload, no timing: build a word corpus from a seeded
// generator, then count word frequencies and collect the distinct words twice,
// once on the heap and once inside a bounded arena. Both summaries must agree.
//
// usage: bench-lite [WORDS [SEED [ARENA_BYTES]]]: by default 2000 words, seed 1
// and a 16 MiB arena.
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;

#[derive(Debug, PartialEq)]
pub struct Summary {
    words: u64,
    different: u64,
    bytes: u64,
    checksum: u64,
    top: Vec<(String, u64)>,
}

impl fmt::Display for Summary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "words {}\ndifferent {}\nbytes {}\nchecksum {}\n",
            self.words, self.different, self.bytes, self.checksum
        )?;
        for (w, n) in &self.top {
            writeln!(f, "  {w} {n}")?;
        }
        Ok(())
    }
}

pub struct Ranked {
    word: String,
    count: u64,
}

const SYLLABLES: [&str; 20] = [
    "ba", "ce", "di", "fo", "gu", "ha", "ji", "ko", "lu", "ma", "ne", "pi", "qo", "ru", "sa", "te",
    "vi", "wo", "xu", "ze",
];

// A seeded SplitMix64 generator.
pub struct Generator {
    state: u64,
}

fn seed(seed: u64) -> Generator {
    Generator { state: seed }
}

fn next_u64(g: &mut Generator) -> u64 {
    g.state = g.state.wrapping_add(0x9e3779b97f4a7c15);
    let mut z = g.state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
    z ^ (z >> 31)
}

// One word of one to three syllables; the generator alone decides it.
fn word(g: &mut Generator) -> String {
    let mut out = String::new();
    let syllables = 1 + next_u64(g) % 3;
    for _ in 0..syllables {
        out.push_str(SYLLABLES[(next_u64(g) % SYLLABLES.len() as u64) as usize]);
    }
    out
}

fn corpus(count: u64, seed_value: u64) -> Vec<String> {
    let mut g = seed(seed_value);
    let mut words = Vec::new();
    for _ in 0..count {
        words.push(word(&mut g));
    }
    words
}

fn by_count(a: &Ranked, b: &Ranked) -> Ordering {
    if a.count != b.count {
        return if a.count > b.count { Ordering::Less } else { Ordering::Greater };
    }
    a.word.cmp(&b.word)
}

// FNV-1a over each distinct word and its count, in first-seen order.
fn mix(hash: u64, data: &[u8]) -> u64 {
    let mut h = hash;
    for &b in data {
        h = (h ^ b as u64).wrapping_mul(1099511628211);
    }
    h
}

// A bump region of fixed capacity: allocations fail once it is full.
pub struct Arena {
    capacity: usize,
    used: usize,
}

impl Arena {
    fn take(&mut self, bytes: usize) -> Result<(), String> {
        if self.used + bytes > self.capacity {
            return Err("the arena ran out of space".into());
        }
        self.used += bytes;
        Ok(())
    }
}

// Count in maps and lists whose storage is charged to `arena` when there is one;
// the summary is always on the heap.
fn tally(
    words: &[String],
    counts: &mut BTreeMap<String, u64>,
    order: &mut Vec<String>,
    mut arena: Option<&mut Arena>,
) -> Result<Summary, String> {
    let mut bytes = 0u64;
    for w in words {
        bytes += w.len() as u64;
        match counts.get(w).copied() {
            Some(n) => {
                counts.insert(w.clone(), n + 1);
            }
            None => {
                if let Some(a) = arena.as_deref_mut() {
                    a.take(2 * w.len() + 48)?;
                }
                counts.insert(w.clone(), 1);
                order.push(w.clone());
            }
        }
    }
    let mut checksum = 14695981039346656037u64;
    let mut ranked: Vec<Ranked> = Vec::new();
    for w in order.iter() {
        let n = counts.get(w).copied().unwrap_or(0);
        checksum = mix(checksum, w.as_bytes());
        checksum = mix(checksum, n.to_string().as_bytes());
        ranked.push(Ranked { word: w.clone(), count: n });
    }
    ranked.sort_by(by_count);
    let mut top = Vec::new();
    for r in ranked {
        if top.len() == 5 {
            break;
        }
        top.push((r.word, r.count));
    }
    Ok(Summary {
        words: words.len() as u64,
        different: order.len() as u64,
        bytes,
        checksum,
        top,
    })
}

fn heap_summary(words: &[String]) -> Summary {
    let mut counts = BTreeMap::new();
    let mut order = Vec::new();
    tally(words, &mut counts, &mut order, None).unwrap_or_else(|_| unreachable!())
}

fn arena_summary(words: &[String], capacity: u64) -> Result<Summary, String> {
    let mut arena = Arena { capacity: capacity as usize, used: 0 };
    let mut counts = BTreeMap::new();
    let mut order = Vec::new();
    tally(words, &mut counts, &mut order, Some(&mut arena))
}

fn number(args: &[String], index: usize, default: u64) -> Result<u64, String> {
    let Some(text) = args.get(index) else {
        return Ok(default);
    };
    match text.parse() {
        Ok(n) => Ok(n),
        Err(_) => Err(format!("not a number: {text}")),
    }
}

fn run(args: &[String]) -> Result<String, String> {
    if args.len() > 3 {
        return Err("usage: bench-lite [WORDS [SEED [ARENA_BYTES]]]".into());
    }
    let words = corpus(number(args, 0, 2000)?, number(args, 1, 1)?);
    let on_heap = heap_summary(&words);
    let in_arena = arena_summary(&words, number(args, 2, 16777216)?)?;
    if on_heap != in_arena {
        return Err("heap and arena summaries differ".into());
    }
    Ok(format!("{on_heap}heap and arena agree\n"))
}

pub fn main(args: &[String]) -> Result<(), String> {
    let text = run(args)?;
    print!("{text}");
    Ok(())
}
