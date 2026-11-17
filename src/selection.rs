//! Turning a list of selected files into a list of recordings to convert.
//!
//! A file manager passes every selected file, and users typically select
//! all WAV files of a folder; the shell does the same for `h2n2flac *.WAV`.
//! That names a recording several times: both `SR001MS.WAV` and
//! `SR001XY.WAV`, and, if the recorder split a long recording, every part
//! of it (see [`crate::join`]). Each recording must be converted once only.

use crate::convert::chain_of;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

/// A name that is the same for every spelling of the path of a file
/// (`a.wav`, `./a.wav`, `/home/me/a.wav`), if the file exists. A file that
/// doesn't exist is its own name.
fn identity(path: &str) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path))
}

/// All files that belong to the same recording as `name`: both microphone
/// pairs and, with `join`, every part of a split recording. Files that
/// don't exist may be included; they simply never match anything.
///
/// Unlike `Recording::new()`, this never panics: errors are reported when
/// the recording is converted, where the GUI can attribute them to a file.
fn recording_files(name: &str, join: bool) -> Vec<String> {
    let given = chain_of(name, join);
    // Only the first part carries the H2n's name (see Recording::new()).
    let first = given[0].clone();
    let mut files = given;
    for sibling in [
        first.replace("XY.WAV", "MS.WAV"),
        first.replace("MS.WAV", "XY.WAV"),
    ] {
        if sibling != first {
            files.extend(chain_of(&sibling, join));
        }
    }
    files
}

/// `files` without the files that belong to a recording named earlier in
/// the list. The order is kept, so recordings are converted in the order
/// they were selected. With `join`, the parts of a split recording count
/// as one recording.
pub fn unique_recordings<S: AsRef<str>>(files: &[S], join: bool) -> Vec<String> {
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut unique = Vec::new();
    for file in files.iter().map(|f| f.as_ref()) {
        if seen.contains(&identity(file)) {
            continue;
        }
        seen.insert(identity(file));
        seen.extend(recording_files(file, join).iter().map(|f| identity(f)));
        unique.push(file.to_string());
    }
    unique
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convert::tests::{PARTS, RATE, arg, signal, test_dir, write_split};
    use crate::convert::{OutputFormat, Recording};
    use crate::join;

    #[test]
    fn ms_and_xy_file_of_one_recording_are_converted_once() {
        let files = [
            "/a/SR001MS.WAV",
            "/a/SR002XY.WAV",
            "/a/SR001XY.WAV",
            "/a/SR002MS.WAV",
            "/a/SR003XY.WAV",
        ];
        for join in [true, false] {
            assert_eq!(
                unique_recordings(&files, join),
                ["/a/SR001MS.WAV", "/a/SR002XY.WAV", "/a/SR003XY.WAV"]
            );
        }
    }

    #[test]
    fn same_name_in_other_folders_are_different_recordings() {
        let files = ["/a/SR001MS.WAV", "/b/SR001XY.WAV"];
        assert_eq!(unique_recordings(&files, true), files);
    }

    #[test]
    fn other_files_are_kept_once() {
        let files = ["notes.wav", "notes.wav", "SR001XY.WAV"];
        assert_eq!(
            unique_recordings(&files, true),
            ["notes.wav", "SR001XY.WAV"]
        );
    }

    #[test]
    fn every_split_recording_is_converted_once() {
        let dir = test_dir("plan");
        write_split(
            &dir,
            &["SR040MS.WAV", "SR041MS.WAV"],
            &PARTS[..2],
            0,
            1.0,
            0,
        );
        write_split(
            &dir,
            &["SR040XY.WAV", "SR041XY.WAV"],
            &PARTS[..2],
            0,
            1.0,
            0,
        );
        join::write_bwf(
            &dir.join("SR050XY.WAV"),
            2,
            RATE as u32,
            10,
            Some(9_000_000),
            signal,
        );
        // What `h2n2flac *.WAV` passes, in shell order; one spelled
        // differently to show that paths are compared as files.
        let files: Vec<String> = [
            "SR040MS.WAV",
            "SR040XY.WAV",
            "SR041MS.WAV",
            "SR041XY.WAV",
            "SR050XY.WAV",
        ]
        .iter()
        .map(|n| arg(&dir.join(n)))
        .chain([arg(&dir.join(".").join("SR041XY.WAV"))])
        .collect();

        let unique = unique_recordings(&files, true);
        let recordings: Vec<Recording> = unique
            .iter()
            .map(|f| Recording::new(f, OutputFormat::Flac, true))
            .collect();
        let outputs: Vec<&str> = recordings.iter().map(|r| r.output()).collect();
        assert_eq!(
            outputs,
            vec![arg(&dir.join("SR040.flac")), arg(&dir.join("SR050.flac"))]
        );
        assert_eq!(recordings[0].xy.len(), 2);
        assert_eq!(recordings[0].ms.len(), 2);

        // Without joining, only the second file of each surround pair is
        // left out.
        assert_eq!(unique_recordings(&files, false).len(), 3);
        let _ = fs::remove_dir_all(&dir);
    }
}
