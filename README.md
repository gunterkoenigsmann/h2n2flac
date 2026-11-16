# h2n2flac
A rust version of h2n2flag (pre-beta stage)

## Compiling
On ubuntu: 

    sudo apt-get install libsndfile1-dev
    cargo build

[![CI](https://github.com/gunterkoenigsmann/h2n2flac/actions/workflows/ci.yml/badge.svg)](https://github.com/gunterkoenigsmann/h2n2flac/actions/workflows/ci.yml)

## What is it all about?

The ZOOM H2n is a small, inexpensive but reasonable rugged recording device 
that contains two recording setups:
 * One front-facing xy stereo recorder and
 * One back-facing ms stereo recorder
 .

Both can produce WAV files and can be enabled at the same time making it a
surround recorder.

This program now if given the name of one of the two stereo wav files
produced this way combines them to a single 4-channel audio file that can
easily processed by audio software.

If only a single file is given to this program it will converted
to a compressed format and, if that is requested, be normalized, as well.

## Output format
 * FLAC, if the program name ends in "flac"
 * OGG, if the program name ends in "ogg"
 .
Since cargo canot create symlinks h2n2flac and h2n2ogg are generated as
separate binaries, currently.

## Progress bar
While converting, a progress bar is shown on stderr, but only if stderr
is a terminal. When the output is redirected (scripts, cron jobs, logs)
the program stays quiet.

Giving both files of a recording (`h2n2flac *.WAV`) converts it once.

The output is written to `SR001.flac.part` first and renamed when it is
complete, so an interrupted conversion never leaves a truncated
`SR001.flac`. A `.part` file that is still being written tells a second
h2n2flac to leave that recording alone; one that hasn't changed for two
minutes is a leftover and gets replaced.

## Right-click menu with a progress window
`h2n2flac-gui` converts like `h2n2flac`, but shows its progress in a
window (made with [wxDragon](https://crates.io/crates/wxdragon)). The
window closes by itself when all went well, and stays open listing the
files that failed otherwise. *Cancel* stops and removes the half-written
file. Options: `--ogg` for Ogg Vorbis instead of FLAC, `-n` to normalize.

It isn't built by default, because wxDragon compiles wxWidgets from
source, which takes a few minutes and needs a few more packages:

    sudo apt-get install libsndfile1-dev libgtk-3-dev cmake libclang-dev
    cargo build --release --features gui

Then

    target/release/h2n2flac --install-menu

adds a submenu "H2n recording" with *Convert to FLAC*, *Convert to Ogg
Vorbis* and normalized variants of both to the right-click menu of WAV
files in

 * Dolphin (KDE Plasma 5.85 or newer):
   `~/.local/share/kio/servicemenus/h2n2flac.desktop`
 * GNOME Files (Nautilus) and Nemo, under *Scripts*:
   `~/.local/share/{nautilus,nemo}/scripts/H2n recording/`
 * Windows Explorer: registry entries below
   `HKEY_CURRENT_USER\Software\Classes\SystemFileAssociations\.wav`.
   On Windows 11 they are under *Show more options*.

Nothing needs administrator rights. The entries point to the
`h2n2flac-gui` next to the `h2n2flac` that installed them; after moving the
programs, run `--install-menu` again. `h2n2flac --uninstall-menu` removes
the entries.

Dolphin, Nautilus and Nemo start one window for all selected files.
Windows Explorer starts one per file; if both files of a recording are
selected, one of the two windows sees that the other one is converting it
and closes.

## Development
CI runs `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`
and `cargo test` on every pull request; running them locally before
pushing saves a round trip. A separate job does the same with
`--features gui` and converts a file through the window on a virtual
display (`xvfb-run`).

The conversion lives in a library (`src/lib.rs`); `src/main.rs` and
`src/bin/h2n2flac-gui.rs` are thin front ends. `src/job.rs` holds all of
the window's logic except the drawing, so it is tested without a display.
