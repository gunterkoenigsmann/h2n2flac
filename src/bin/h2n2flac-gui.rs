//! `h2n2flac-gui`: converts the given H2n recordings like `h2n2flac` does,
//! but shows the progress in a window. Meant to be started from the
//! right-click menu of a file manager (`h2n2flac --install-menu`).
//!
//! The window shows the file being converted, a bar for that file and one
//! for all files. It closes by itself when everything worked; if a file
//! failed it stays open and lists why. "Cancel" (or closing the window)
//! stops after removing the half-written file.

// Without this Windows would open a console window next to ours.
#![cfg_attr(windows, windows_subsystem = "windows")]

use getopts::Options;
use h2n2flac::convert::OutputFormat;
use h2n2flac::job::{self, Shared};
use h2n2flac::selection;
use std::process::ExitCode;
use std::sync::Arc;
use std::thread;
use wxdragon::prelude::*;
use wxdragon::timer::Timer;

/// How often the window fetches the worker's progress.
const REFRESH_MS: i32 = 100;

/// Builds the window and keeps it updated from `shared` until the user (or,
/// on success, the window itself) closes it.
fn show_window(shared: Arc<Shared>, format: OutputFormat) {
    let format_name = match format {
        OutputFormat::Flac => "FLAC",
        OutputFormat::Ogg => "Ogg Vorbis",
    };
    let frame = Frame::builder()
        .with_title(&format!("H2n recording to {format_name}"))
        .with_size(Size::new(480, 230))
        .build();
    let panel = Panel::builder(&frame).build();
    let sizer = BoxSizer::builder(Orientation::Vertical).build();

    let headline = StaticText::builder(&panel).with_label("Starting…").build();
    let file_gauge = Gauge::builder(&panel).with_range(100).build();
    let overall_label = StaticText::builder(&panel).with_label("All files").build();
    let overall_gauge = Gauge::builder(&panel).with_range(100).build();
    let report = StaticText::builder(&panel).with_label("").build();
    let button = Button::builder(&panel).with_label("Cancel").build();

    let grow = SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top;
    sizer.add(&headline, 0, grow, 10);
    sizer.add(&file_gauge, 0, grow, 10);
    sizer.add(&overall_label, 0, grow, 10);
    sizer.add(&overall_gauge, 0, grow, 10);
    sizer.add(&report, 1, grow, 10);
    sizer.add(&button, 0, SizerFlag::AlignRight | SizerFlag::All, 10);
    panel.set_sizer(sizer, true);

    let on_click_shared = shared.clone();
    button.on_click(move |_| {
        if on_click_shared.snapshot().finished {
            frame.close(true);
        } else {
            on_click_shared.cancel();
            button.set_label("Cancelling…");
            button.enable(false);
        }
    });

    let timer = Timer::new(&frame);
    let mut reported = false;
    timer.on_tick(move |_| {
        let status = shared.snapshot();
        headline.set_label(&status.headline());
        file_gauge.set_value(status.file_percent.min(100) as i32);
        overall_gauge.set_value(status.overall_percent() as i32);
        if status.finished && !reported {
            reported = true;
            if status.has_failures() {
                // Stay open so the user can read what went wrong.
                report.set_label(&status.report());
                report.wrap(440);
                button.set_label("Close");
                button.enable(true);
                panel.layout();
            } else {
                frame.close(true);
            }
        }
    });
    timer.start(REFRESH_MS, false);
    // The timer has to live as long as the window, which is until the
    // program ends.
    std::mem::forget(timer);

    frame.show(true);
    frame.centre();
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let mut opts = Options::new();
    opts.optflag("h", "help", "print this help menu");
    opts.optflag("n", "normalize", "Normalize the output");
    opts.optflag("", "ogg", "Write Ogg Vorbis instead of FLAC");
    let matches = match opts.parse(&args[1..]) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    if matches.opt_present("h") || matches.free.is_empty() {
        print!(
            "{}",
            opts.usage(&format!("Usage: {} FILE(s) [options]", args[0]))
        );
        return ExitCode::SUCCESS;
    }
    let normalize = matches.opt_present("n");
    let format = if matches.opt_present("ogg") {
        OutputFormat::Ogg
    } else {
        OutputFormat::Flac
    };

    // Split recordings are always joined (see job::run_jobs), so the
    // selection is deduplicated the same way.
    let files = selection::unique_recordings(&matches.free, true);
    let shared = Shared::new(&files);
    job::quiet_worker_panics();
    let worker_shared = shared.clone();
    let worker = thread::Builder::new()
        .name(job::WORKER_THREAD.to_string())
        .spawn(move || job::run_jobs(&worker_shared, normalize, format))
        .expect("Cannot start the conversion thread");

    let window_shared = shared.clone();
    if let Err(e) = wxdragon::main(move |_| show_window(window_shared, format)) {
        // No display (e.g. started over ssh): convert anyway, without
        // a window, rather than leaving the files unconverted.
        eprintln!("Cannot open a window ({e}); converting without one.");
    } else {
        // The window was closed. If that happened before the end, it
        // means "cancel"; the worker stops and cleans up.
        shared.cancel();
    }
    // Waiting for the worker makes sure no half-written .part file stays.
    let _ = worker.join();

    let status = shared.snapshot();
    let report = status.report();
    if !report.is_empty() {
        eprintln!("{report}");
    }
    if status.has_failures() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
