//! Work that takes seconds, run off the interface thread.
//!
//! A remesh, a boolean or fusing tubes into a model must not freeze the
//! window. A tool starts one with `Ctx::spawn`, keeps the [`Job`] it gets
//! back, and asks it for the result each frame in `Tool::frame`; the app
//! keeps drawing frames while any job runs and shows what each one says it is
//! doing.
//!
//! The work runs on its own thread and cannot borrow from the scene: copy
//! what it needs first (`MeshView::to_tri_mesh`). It may use `rayon` freely.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;

use sculpt_geom::TriMesh;

use crate::scene::MeshData;

/// Turns plain triangles into geometry ready for the scene. The app's version
/// builds its sculpting structures, which is the slow part of making a large
/// object; inside a job that happens off the interface thread.
pub type Baker = Arc<dyn Fn(TriMesh, Option<Vec<f32>>) -> MeshData + Send + Sync>;

struct Shared {
    label: String,
    progress: Mutex<String>,
    cancel: AtomicBool,
    done: AtomicBool,
}

/// A poisoned lock only means a job panicked while holding it; the text or
/// result inside is still the best there is.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// What the work sees of the app while it runs.
pub struct JobCtx {
    shared: Arc<Shared>,
    baker: Baker,
}

impl JobCtx {
    /// Says what the job is doing now ("Voxelizing", "Fusing 3 of 8"). Shown
    /// beside the job's name while it runs.
    pub fn progress(&self, text: impl Into<String>) {
        *lock(&self.shared.progress) = text.into();
    }

    /// Whether the job was cancelled (by the tool, by the user, or by the
    /// tool dropping its [`Job`]). Check between stages and return early;
    /// what is returned after a cancel is thrown away.
    pub fn cancelled(&self) -> bool {
        self.shared.cancel.load(Ordering::Relaxed)
    }

    /// Prepares a mesh for the scene here, on the job's thread, so the
    /// commit that uses it is instant. Worth doing for anything over a few
    /// hundred thousand triangles.
    pub fn bake(&self, mesh: TriMesh, mask: Option<Vec<f32>>) -> MeshData {
        (self.baker)(mesh, mask)
    }
}

/// A running or finished piece of background work, and the only way to its
/// result. Dropping it cancels the work.
pub struct Job<T> {
    shared: Arc<Shared>,
    result: Arc<Mutex<Option<Result<T, String>>>>,
    thread: Option<JoinHandle<()>>,
}

impl<T> Job<T> {
    /// The result, once, on the first call after the work has finished;
    /// `None` while it is still running and after the result was taken.
    /// `Err` carries a message the user can read.
    pub fn poll(&mut self) -> Option<Result<T, String>> {
        if !self.is_done() {
            return None;
        }
        lock(&self.result).take()
    }

    pub fn is_done(&self) -> bool {
        self.shared.done.load(Ordering::Acquire)
    }

    /// What the job last said it was doing.
    pub fn progress(&self) -> String {
        lock(&self.shared.progress).clone()
    }

    /// Asks the work to stop. It stops when it next checks.
    pub fn cancel(&self) {
        self.shared.cancel.store(true, Ordering::Relaxed);
    }

    /// Blocks until the work is done and returns its result. For tests and
    /// for `Tool::demo`, where there is no later frame to wait for.
    pub fn wait(mut self) -> Result<T, String> {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        lock(&self.result).take().unwrap_or_else(|| Err(format!("{} gave no result.", self.shared.label)))
    }
}

impl<T> Drop for Job<T> {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// One line of the app's list of running jobs.
#[derive(Clone, Debug, PartialEq)]
pub struct Running {
    pub label: String,
    pub progress: String,
}

/// Starts jobs and knows which are running. The app owns one.
pub struct Jobs {
    baker: Baker,
    running: Mutex<Vec<Arc<Shared>>>,
}

impl Jobs {
    pub fn new(baker: Baker) -> Jobs {
        Jobs { baker, running: Mutex::new(Vec::new()) }
    }

    /// Jobs whose `bake` leaves meshes as plain triangles: for tests.
    pub fn plain() -> Jobs {
        Jobs::new(Arc::new(|mesh, mask| MeshData::Plain { mesh, mask }))
    }

