//! Converting a batch of files on a worker thread while a window shows the
//! progress.
//!
//! This is everything the graphical front end does except drawing, so that
//! it can be tested without a display: the worker thread runs
//! [`run_jobs`], which records its progress in a [`Shared`] state; the GUI
//! thread polls [`Shared::snapshot`] and asks for cancellation through
//! [`Shared::cancel`].

use crate::convert::{Converted, OutputFormat, Recording, h2n2flac};
use crate::progress::{ProgressSink, percent};
use std::any::Any;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

/// The panic payload that aborts a conversion when the user cancels it.
///
/// Unwinding is the simplest way out of the conversion loops, and it closes
/// the files and removes the half-written `.part` file on the way.
#[derive(Debug)]
pub struct Cancelled;

/// What happened to one selected file.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Converted to the named output file.
    Converted(String),
    /// Another process converts this recording right now.
    AlreadyRunning,
    /// Not converted; the text says why.
    Failed(String),
    /// The user cancelled before or while this file was converted.
    Cancelled,
}

/// The state of a batch, as shown in the window.
#[derive(Debug, Clone, Default)]
pub struct Status {
    /// The files to convert, in order.
    pub files: Vec<String>,
    /// The file being converted (an index into `files`).
    pub current: usize,
    /// How far the current file has got, in percent.
    pub file_percent: u64,
    /// One entry per finished file, in the order of `files`.
    pub outcomes: Vec<Outcome>,
    /// True once the worker thread is done with all files.
    pub finished: bool,
}

impl Status {
    /// Progress of the whole batch, in percent: finished files count fully,
    /// the current one by its own progress.
    pub fn overall_percent(&self) -> u64 {
        let n = self.files.len() as u64;
        let finished = self.outcomes.len() as u64;
        if n == 0 || finished >= n {
            100
        } else {
            (finished * 100 + self.file_percent.min(100)) / n
        }
    }

    /// The line above the progress bars, e.g. `Converting SR001XY.WAV (2 of 5)`.
    pub fn headline(&self) -> String {
        if self.finished {
            if self.has_failures() {
                "Finished with errors".to_string()
            } else if self.outcomes.contains(&Outcome::Cancelled) {
                "Cancelled".to_string()
            } else {
                "Done".to_string()
            }
        } else if let Some(file) = self.files.get(self.current) {
            format!(
                "Converting {} ({} of {})",
                display_name(file),
                self.current + 1,
                self.files.len()
            )
        } else {
            "Starting…".to_string()
        }
    }

    /// True if any file failed: then the window stays open so that the user
    /// can read why. Otherwise it closes by itself when done.
    pub fn has_failures(&self) -> bool {
        self.outcomes
            .iter()
            .any(|o| matches!(o, Outcome::Failed(_)))
    }

    /// One line per file that the user should hear about: everything except
    /// plain successes.
    pub fn report(&self) -> String {
        let mut lines = Vec::new();
        for (file, outcome) in self.files.iter().zip(&self.outcomes) {
            let name = display_name(file);
            match outcome {
                Outcome::Converted(_) => {}
                Outcome::AlreadyRunning => {
                    lines.push(format!("{name}: already being converted elsewhere"))
                }
                Outcome::Failed(why) => lines.push(format!("{name}: {why}")),
                Outcome::Cancelled => lines.push(format!("{name}: cancelled")),
            }
        }
        lines.join("\n")
    }
}

/// The file name without its folder, for display.
fn display_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

/// State shared between the worker thread and the window.
#[derive(Debug, Default)]
pub struct Shared {
    status: Mutex<Status>,
    cancel: AtomicBool,
}

impl Shared {
    pub fn new(files: &[String]) -> Arc<Shared> {
        Arc::new(Shared {
            status: Mutex::new(Status {
                files: files.to_vec(),
                ..Status::default()
            }),
            cancel: AtomicBool::new(false),
        })
    }

    /// Asks the worker to stop as soon as possible. Files that are already
    /// converted stay; the current one is abandoned.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// A copy of the current state, so that the GUI doesn't hold the lock
    /// while it draws.
    pub fn snapshot(&self) -> Status {
        self.lock().clone()
    }

