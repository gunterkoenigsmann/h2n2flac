//! A minimal, dependency-free progress bar.
//!
//! The bar is drawn on stderr, and only when stderr is a terminal: when the
//! output is redirected to a file or a pipe (cron jobs, scripts, CI logs)
//! nothing is printed, so those logs don't fill up with carriage returns.
//!
//! The bar is redrawn only when the shown percentage changes, so even very
//! long recordings cause at most about a hundred writes.

use std::io::{self, IsTerminal, Write};

/// Width of the bar itself, in characters, without label and percentage.
const BAR_WIDTH: usize = 30;

/// Progress of one piece of work, measured in arbitrary units (here: audio
/// frames, counted once per pass over the data).
pub struct Progress<W: Write> {
    label: String,
    total: u64,
    done: u64,
    /// The percentage drawn last, so that unchanged states aren't redrawn.
    last_percent: Option<u64>,
    /// Where the bar is drawn; `None` disables drawing.
    out: Option<W>,
}

impl Progress<io::Stderr> {
    /// A progress bar on stderr that is shown only if stderr is a terminal.
    pub fn on_stderr(label: &str) -> Self {
        let stderr = io::stderr();
        let out = if stderr.is_terminal() {
            Some(stderr)
        } else {
            None
        };
        Progress::new(label, out)
    }
}

impl<W: Write> Progress<W> {
    /// A progress bar that draws to `out`, or nothing if `out` is `None`.
    /// The total is 0 until [`Progress::set_total`] is called.
    pub fn new(label: &str, out: Option<W>) -> Self {
        Progress {
            label: label.to_string(),
            total: 0,
            done: 0,
            last_percent: None,
            out,
        }
    }

    /// Sets the amount of work that corresponds to 100%.
    pub fn set_total(&mut self, total: u64) {
        self.total = total;
    }

    /// Records that `units` more units of work are done and redraws the bar
    /// if the percentage changed.
    pub fn advance(&mut self, units: u64) {
        self.done = self.done.saturating_add(units).min(self.total);
        self.draw();
    }

    /// Shows the bar at 100% and ends its line, so that later output starts
    /// on a fresh line.
    pub fn finish(&mut self) {
        self.done = self.total;
        self.draw();
        if let Some(out) = self.out.as_mut() {
            // A broken stderr is no reason to abort a conversion.
            let _ = writeln!(out);
            let _ = out.flush();
        }
    }

    /// The writer the bar draws to (used by the tests).
    #[cfg(test)]
    pub fn output(&self) -> Option<&W> {
        self.out.as_ref()
    }

    fn draw(&mut self) {
        let percent = percent(self.done, self.total);
        if self.last_percent == Some(percent) {
            return;
        }
        self.last_percent = Some(percent);
        if let Some(out) = self.out.as_mut() {
            let _ = write!(out, "\r{}", render(&self.label, percent));
            let _ = out.flush();
        }
    }
}

/// `done` as a percentage of `total`, rounded down. Nothing to do counts as
/// finished.
pub fn percent(done: u64, total: u64) -> u64 {
    if total == 0 {
        100
    } else {
        // u128, so that huge totals cannot overflow the multiplication.
        (u128::from(done) * 100 / u128::from(total)) as u64
    }
}

/// One line of the progress bar, e.g. `SR001MS.WAV [#######-------]  50%`.
pub fn render(label: &str, percent: u64) -> String {
    let filled = (percent.min(100) as usize * BAR_WIDTH) / 100;
    format!(
        "{} [{}{}] {:3}%",
        label,
        "#".repeat(filled),
        "-".repeat(BAR_WIDTH - filled),
        percent
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_rounds_down_and_handles_zero_total() {
        assert_eq!(percent(0, 200), 0);
        assert_eq!(percent(1, 200), 0);
        assert_eq!(percent(100, 200), 50);
        assert_eq!(percent(200, 200), 100);
        assert_eq!(percent(0, 0), 100);
        assert_eq!(percent(u64::MAX, u64::MAX), 100);
    }

    #[test]
    fn render_shows_label_bar_and_percentage() {
        let empty = render("x", 0);
        assert_eq!(empty, format!("x [{}]   0%", "-".repeat(BAR_WIDTH)));
        let full = render("x", 100);
        assert_eq!(full, format!("x [{}] 100%", "#".repeat(BAR_WIDTH)));
        let half = render("x", 50);
        assert_eq!(half.matches('#').count(), BAR_WIDTH / 2);
        // All states have the same length, so redrawing with \r leaves no
        // remains of an earlier, longer line.
        assert_eq!(empty.len(), half.len());
        assert_eq!(empty.len(), full.len());
    }

    #[test]
    fn redraws_only_when_the_percentage_changes() {
        let mut progress = Progress::new("f", Some(Vec::new()));
        progress.set_total(1000);
        for _ in 0..1000 {
            progress.advance(1);
        }
        progress.finish();
        let text = String::from_utf8(progress.output().unwrap().clone()).unwrap();
        // 0% (the first advance is 1/1000) .. 100%: one draw per percent;
        // finish() adds only the newline.
        assert_eq!(text.matches('\r').count(), 101);
        assert!(text.ends_with(&format!("{}\n", render("f", 100))));
    }

    #[test]
    fn advance_never_exceeds_the_total() {
        let mut progress = Progress::new("f", Some(Vec::new()));
        progress.set_total(10);
        progress.advance(25);
        let text = String::from_utf8(progress.output().unwrap().clone()).unwrap();
        assert!(text.ends_with("100%"));
    }

    #[test]
    fn disabled_progress_draws_nothing() {
        let mut progress: Progress<Vec<u8>> = Progress::new("f", None);
        progress.set_total(10);
        progress.advance(5);
        progress.finish();
        assert!(progress.output().is_none());
    }
}
