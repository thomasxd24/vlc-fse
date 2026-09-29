//! The download queue: one job (a planned file or folder) at a time, each file written to
//! `<name>.part` and renamed once complete, so the library scan never picks up half a film. Direct
//! port of `src/transfers.js`.
//!
//! Unlike most of the deferred stateful classes elsewhere in this migration, this one *is* ported: the
//! JS version's own test (`test/remote.test.js`: "transfer queue: cancelling stops the download and
//! leaves no finished file behind") already drives it through a fully fake, injected `client` object —
//! no real SFTP/FTP connection needed — proving the design is already testable without `remote.js`'s
//! actual network client. That test, and a second one covering the successful-download and
//! skip-already-downloaded paths it doesn't reach, are ported here against an equivalent fake client. A
//! third JS test ("downloads a planned season pack... then skips it next time") isn't ported: it needs a
//! real SFTP round trip through `remote.js`, which is deferred (see `MIGRATION.md`) pending a decision
//! on an SSH/FTP crate and the Tauri app crate's async runtime.
//!
//! Runs each job on one dedicated worker thread (spawned once, for the queue's lifetime) rather than the
//! JS version's async/event-loop concurrency — `RemoteClient::download` is a blocking call here, matching
//! the "blocking core" approach used throughout this crate.

use crate::transfer_plan::{Kind, Plan, PlanItem};
use sha1::{Digest, Sha1};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const KEEP_FINISHED: usize = 20;

/// What `remote.rs`'s `connect()` returns: something that can fetch one file and be closed (which
/// aborts whatever it's doing, same as the JS version relying on `client.close()` to abort an
/// in-flight download on cancel).
pub use crate::remote::RemoteClient;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Queued,
    Running,
    Done,
    Error,
    Cancelled,
}

/// What the UI shows.
#[derive(Debug, Clone)]
pub struct JobState {
    pub id: String,
    pub server_id: String,
    pub title: String,
    pub kind: Kind,
    pub status: JobStatus,
    pub files: usize,
    pub file_index: usize,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub rate: u64,
    pub error: Option<String>,
    pub folders: Vec<PathBuf>,
    pub skipped: u32,
}

struct Job {
    state: JobState,
    items: Vec<PlanItem>,
}

pub enum TransferEvent {
    Update(Vec<JobState>),
    Finished(JobState),
}

/// Queue a planned download (see `transfer_plan.rs`).
pub struct AddSpec {
    pub server_id: String,
    pub title: String,
    pub plan: Plan,
}

type ConnectFn = dyn Fn(&str) -> Result<Arc<dyn RemoteClient>, String> + Send + Sync;

struct Inner {
    jobs: Mutex<Vec<Job>>,
    current_client: Mutex<Option<Arc<dyn RemoteClient>>>,
    connect: Box<ConnectFn>,
    on_event: Box<dyn Fn(TransferEvent) + Send + Sync>,
    poke: Mutex<Sender<()>>,
}

pub struct TransferQueue {
    inner: Arc<Inner>,
}

fn now_millis() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn random_hex_id() -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut hasher = Sha1::new();
    hasher.update(format!("{}-{n}", now_millis()));
    hex::encode(hasher.finalize())[..12].to_string()
}

fn part_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_os_string();
    s.push(".part");
    PathBuf::from(s)
}

fn emit_update(inner: &Inner) {
    let snapshot: Vec<JobState> = inner.jobs.lock().unwrap().iter().map(|j| j.state.clone()).collect();
    (inner.on_event)(TransferEvent::Update(snapshot));
}

impl TransferQueue {
    pub fn new(connect: impl Fn(&str) -> Result<Arc<dyn RemoteClient>, String> + Send + Sync + 'static, on_event: impl Fn(TransferEvent) + Send + Sync + 'static) -> Self {
        let (tx, rx) = channel();
        let inner = Arc::new(Inner { jobs: Mutex::new(Vec::new()), current_client: Mutex::new(None), connect: Box::new(connect), on_event: Box::new(on_event), poke: Mutex::new(tx) });
        let worker_inner = Arc::clone(&inner);
        thread::spawn(move || run_worker(worker_inner, rx));
        TransferQueue { inner }
    }

