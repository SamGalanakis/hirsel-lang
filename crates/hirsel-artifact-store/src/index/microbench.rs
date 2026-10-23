//! Manual measured hook into the exact production index source. Input files
//! are the retained study fixtures; setup and measured operations are separate.
use crate::{index::*, pack::index_benchmark_epoch};
use hirsel_syntax::ContentHash;
use std::io::{Read, Seek, SeekFrom};
use std::{
    fs,
    hint::black_box,
    path::{Path, PathBuf},
};
fn read_keys(path: &Path) -> Vec<Key> {
    fs::read(path)
        .unwrap()
        .as_chunks::<33>()
        .0
        .iter()
        .map(|b| Key {
            kind: match b[0] {
                0 => KeyKind::Content,
                1 => KeyKind::Association,
                _ => panic!("fixture kind"),
            },
            digest: ContentHash(b[1..].try_into().unwrap()),
        })
        .collect()
}
fn lookup_entries(fixture: &Path, name: &str) -> Vec<Entry> {
    let keys = read_keys(&fixture.join(name));
    let mut file = fs::File::open(fixture.join("radix_tags")).unwrap();
    let mut header = [0; 128];
    file.read_exact(&mut header).unwrap();
    let offset = u32::from_le_bytes(header[32..36].try_into().unwrap()) as u64;
    let count = u32::from_le_bytes(header[36..40].try_into().unwrap()) as usize;
    file.seek(SeekFrom::Start(offset)).unwrap();
    let mut dictionary = vec![0; count * 32];
    file.read_exact(&mut dictionary).unwrap();
    keys.into_iter()
        .enumerate()
        .map(|(i, key)| Entry {
            key,
            location: Location {
                segment: ContentHash(
                    dictionary[(i / 256) * 32..(i / 256 + 1) * 32]
                        .try_into()
                        .unwrap(),
                ),
                offset: 64 + (i % 256) as u32 * 72,
                len: 72,
            },
        })
        .collect()
}
fn publish_entries(path: &Path, header: usize) -> Vec<Entry> {
    let raw = fs::read(path).unwrap();
    let n = if header == 64 {
        u64::from_le_bytes(raw[8..16].try_into().unwrap()) as usize
    } else {
        raw.len() / 88
    };
    let mut entries: Vec<_> = raw[header..header + n * 88]
        .as_chunks::<88>()
        .0
        .iter()
        .map(|r| Entry {
            key: Key {
                kind: if r[0] == 0 {
                    KeyKind::Content
                } else {
                    KeyKind::Association
                },
                digest: ContentHash(r[8..40].try_into().unwrap()),
            },
            location: Location {
                segment: ContentHash(r[40..72].try_into().unwrap()),
                offset: u64::from_le_bytes(r[72..80].try_into().unwrap())
                    .try_into()
                    .unwrap(),
                len: u64::from_le_bytes(r[80..88].try_into().unwrap())
                    .try_into()
                    .unwrap(),
            },
        })
        .collect();
    entries.sort_unstable_by_key(|e| e.key);
    entries
}
fn write(root: &Path, entries: &[Entry]) {
    fs::create_dir_all(root).unwrap();
    let epoch = index_benchmark_epoch(root).unwrap();
    let delta = build(root, entries).unwrap();
    let result = publish_delta(root, delta, &epoch).unwrap();
    assert!(
        matches!(result, PublishOutcome::Published { .. }),
        "{result:?}"
    );
}
#[test]
#[ignore = "explicit index36 perf invocation only"]
fn measured() {
    let op = std::env::var("INDEX36_OP").unwrap();
    let root = PathBuf::from(std::env::var("INDEX36_ROOT").unwrap());
    let fixture = PathBuf::from(std::env::var("INDEX36_FIXTURE").unwrap());
    let arg: usize = std::env::var("INDEX36_ARG")
        .unwrap_or_else(|_| "0".into())
        .parse()
        .unwrap();
    match op.as_str() {
        "setup" => {
            let entries = lookup_entries(&fixture, "keys");
            for j in 0..arg {
                let part: Vec<_> = entries.iter().skip(j).step_by(arg).copied().collect();
                write(&root, &part);
            }
        }
        "setup-publish" => write(&root, &publish_entries(&fixture.join("layer-1-0"), 64)),
        "build" => write(&root, &lookup_entries(&fixture, "keys")),
        "build-only" => {
            let delta = build(&root, &lookup_entries(&fixture, "keys")).unwrap();
            black_box(delta);
        }
        "publish" => write(
            &root,
            &publish_entries(&fixture.join(format!("delta-{arg}")), 0),
        ),
        "load" => {
            let pin = load_pinned(&root).unwrap();
            println!("retained_bytes={}", pin.retained_bytes());
            black_box(pin);
        }
        "compact" => {
            let outcome = compact(
                &root,
                MaintenanceBudget {
                    max_input_bytes: 128 << 20,
                    max_rows: 2_000_000,
                    max_scratch_bytes: 256 << 20,
                },
            )
            .unwrap();
            assert!(
                matches!(outcome, CompactOutcome::Published { .. }),
                "{outcome:?}"
            );
            println!("{outcome:?}");
        }
        "lookup" | "control" | "warm" | "warm-control" => {
            let pin = load_pinned(&root).unwrap();
            let queries = read_keys(&fixture.join(format!("q{arg}")));
            let probe = || {
                let mut hits = 0;
                for key in &queries {
                    hits += find(&pin, black_box(*key)).is_some() as usize;
                }
                assert_eq!(hits, arg * 1000);
                black_box(hits);
            };
            if op.starts_with("warm") {
                probe();
            }
            if op == "control" || op == "warm-control" {
                black_box(queries);
            } else {
                probe();
            }
        }
        _ => panic!("unknown operation"),
    }
}
