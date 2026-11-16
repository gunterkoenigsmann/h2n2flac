//! Converts the WAV files of a ZOOM H2n recorder to FLAC or Ogg Vorbis.
//!
//! The binaries are thin front ends to this library:
//! * `h2n2flac` / `h2n2ogg` (src/main.rs): the command line tools.
//! * `h2n2flac-gui` (src/bin/h2n2flac-gui.rs, needs the `gui` feature):
//!   the same conversion with a progress window, started from the
//!   right-click menu of file managers (see [`integration`]).

pub mod convert;
pub mod integration;
pub mod job;
pub mod progress;
pub mod selection;
