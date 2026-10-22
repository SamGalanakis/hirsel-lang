//! LangArena 3c9f84d neural_net.rs:10–222, unmodified (MIT, adjacent LICENSE).
//! The wrapper exposes state and retains upstream PRNG and checksum scalars.
#![allow(dead_code)]
static ORACLE: std::sync::Mutex<()> = std::sync::Mutex::new(());

trait Benchmark {
    fn name(&self) -> String;
    fn prepare(&mut self);
    fn run(&mut self, iteration: i64);
    fn checksum(&self) -> u32;
}
mod helper {
    use std::sync::atomic::{AtomicI32, Ordering};
    static SEED: AtomicI32 = AtomicI32::new(42);
    pub fn reset() {
        SEED.store(42, Ordering::SeqCst);
    }
    pub fn next_float(max: f64) -> f64 {
        let seed = (SEED.load(Ordering::SeqCst) * 3877 + 29573) % 139968;
        SEED.store(seed, Ordering::SeqCst);
        max * f64::from(seed) / 139968.0
    }
    pub fn checksum_f64(value: f64) -> u32 {
        format!("{value:.7}").bytes().fold(5381u32, |h, b| {
            h.wrapping_mul(33).wrapping_add(u32::from(b))
        })
    }
}
mod benchmarks {
    pub mod neural_net {
        include!("upstream.rs");
        pub fn vectors(points: &[usize]) -> Vec<Vec<u64>> {
            let _oracle = super::super::ORACLE.lock().unwrap();
            helper::reset();
            let mut net = NeuralNetwork::new(2, 10, 1);
            let mut states = Vec::new();
            let mut updates = 0;
            for &point in points {
                while updates < point {
                    let (a, b, t) = [
                        (0.0, 0.0, 0.0),
                        (1.0, 0.0, 1.0),
                        (0.0, 1.0, 1.0),
                        (1.0, 1.0, 0.0),
                    ][updates % 4];
                    net.train(&[a, b], &[t]);
                    updates += 1;
                }
                let mut state = Vec::new();
                for n in net
                    .input_layer
                    .iter()
                    .chain(&net.hidden_layer)
                    .chain(&net.output_layer)
                {
                    state.extend([
                        n.threshold.to_bits(),
                        n.prev_threshold.to_bits(),
                        n.output.to_bits(),
                        n.error.to_bits(),
                    ]);
                }
                for s in &net.synapses {
                    state.extend([s.weight.to_bits(), s.prev_weight.to_bits()]);
                }
                states.push(state);
            }
            states
        }
        pub fn checksums() -> Vec<u32> {
            let _oracle = super::super::ORACLE.lock().unwrap();
            helper::reset();
            let mut bench = NeuralNet::new();
            bench.prepare();
            let mut result = Vec::new();
            for i in 0..3 {
                bench.run(i);
                result.push(bench.checksum());
            }
            result
        }
    }
}
pub use benchmarks::neural_net::{checksums, vectors};
