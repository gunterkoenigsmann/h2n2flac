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

## Long recordings split into several files
The H2n ends a WAV file at about 2 GB and seamlessly continues the
recording in a new file. h2n2flac detects such files and joins them to
one output file, no matter which of the parts is named on the command
line. Joining is on by default; `--no-join` converts just the named file.

The parts are recognised by their Broadcast WAV metadata, not by their
names: a file continues another one if its time reference (the position
of its first sample in the `bext` chunk) is exactly where the other one
ends, and if both have the same sample rate, channel count and sample
format. In surround mode, "XY" or "MS" in the file name keeps the
continuations of the two microphone pairs apart. Plain WAV files without
a `bext` chunk are never joined.

`h2n2flac *.WAV` converts every recording once: files that are a part of
a recording that was already converted (a continuation, or the second
file of an XY/MS pair) are skipped.

## Progress bar
While converting, a progress bar is shown on stderr, but only if stderr
is a terminal. When the output is redirected (scripts, cron jobs, logs)
the program stays quiet.

## Development
CI runs `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`
and `cargo test` on every pull request; running them locally before
pushing saves a round trip.