    fn lock(&self) -> MutexGuard<'_, Status> {
        // No code panics while holding the lock, but if it ever did, the
        // status is still good enough to display.
        self.status.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The progress of one file, reported to the [`Shared`] state.
struct JobProgress<'a> {
    shared: &'a Shared,
    total: u64,
    done: u64,
}

impl ProgressSink for JobProgress<'_> {
    fn set_total(&mut self, total: u64) {
        self.total = total;
    }

    fn advance(&mut self, units: u64) {
        // Checked here because the conversion calls this every few
        // milliseconds; panicking unwinds out of it (see `Cancelled`).
        if self.shared.is_cancelled() {
            panic::panic_any(Cancelled);
        }
        self.done = self.done.saturating_add(units).min(self.total);
        self.shared.lock().file_percent = percent(self.done, self.total);
    }

    fn finish(&mut self) {
        self.shared.lock().file_percent = 100;
    }
}

/// The message of a panic, for showing it to the user.
fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown error".to_string()
    }
}

/// Converts one file, turning panics into an [`Outcome`].
fn run_job(shared: &Shared, file: &str, normalize: bool, format: OutputFormat) -> Outcome {
    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        let recording = Recording::new(file, format);
        let mut progress = JobProgress {
            shared,
            total: 0,
            done: 0,
        };
        match h2n2flac(normalize, &recording, format, &mut progress) {
            Converted::Done => Outcome::Converted(recording.output().to_string()),
            Converted::AlreadyRunning => Outcome::AlreadyRunning,
        }
    }));
    match result {
        Ok(outcome) => outcome,
        Err(payload) if payload.is::<Cancelled>() => Outcome::Cancelled,
        Err(payload) => Outcome::Failed(panic_message(payload.as_ref())),
    }
}

/// Converts all files of `shared`, one after the other, recording the
/// progress and the outcome of each. Meant to run on a worker thread.
pub fn run_jobs(shared: &Shared, normalize: bool, format: OutputFormat) {
    let files = shared.lock().files.clone();
    for (index, file) in files.iter().enumerate() {
        {
            let mut status = shared.lock();
            status.current = index;
            status.file_percent = 0;
        }
        let outcome = if shared.is_cancelled() {
            Outcome::Cancelled
        } else {
            run_job(shared, file, normalize, format)
        };
        shared.lock().outcomes.push(outcome);
    }
    shared.lock().finished = true;
}

/// The name of the thread that should run [`run_jobs`], so that
/// [`quiet_worker_panics`] can recognize it.
pub const WORKER_THREAD: &str = "converter";