    pub fn state(&self) -> Vec<JobState> {
        self.inner.jobs.lock().unwrap().iter().map(|j| j.state.clone()).collect()
    }

    pub fn active(&self) -> bool {
        self.inner.jobs.lock().unwrap().iter().any(|j| matches!(j.state.status, JobStatus::Queued | JobStatus::Running))
    }

    pub fn add(&self, spec: AddSpec) -> String {
        let id = random_hex_id();
        let job = Job {
            state: JobState {
                id: id.clone(),
                server_id: spec.server_id,
                title: spec.title,
                kind: spec.plan.kind,
                status: JobStatus::Queued,
                files: spec.plan.items.len(),
                file_index: 0,
                bytes_done: 0,
                bytes_total: spec.plan.total_size,
                rate: 0,
                error: None,
                folders: spec.plan.folders.clone(),
                skipped: 0,
            },
            items: spec.plan.items,
        };
        self.inner.jobs.lock().unwrap().push(job);
        emit_update(&self.inner);
        let _ = self.inner.poke.lock().unwrap().send(());
        id
    }

    pub fn cancel(&self, id: &str) {
        {
            let mut jobs = self.inner.jobs.lock().unwrap();
            if let Some(job) = jobs.iter_mut().find(|j| j.state.id == id) {
                match job.state.status {
                    JobStatus::Queued => job.state.status = JobStatus::Cancelled,
                    JobStatus::Running => {
                        job.state.status = JobStatus::Cancelled;
                        // Aborts the file being downloaded.
                        if let Some(client) = self.inner.current_client.lock().unwrap().clone() {
                            client.close();
                        }
                    }
                    _ => {}
                }
            }
        }
        emit_update(&self.inner);
    }

    /// Run a failed or cancelled job again, as a new job (files already downloaded are skipped).
    pub fn retry(&self, id: &str) -> Option<String> {
        let job = {
            let mut jobs = self.inner.jobs.lock().unwrap();
            let idx = jobs.iter().position(|j| j.state.id == id)?;
            if matches!(jobs[idx].state.status, JobStatus::Queued | JobStatus::Running) {
                return None;
            }
            jobs.remove(idx)
        };
        let spec = AddSpec {
            server_id: job.state.server_id,
            title: job.state.title,
            plan: Plan { kind: job.state.kind, root: None, items: job.items, total_size: job.state.bytes_total, folders: job.state.folders },
        };
        Some(self.add(spec))
    }

    /// Forget finished, failed and cancelled jobs (or just one of them).
    pub fn clear(&self, id: Option<&str>) {
        {
            let mut jobs = self.inner.jobs.lock().unwrap();
            jobs.retain(|j| (if let Some(id) = id { j.state.id != id } else { false }) || matches!(j.state.status, JobStatus::Queued | JobStatus::Running));
        }
        emit_update(&self.inner);
    }
}

fn run_worker(inner: Arc<Inner>, poke_rx: std::sync::mpsc::Receiver<()>) {
    loop {
        let idx = { inner.jobs.lock().unwrap().iter().position(|j| j.state.status == JobStatus::Queued) };
        let Some(idx) = idx else {
            if poke_rx.recv().is_err() {
                return; // the queue (and its Sender) was dropped
            }
            continue;
        };
        {
            inner.jobs.lock().unwrap()[idx].state.status = JobStatus::Running;
        }
        emit_update(&inner);

        let result = run_job(&inner, idx);

        let finished_state = {
            let mut jobs = inner.jobs.lock().unwrap();
            let job = &mut jobs[idx];
            if job.state.status == JobStatus::Running {
                match result {
                    Ok(()) => job.state.status = JobStatus::Done,
                    Err(e) => {
                        job.state.status = JobStatus::Error;
                        job.state.error = Some(e);
                    }
                }
            }
            let snapshot = job.state.clone();
            // Keep the list short: old finished jobs drop off.
            let finished_positions: Vec<bool> = jobs.iter().map(|j| !matches!(j.state.status, JobStatus::Queued | JobStatus::Running)).collect();
            let finished_count = finished_positions.iter().filter(|&&f| f).count();
            if finished_count > KEEP_FINISHED {
                let mut to_drop = finished_count - KEEP_FINISHED;
                let mut i = 0;
                jobs.retain(|j| {
                    let is_finished = !matches!(j.state.status, JobStatus::Queued | JobStatus::Running);
                    let drop_this = is_finished && to_drop > 0 && { i += 1; true };
                    if drop_this {
                        to_drop -= 1;
                        false
                    } else {
                        true
                    }
                });
            }
            snapshot
        };
        *inner.current_client.lock().unwrap() = None;
        emit_update(&inner);
        (inner.on_event)(TransferEvent::Finished(finished_state));
    }
}