    /// Runs `work` on a thread of its own. `label` names it to the user.
    pub fn spawn<T, F>(&self, label: &str, work: F) -> Job<T>
    where
        T: Send + 'static,
        F: FnOnce(&JobCtx) -> Result<T, String> + Send + 'static,
    {
        let shared = Arc::new(Shared {
            label: label.to_owned(),
            progress: Mutex::new(String::new()),
            cancel: AtomicBool::new(false),
            done: AtomicBool::new(false),
        });
        let result = Arc::new(Mutex::new(None));
        let cx = JobCtx { shared: shared.clone(), baker: self.baker.clone() };
        let slot = result.clone();
        let run = move || {
            // A bug in the work must not take the window with it, nor leave
            // the tool waiting for ever. This only holds while the app is
            // built to unwind on a panic (`[profile.release]` in the top
            // Cargo.toml): with `panic = "abort"` the panic ends the whole
            // program before it ever gets back here.
            let outcome = catch_unwind(AssertUnwindSafe(|| work(&cx)))
                .unwrap_or_else(|_| Err(format!("{} stopped unexpectedly.", cx.shared.label)));
            *lock(&slot) = Some(outcome);
            cx.shared.done.store(true, Ordering::Release);
        };
        let thread = match std::thread::Builder::new().name(format!("job: {label}")).spawn(run) {
            Ok(thread) => Some(thread),
            Err(e) => {
                *lock(&result) = Some(Err(format!("Could not start {label}: {e}.")));
                shared.done.store(true, Ordering::Release);
                None
            }
        };
        lock(&self.running).push(shared.clone());
        Job { shared, result, thread }
    }

    /// The jobs still running, oldest first. Cancelled ones are listed until
    /// they notice and stop.
    pub fn running(&self) -> Vec<Running> {
        let mut running = lock(&self.running);
        running.retain(|job| !job.done.load(Ordering::Acquire));
        running.iter().map(|job| Running { label: job.label.clone(), progress: lock(&job.progress).clone() }).collect()
    }

    pub fn is_busy(&self) -> bool {
        !self.running().is_empty()
    }

    /// Asks running job `index` (as listed by [`Self::running`]) to stop.
    pub fn cancel(&self, index: usize) {
        let mut running = lock(&self.running);
        running.retain(|job| !job.done.load(Ordering::Acquire));
        if let Some(job) = running.get(index) {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use super::*;

    fn wait_until(what: impl Fn() -> bool) {
        let start = Instant::now();
        while !what() {
            assert!(start.elapsed() < Duration::from_secs(10), "timed out");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn a_result_arrives_on_a_later_poll_and_only_once() {
        let jobs = Jobs::plain();
        let (go, wait) = mpsc::channel::<()>();
        let mut job = jobs.spawn("Sum", move |_| {
            wait.recv().map_err(|e| e.to_string())?;
            Ok((1..=10).sum::<i32>())
        });
        assert!(job.poll().is_none());
        assert!(jobs.is_busy());
        go.send(()).unwrap();
        wait_until(|| job.is_done());
        assert_eq!(job.poll(), Some(Ok(55)));
        assert_eq!(job.poll(), None);
        assert!(!jobs.is_busy());
    }

    #[test]
    fn progress_text_is_readable_while_the_job_runs() {
        let jobs = Jobs::plain();
        let (go, wait) = mpsc::channel::<()>();
        let job = jobs.spawn("Build", move |cx| {
            cx.progress("Halfway");
            let _ = wait.recv();
            Ok(())
        });
        wait_until(|| job.progress() == "Halfway");
        assert_eq!(jobs.running(), vec![Running { label: "Build".into(), progress: "Halfway".into() }]);
        go.send(()).unwrap();
        assert_eq!(job.wait(), Ok(()));
    }

    #[test]
    fn cancelling_reaches_the_work() {
        let jobs = Jobs::plain();
        let job = jobs.spawn("Long", |cx| {
            while !cx.cancelled() {
                std::thread::sleep(Duration::from_millis(1));
            }
            Err::<(), _>("Cancelled.".to_owned())
        });
        jobs.cancel(0);
        assert_eq!(job.wait(), Err("Cancelled.".to_owned()));
    }

    #[test]
    fn dropping_the_job_cancels_it() {
        let jobs = Jobs::plain();
        let (seen, saw) = mpsc::channel();
        drop(jobs.spawn("Dropped", move |cx| {
            while !cx.cancelled() {
                std::thread::sleep(Duration::from_millis(1));
            }
            let _ = seen.send(());
            Ok(())
        }));
        saw.recv_timeout(Duration::from_secs(10)).expect("the work saw the cancel");
        wait_until(|| !jobs.is_busy());
    }

    #[test]
    fn a_panic_in_the_work_comes_back_as_an_error() {
        let jobs = Jobs::plain();
        let job = jobs.spawn("Fragile", |_| -> Result<(), String> { panic!("bug") });
        assert_eq!(job.wait(), Err("Fragile stopped unexpectedly.".to_owned()));
    }

    #[test]
    fn bake_goes_through_the_hosts_baker() {
        let jobs = Jobs::plain();
        let job = jobs.spawn("Bake", |cx| Ok(cx.bake(TriMesh::default(), Some(vec![1.0]))));
        match job.wait() {
            Ok(MeshData::Plain { mask: Some(mask), .. }) => assert_eq!(mask, vec![1.0]),
            other => panic!("unexpected {other:?}"),
        }
    }
}
