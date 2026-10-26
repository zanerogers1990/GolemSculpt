//! The threads a brush dab runs on.
//!
//! A dab is shared out between threads once it reaches a few thousand
//! vertices (`kernels::PAR_MIN`). On a processor with many cores, waking all
//! of them for a dab of a few tens of thousands of vertices costs more than
//! they save. On the user's i9-13900K (8 fast cores, 16 slower ones, 32
//! threads) a dab that reaches fewer than [`SMALL_DAB`] vertices runs on a
//! pool of its own of at most [`SMALL_THREADS`] threads, and a bigger one on
//! every thread, as before. Measured with `sculpt --bench` against all 32
//! threads for every dab: 4.7% less time over all the brushes on a million
//! triangles and 3.3% on four million, 8 to 12% for the small brush; the cut
//! between 50 and 75 thousand vertices did best at both sizes, all 8 threads
//! for every dab lost 9% on four million. The work is the same either way
//! and the result the same to the bit: only how many threads share it
//! changes.

use std::sync::OnceLock;

use rayon::{ThreadPool, ThreadPoolBuilder};

/// Below this many vertices in reach, a dab runs on the small pool.
pub(super) const SMALL_DAB: usize = 64_000;
/// The most threads the small pool has.
const SMALL_THREADS: usize = 8;

/// The small pool, made the first time a dab asks for it. `None` if it could
/// not be made, and every dab then runs on all the threads.
fn small() -> Option<&'static ThreadPool> {
    static POOL: OnceLock<Option<ThreadPool>> = OnceLock::new();
    POOL.get_or_init(|| {
        let threads = rayon::current_num_threads().min(SMALL_THREADS);
        ThreadPoolBuilder::new().num_threads(threads).thread_name(|i| format!("brush {i}")).build().ok()
    })
    .as_ref()
}

/// Runs `work`, a dab that reaches about `vertices` vertices, on the threads
/// that suit its size.
pub(super) fn run<R: Send>(vertices: usize, work: impl FnOnce() -> R + Send) -> R {
    match small().filter(|_| vertices < SMALL_DAB) {
        Some(pool) => pool.install(work),
        None => work(),
    }
}