fn run_job(inner: &Inner, idx: usize) -> Result<(), String> {
    let server_id = inner.jobs.lock().unwrap()[idx].state.server_id.clone();
    let client = (inner.connect)(&server_id)?;
    *inner.current_client.lock().unwrap() = Some(Arc::clone(&client));

    let mut before: u64 = 0;
    let mut last_tick = (Instant::now(), 0u64);
    let item_count = inner.jobs.lock().unwrap()[idx].items.len();

    for i in 0..item_count {
        if inner.jobs.lock().unwrap()[idx].state.status != JobStatus::Running {
            return Ok(());
        }
        let item = inner.jobs.lock().unwrap()[idx].items[i].clone();
        inner.jobs.lock().unwrap()[idx].state.file_index = i;

        let existing_size = std::fs::metadata(&item.dest).ok().map(|m| m.len());
        if existing_size == Some(item.size) {
            // Already there (an earlier download, or the same file from another source).
            let mut jobs = inner.jobs.lock().unwrap();
            let j = &mut jobs[idx];
            j.state.skipped += 1;
            before += item.size;
            j.state.bytes_done = before;
            drop(jobs);
            emit_update(inner);
            continue;
        }

        if let Some(parent) = item.dest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let part = part_path(&item.dest);
        let before_snapshot = before;
        let download_result = client.download(&item.remote, &part, &mut |bytes: u64| {
            let mut jobs = inner.jobs.lock().unwrap();
            let j = &mut jobs[idx];
            j.state.bytes_done = before_snapshot + bytes;
            let now = Instant::now();
            if now.duration_since(last_tick.0) >= Duration::from_secs(1) {
                let elapsed = now.duration_since(last_tick.0).as_secs_f64();
                j.state.rate = ((j.state.bytes_done.saturating_sub(last_tick.1)) as f64 / elapsed).round() as u64;
                last_tick = (now, j.state.bytes_done);
            }
            drop(jobs);
            emit_update(inner);
        });
        download_result?;

        if inner.jobs.lock().unwrap()[idx].state.status != JobStatus::Running {
            return Ok(());
        }
        crate::fsops::rename_replace(&part, &item.dest).map_err(|e| e.to_string())?;
        before += item.size;
        let mut jobs = inner.jobs.lock().unwrap();
        jobs[idx].state.bytes_done = before;
        drop(jobs);
        emit_update(inner);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transfer_plan::Plan;
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc::{channel, RecvTimeoutError};
    use tempfile::tempdir;

    fn wait_finished(rx: &std::sync::mpsc::Receiver<JobState>) -> JobState {
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(j) => j,
            Err(RecvTimeoutError::Timeout) => panic!("timed out waiting for the job to finish"),
            Err(e) => panic!("{e}"),
        }
    }

    /// A client whose `download` blocks (having already written a partial file and reported some
    /// bytes) until `close()` releases it, then fails — mirroring the JS test's fake client exactly.
    struct BlockingThenFailClient {
        closed: Arc<AtomicBool>,
        release: Mutex<Option<Sender<()>>>,
    }
    impl RemoteClient for BlockingThenFailClient {
        fn download(&self, _remote: &str, local: &Path, on_bytes: &mut dyn FnMut(u64)) -> Result<(), String> {
            std::fs::write(local, "part").unwrap();
            on_bytes(4);
            let (tx, rx) = channel();
            *self.release.lock().unwrap() = Some(tx);
            let _ = rx.recv(); // blocks until close() sends
            Err("connection closed".to_string())
        }
        fn close(&self) {
            self.closed.store(true, Ordering::SeqCst);
            if let Some(tx) = self.release.lock().unwrap().take() {
                let _ = tx.send(());
            }
        }
    }

    // Parity with "transfer queue: cancelling stops the download and leaves no finished file behind".
    #[test]
    fn cancelling_stops_the_download_and_leaves_no_finished_file_behind() {
        let lib = tempdir().unwrap();
        let dest = lib.path().join("Movies").join("Heat (1995)").join("Heat.mkv");
        let closed = Arc::new(AtomicBool::new(false));
        let client = Arc::new(BlockingThenFailClient { closed: Arc::clone(&closed), release: Mutex::new(None) });

        let (fin_tx, fin_rx) = channel();
        let client_for_connect = Arc::clone(&client);
        let queue = TransferQueue::new(
            move |_server_id| Ok(client_for_connect.clone() as Arc<dyn RemoteClient>),
            move |e| {
                if let TransferEvent::Finished(j) = e {
                    let _ = fin_tx.send(j);
                }
            },
        );

        let plan = Plan {
            kind: Kind::Movie,
            root: None,
            folders: vec![],
            total_size: 10,
            items: vec![PlanItem { remote: "/Heat.mkv".into(), rel: "Heat.mkv".into(), size: 10, dest: dest.clone() }],
        };
        let id = queue.add(AddSpec { server_id: "s".into(), title: "Heat".into(), plan });
        thread::sleep(Duration::from_millis(20));
        queue.cancel(&id);
        let job = wait_finished(&fin_rx);

        assert_eq!(job.status, JobStatus::Cancelled);
        assert!(closed.load(Ordering::SeqCst));
        assert!(!dest.exists());
        assert!(part_path(&dest).exists(), "kept so FTP can resume it");
    }

    /// A client that "downloads" by copying bytes from an in-memory map keyed by remote path — enough
    /// to exercise the successful-download and skip-already-downloaded paths the cancellation test
    /// above doesn't reach, without needing a real SFTP/FTP connection.
    struct InMemoryClient {
        files: std::collections::HashMap<String, Vec<u8>>,
    }
    impl RemoteClient for InMemoryClient {
        fn download(&self, remote: &str, local: &Path, on_bytes: &mut dyn FnMut(u64)) -> Result<(), String> {
            let data = self.files.get(remote).ok_or_else(|| "not found".to_string())?;
            std::fs::write(local, data).map_err(|e| e.to_string())?;
            on_bytes(data.len() as u64);
            Ok(())
        }
        fn close(&self) {}
    }

    #[test]
    fn downloads_a_planned_movie_and_skips_it_on_a_second_run() {
        let lib = tempdir().unwrap();
        let dest = lib.path().join("Movies").join("Heat (1995)").join("Heat.mkv");
        let mut files = std::collections::HashMap::new();
        files.insert("/Heat.mkv".to_string(), b"movie bytes".to_vec());
        let client: Arc<dyn RemoteClient> = Arc::new(InMemoryClient { files });

        let (fin_tx, fin_rx) = channel();
        let client_for_connect = Arc::clone(&client);
        let queue = TransferQueue::new(move |_| Ok(client_for_connect.clone()), move |e| if let TransferEvent::Finished(j) = e { let _ = fin_tx.send(j); });

        let plan = || Plan { kind: Kind::Movie, root: None, folders: vec![], total_size: 11, items: vec![PlanItem { remote: "/Heat.mkv".into(), rel: "Heat.mkv".into(), size: 11, dest: dest.clone() }] };

        queue.add(AddSpec { server_id: "s".into(), title: "Heat".into(), plan: plan() });
        let job = wait_finished(&fin_rx);
        assert_eq!(job.status, JobStatus::Done);
        assert_eq!(job.bytes_done, 11);
        assert_eq!(std::fs::read(&dest).unwrap(), b"movie bytes");
        assert!(!part_path(&dest).exists());

        // Second run: the file's already there with the right size, so it's skipped, not re-downloaded.
        queue.add(AddSpec { server_id: "s".into(), title: "Heat".into(), plan: plan() });
        let job = wait_finished(&fin_rx);
        assert_eq!(job.status, JobStatus::Done);
        assert_eq!(job.skipped, 1);
    }
}
