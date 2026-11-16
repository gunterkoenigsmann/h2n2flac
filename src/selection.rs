//! Turning a list of selected files into a list of recordings to convert.
//!
//! A file manager passes every selected file, and users typically select
//! all WAV files of a folder: both `SR001MS.WAV` and `SR001XY.WAV`. Both
//! name the same recording, which must be converted once only.

/// The name that identifies the recording a file belongs to: the name of
/// its MS file. Files that aren't named like H2n recordings are their own
/// key, so they are kept (and later reported as not being H2n files).
fn recording_key(name: &str) -> String {
    name.replace("XY.WAV", "MS.WAV")
}

/// `files` without the files that belong to a recording named earlier in
/// the list. The order is kept, so recordings are converted in the order
/// they were selected.
pub fn unique_recordings<S: AsRef<str>>(files: &[S]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    files
        .iter()
        .map(|f| f.as_ref())
        .filter(|f| seen.insert(recording_key(f)))
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ms_and_xy_file_of_one_recording_are_converted_once() {
        let files = [
            "/a/SR001MS.WAV",
            "/a/SR002XY.WAV",
            "/a/SR001XY.WAV",
            "/a/SR002MS.WAV",
            "/a/SR003XY.WAV",
        ];
        assert_eq!(
            unique_recordings(&files),
            ["/a/SR001MS.WAV", "/a/SR002XY.WAV", "/a/SR003XY.WAV"]
        );
    }

    #[test]
    fn same_name_in_other_folders_are_different_recordings() {
        let files = ["/a/SR001MS.WAV", "/b/SR001XY.WAV"];
        assert_eq!(unique_recordings(&files), files);
    }

    #[test]
    fn other_files_are_kept_once() {
        let files = ["notes.wav", "notes.wav", "SR001XY.WAV"];
        assert_eq!(unique_recordings(&files), ["notes.wav", "SR001XY.WAV"]);
    }
}
