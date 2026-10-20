//! Matched B1 algorithms. Eight logical lanes at every execution width.
use futures_util::{StreamExt, stream::FuturesUnordered};
use std::{
    env, fs,
    io::Write,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Instant,
};
use tokio::{
    sync::{Mutex, mpsc},
    task::JoinSet,
};
unsafe extern "C" {
    fn b1_identity(value: u64) -> u64;
}
const LANES: u64 = 8;

async fn run(case: &str, n: u64, root: PathBuf) -> u64 {
    if case == "channel" {
        let (tx, mut rx) = mpsc::unbounded_channel::<u64>();
        let (reply, mut replies) = mpsc::unbounded_channel();
        let worker = tokio::spawn(async move {
            for _ in 0..n {
                reply.send(rx.recv().await.unwrap() + 1).unwrap();
            }
        });
        let mut sum = 0;
        for i in 0..n {
            tx.send(i).unwrap();
            sum += replies.recv().await.unwrap();
        }
        worker.await.unwrap();
        return sum;
    }
    if case == "fork_join" {
        let mut sum = 0;
        for i in 0..n {
            sum += tokio::spawn(async move { i + 1 }).await.unwrap();
        }
        return sum;
    }
    if case == "late_set" {
        // Two children start before the set exists; one completes and signals the
        // parent before insertion. The third is spawned after the first join.
        let mut sum = 0;
        for _ in 0..n {
            let (tx, mut rx) = mpsc::unbounded_channel();
            let a = tokio::spawn(async move {
                tx.send(()).unwrap();
                1u64
            });
            let b = tokio::spawn(async {
                tokio::task::yield_now().await;
                2u64
            });
            rx.recv().await.unwrap();
            let mut set = FuturesUnordered::new();
            set.push(a);
            set.push(b);
            sum += set.next().await.unwrap().unwrap();
            set.push(tokio::spawn(async { 3u64 }));
            while let Some(value) = set.next().await {
                sum += value.unwrap();
            }
        }
        return sum;
    }
    let atomic = Arc::new(AtomicU64::new(7));
    let mutex = Arc::new(Mutex::new(0u64));
    let shared = Arc::new(vec![1u64; 64]);
    let cancel: Arc<Vec<AtomicBool>> = Arc::new((0..5).map(|_| AtomicBool::new(false)).collect());
    let mut set = JoinSet::new();
    for lane in 0..LANES {
        let (a, m, r, case, root, cancel) = (
            atomic.clone(),
            mutex.clone(),
            shared.clone(),
            case.to_owned(),
            root.clone(),
            cancel.clone(),
        );
        set.spawn(async move {
            let mut sum = 0;
            if case == "independent_files" {
                let mut file = fs::File::create(root.join(format!("lane-{lane}"))).unwrap();
                for _ in 0..n {
                    file.write_all(b"x").unwrap();
                    sum += 1;
                }
                return sum;
            }
            for _ in 0..n {
                match case.as_str() {
                    "yield" => {
                        tokio::task::yield_now().await;
                        sum += 1;
                    }
                    "cancel_depth" => {
                        assert!(!cancel.iter().any(|flag| flag.load(Ordering::SeqCst)));
                        tokio::task::consume_budget().await;
                        sum += 1;
                    }
                    "extern" => {
                        // SAFETY: the linked pure C function accepts/returns any u64 and retains nothing.
                        sum += unsafe { b1_identity(1) };
                    }
                    "atomic" => {
                        sum += a.load(Ordering::SeqCst);
                    }
                    "roots" => {
                        let copy = r.clone();
                        sum += copy[0];
                        drop(copy);
                    }
                    "mutex" => {
                        *m.lock().await += 1;
                        sum += 1;
                    }
                    "allocation" => {
                        let mut data = Vec::new();
                        for _ in 0..64 {
                            data.push(1u64);
                        }
                        sum += std::hint::black_box(data)[0];
                    }
                    "namespace_mix" => {
                        let from = root.join(format!("a-{lane}"));
                        let to = root.join(format!("b-{lane}"));
                        drop(fs::File::create(&from).unwrap());
                        fs::rename(&from, &to).unwrap();
                        fs::remove_file(&to).unwrap();
                        sum += 1;
                    }
                    _ => panic!("unknown workload: {case}"),
                }
            }
            sum
        });
    }
    let mut sum = 0;
    while let Some(value) = set.join_next().await {
        sum += value.unwrap();
    }
    sum
}
fn main() {
    let args: Vec<_> = env::args().collect();
    let case = &args[1];
    let width: usize = args[2].parse().unwrap();
    let n: u64 = args[3].parse().unwrap();
    let root = PathBuf::from(&args[4]);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(width)
        .build()
        .unwrap();
    let start = Instant::now();
    let checksum = runtime.block_on(run(case, n, root));
    eprintln!(
        "{}",
        serde_json::json!({"schema": 1, "elapsed_ns": start.elapsed().as_nanos(),
        "width": width, "implementation": "tokio", "metrics": {}})
    );
    println!("{checksum}");
}