/// Installs a panic hook that keeps quiet about panics on the
/// [`WORKER_THREAD`]: [`run_jobs`] catches them and reports them as
/// [`Outcome`]s, so printing them (with a backtrace, if enabled) as well
/// would only be noise. Panics on other threads are reported as before.
pub fn quiet_worker_panics() {
    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        if std::thread::current().name() != Some(WORKER_THREAD) {
            default_hook(info);
        }
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convert::part_name;
    use crate::convert::tests::{FRAMES, read_all, signal, test_dir, write_wav};
    use std::fs;

    fn status(files: usize, outcomes: Vec<Outcome>, file_percent: u64) -> Status {
        Status {
            files: (1..=files).map(|i| format!("/x/SR00{i}XY.WAV")).collect(),
            current: outcomes.len(),
            file_percent,
            outcomes,
            finished: false,
        }
    }

    #[test]
    fn overall_percent_weighs_all_files_equally() {
        assert_eq!(status(4, vec![], 0).overall_percent(), 0);
        assert_eq!(status(4, vec![], 100).overall_percent(), 25);
        let one_done = vec![Outcome::Converted("a".into())];
        assert_eq!(status(4, one_done.clone(), 0).overall_percent(), 25);
        assert_eq!(status(4, one_done, 50).overall_percent(), 37);
        assert_eq!(status(0, vec![], 0).overall_percent(), 100);
    }

    #[test]
    fn headline_names_the_current_file_without_its_folder() {
        assert_eq!(
            status(3, vec![Outcome::Cancelled], 0).headline(),
            "Converting SR002XY.WAV (2 of 3)"
        );
        let mut done = status(1, vec![Outcome::Converted("a".into())], 0);
        done.finished = true;
        assert_eq!(done.headline(), "Done");
        done.outcomes = vec![Outcome::Failed("x".into())];
        assert_eq!(done.headline(), "Finished with errors");
    }

    #[test]
    fn report_lists_everything_but_successes() {
        let s = status(
            4,
            vec![
                Outcome::Converted("SR001.flac".into()),
                Outcome::AlreadyRunning,
                Outcome::Failed("No H2n filename found!".into()),
                Outcome::Cancelled,
            ],
            0,
        );
        assert_eq!(
            s.report(),
            "SR002XY.WAV: already being converted elsewhere\n\
             SR003XY.WAV: No H2n filename found!\n\
             SR004XY.WAV: cancelled"
        );
        assert!(s.has_failures());
    }

    #[test]
    fn run_jobs_converts_all_and_reports_failures_per_file() {
        let dir = test_dir("jobs");
        let good = dir.join("SR001XY.WAV");
        write_wav(&good, 2, FRAMES, signal);
        let not_h2n = dir.join("notes.wav");
        write_wav(&not_h2n, 2, 10, signal);
        let files: Vec<String> = [&not_h2n, &good]
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let shared = Shared::new(&files);

        run_jobs(&shared, false, OutputFormat::Flac);

        let status = shared.snapshot();
        assert!(status.finished);
        assert_eq!(
            status.outcomes[0],
            Outcome::Failed("No H2n filename found!".into())
        );
        let out = dir.join("SR001.flac").to_string_lossy().into_owned();
        assert_eq!(status.outcomes[1], Outcome::Converted(out.clone()));
        // A failure doesn't stop the batch.
        assert_eq!(read_all(&out).1.len(), FRAMES * 2);
        assert_eq!(status.overall_percent(), 100);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancelling_stops_the_batch_and_leaves_no_partial_files() {
        let dir = test_dir("cancel");
        let a = dir.join("SR001XY.WAV");
        let b = dir.join("SR002XY.WAV");
        write_wav(&a, 2, FRAMES, signal);
        write_wav(&b, 2, FRAMES, signal);
        let files = vec![
            a.to_string_lossy().into_owned(),
            b.to_string_lossy().into_owned(),
        ];
        let shared = Shared::new(&files);
        // Cancelled before the worker gets to any file, so none is started.
        // (Abandoning a file mid-conversion is covered by the next test and
        // by convert's failed_conversion_leaves_no_files_behind.)
        shared.cancel();

        run_jobs(&shared, false, OutputFormat::Flac);

        let status = shared.snapshot();
        assert_eq!(
            status.outcomes,
            vec![Outcome::Cancelled, Outcome::Cancelled]
        );
        for name in ["SR001.flac", "SR002.flac"] {
            let out = dir.join(name).to_string_lossy().into_owned();
            assert!(!Path::new(&out).exists(), "{out}");
            assert!(!Path::new(&part_name(&out)).exists(), "{out}.part");
        }
        assert_eq!(status.headline(), "Cancelled");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancelling_mid_file_unwinds_out_of_the_conversion() {
        let dir = test_dir("cancel-mid");
        let a = dir.join("SR001XY.WAV");
        write_wav(&a, 2, FRAMES, signal);
        let shared = Shared::new(&[a.to_string_lossy().into_owned()]);
        let mut progress = JobProgress {
            shared: &shared,
            total: 10,
            done: 0,
        };
        progress.advance(5);
        assert_eq!(shared.snapshot().file_percent, 50);
        shared.cancel();
        let unwound = panic::catch_unwind(AssertUnwindSafe(|| progress.advance(1)));
        assert!(unwound.unwrap_err().is::<Cancelled>());
        let _ = fs::remove_dir_all(&dir);
    }
}
