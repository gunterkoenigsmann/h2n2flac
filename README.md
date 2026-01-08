# h2n2flac
A rust version of h2n2flag (pre-beta stage)

## Compiling
On ubuntu: 

    sudo apt-get install libsndfile1-dev
    cargo build

[![Rust](https://github.com/gunterkoenigsmann/h2n2flac/actions/workflows/rust.yml/badge.svg)](https://github.com/gunterkoenigsmann/h2n2flac/actions/workflows/rust.yml)

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
