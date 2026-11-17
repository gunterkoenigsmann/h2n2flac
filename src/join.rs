//! Detects recordings that the recorder split into several WAV files.
//!
//! WAV files cannot grow beyond 4 GB, and FAT32 SD cards not beyond 2 GB, so
//! the H2n ends a long recording at about 2 GB and continues it seamlessly in
//! a new file. The files are Broadcast WAV (BWF) files: their `bext` chunk
//! contains the *time reference*, the position of the file's first sample,
//! counted in samples since midnight.
//!
//! A file therefore continues another one if its time reference is exactly
//! where the other one ends:
//!
//! ```text
//! next.time_reference == previous.time_reference + previous.frames
//! ```
//!
//! This doesn't depend on how the recorder names the continuation files,
//! and it is sample-exact, so separate recordings practically never match by
//! accident. In addition, both files must have the same audio format, and
//! they must belong to the same microphone pair: in surround mode the XY and
//! the MS file of a recording start at the very same sample, so without that
//! check the continuation of the XY file could be mistaken for the
//! continuation of the MS file. The pair is recognised by "XY" or "MS" in
//! the file name, as in `SR001XY.WAV`.
//!
//! Only the headers of the files are read here; joining the audio data is
//! done by `Source` in `convert.rs`.

use std::fs::{self, File};
use std::io::{self, BufReader, ErrorKind, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Samples per day at `rate`: time references are counted since midnight,
/// so a recording that runs past midnight may continue at a small one.
fn samples_per_day(rate: u32) -> u64 {
    u64::from(rate) * 24 * 60 * 60
}

/// The properties of a WAV file that matter for detecting continuations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WavInfo {
    pub channels: u16,
    pub sample_rate: u32,
    /// Bytes per frame (one sample of every channel). Equal block alignment
    /// means equal sample formats (16 or 24 bit) for the same channel count.
    pub block_align: u16,
    pub frames: u64,
    /// The time reference from the `bext` chunk, `None` for plain WAV files.
    pub time_reference: Option<u64>,
}

impl WavInfo {
    /// True if `next` is the seamless continuation of `self`.
    pub fn is_continued_by(&self, next: &WavInfo) -> bool {
        let (Some(start), Some(next_start)) = (self.time_reference, next.time_reference) else {
            return false;
        };
        if self.channels != next.channels
            || self.sample_rate != next.sample_rate
            || self.block_align != next.block_align
            || self.frames == 0
        {
            return false;
        }
        let end = start + self.frames;
        next_start == end || next_start == end % samples_per_day(self.sample_rate)
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, message.to_string())
}

/// Offset of TimeReferenceLow in the bext chunk.
const BEXT_TIME_REFERENCE_OFFSET: usize = 256 + 32 + 32 + 10 + 8;
/// Bytes of the bext chunk up to and including TimeReferenceHigh.
const BEXT_TIME_REFERENCE_END: u64 = BEXT_TIME_REFERENCE_OFFSET as u64 + 8;

/// Reads the header chunks of a RIFF/WAVE file. The audio data itself is
/// skipped, so this is fast even for 2 GB files.
pub fn parse_wav_info<R: Read + Seek>(r: &mut R) -> io::Result<WavInfo> {
    let mut header = [0u8; 12];
    r.read_exact(&mut header)?;
    if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return Err(invalid("not a RIFF/WAVE file"));
    }

    // (channels, sample rate, block align)
    let mut format: Option<(u16, u32, u16)> = None;
    let mut data_size: Option<u64> = None;
    let mut time_reference: Option<u64> = None;

    loop {
        let mut chunk = [0u8; 8];
        match r.read_exact(&mut chunk) {
            Ok(()) => {}
            // The end of the file ends the list of chunks.
            Err(e) if e.kind() == ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e),
        }
        let id = &chunk[0..4];
        let size = u64::from(u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]));
        // How much of the chunk is still to be skipped after reading the
        // parts we are interested in.
        let mut rest = size;
        match id {
            b"fmt " => {
                if size < 16 {
                    return Err(invalid("fmt chunk too short"));
                }
                let mut b = [0u8; 16];
                r.read_exact(&mut b)?;
                format = Some((
                    u16::from_le_bytes([b[2], b[3]]),
                    u32::from_le_bytes([b[4], b[5], b[6], b[7]]),
                    u16::from_le_bytes([b[12], b[13]]),
                ));
                rest -= 16;
            }
            // Layout (EBU Tech 3285): Description[256], Originator[32],
            // OriginatorReference[32], OriginationDate[10],
            // OriginationTime[8], TimeReferenceLow (u32),
            // TimeReferenceHigh (u32), ...
            b"bext" if size >= BEXT_TIME_REFERENCE_END => {
                let mut b = [0u8; BEXT_TIME_REFERENCE_END as usize];
                r.read_exact(&mut b)?;
                let at = BEXT_TIME_REFERENCE_OFFSET;
                let low = u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
                let high = u32::from_le_bytes([b[at + 4], b[at + 5], b[at + 6], b[at + 7]]);
                time_reference = Some((u64::from(high) << 32) | u64::from(low));
                rest -= BEXT_TIME_REFERENCE_END;
            }
            b"data" => data_size = Some(size),
            _ => {}
        }
        // Chunks are padded to an even size.
        rest += size & 1;
        r.seek(SeekFrom::Current(rest as i64))?;
    }

    let (channels, sample_rate, block_align) = format.ok_or_else(|| invalid("no fmt chunk"))?;
    let data_size = data_size.ok_or_else(|| invalid("no data chunk"))?;
    if block_align == 0 {
        return Err(invalid("block align is 0"));
    }
    Ok(WavInfo {
        channels,
        sample_rate,
        block_align,
        frames: data_size / u64::from(block_align),
        time_reference,
    })
}

pub fn read_wav_info(path: &Path) -> io::Result<WavInfo> {
    parse_wav_info(&mut BufReader::new(File::open(path)?))
}

/// Which microphone pair of the H2n a file was recorded with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pair {
    Xy,
    Ms,
    /// Neither "XY" nor "MS" in the name (e.g. other recorders).
    Unknown,
}

pub fn pair_of(path: &Path) -> Pair {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_uppercase())
        .unwrap_or_default();
    if name.contains("XY") {
        Pair::Xy
    } else if name.contains("MS") {
        Pair::Ms
    } else {
        Pair::Unknown
    }
}

/// A WAV file and its header information.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub path: PathBuf,
    pub info: WavInfo,
}

/// Finds the files `start` belongs to among `candidates`, in recording
/// order: first the files it continues, then `start`, then the files that
/// continue it.
///
/// A step is taken only if exactly one candidate fits; if several do, the
/// chain ends there with a warning instead of guessing. `start` itself may
/// be among the candidates.
pub fn chain_from(start: &Candidate, candidates: &[Candidate]) -> Vec<PathBuf> {
    let pair = pair_of(&start.path);
    // The indices of the candidates that pass `test`, belong to the same
    // microphone pair and aren't in the chain yet.
    let fitting = |chain: &[&Candidate], test: &dyn Fn(&Candidate) -> bool| -> Vec<usize> {
        candidates
            .iter()
            .enumerate()
            .filter(|(_, c)| pair_of(&c.path) == pair)
            .filter(|(_, c)| !chain.iter().any(|d| d.path == c.path))
            .filter(|(_, c)| test(c))
            .map(|(i, _)| i)
            .collect()
    };

    let mut chain: Vec<&Candidate> = vec![start];

    // Forward: the files that continue the last one in the chain.
    loop {
        let last = chain[chain.len() - 1];
        let next = fitting(&chain, &|c| last.info.is_continued_by(&c.info));
        match next.as_slice() {
            [i] => chain.push(&candidates[*i]),
            [] => break,
            _ => {
                warn_ambiguous(last, &next, candidates);
                break;
            }
        }
    }

    // Backward: the files that the first one in the chain continues.
    loop {
        let first = chain[0];
        let previous = fitting(&chain, &|c| c.info.is_continued_by(&first.info));
        match previous.as_slice() {
            [i] => chain.insert(0, &candidates[*i]),
            [] => break,
            _ => {
                warn_ambiguous(first, &previous, candidates);
                break;
            }
        }
    }

    chain.into_iter().map(|c| c.path.clone()).collect()
}

fn warn_ambiguous(file: &Candidate, matches: &[usize], candidates: &[Candidate]) {
    let names: Vec<String> = matches
        .iter()
        .map(|i| candidates[*i].path.display().to_string())
        .collect();
    eprintln!(
        "Warning: several files could continue {}: {}. Not joining them.",
        file.path.display(),
        names.join(", ")
    );
}

/// The files of the recording that `path` is part of, in recording order.
///
/// Looks at all WAV files in the directory of `path`. If `path` has no
/// usable `bext` chunk, or nothing continues it, the result is just `path`.
pub fn find_chain(path: &Path) -> Vec<PathBuf> {
    let alone = vec![path.to_path_buf()];
    let Ok(info) = read_wav_info(path) else {
        return alone;
    };
    if info.time_reference.is_none() {
        return alone;
    }
    let start = Candidate {
        path: path.to_path_buf(),
        info,
    };

    // `parent()` of a bare file name is "", which read_dir() doesn't
    // accept; joining with "" keeps the names as short as the user gave them.
    let dir = path.parent().unwrap_or(Path::new(""));
    let listed_dir = if dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dir
    };
    let Ok(entries) = fs::read_dir(listed_dir) else {
        return alone;
    };
    let own_name = path.file_name();
    let candidates: Vec<Candidate> = entries
        .filter_map(Result::ok)
        .map(|e| e.file_name())
        .filter(|name| Some(name.as_os_str()) != own_name)
        .map(|name| dir.join(name))
        .filter(|p| {
            p.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("wav"))
        })
        // Files that aren't readable WAV files simply aren't candidates.
        .filter_map(|p| {
            read_wav_info(&p)
                .ok()
                .map(|info| Candidate { path: p, info })
        })
        .collect();

    chain_from(&start, &candidates)
}

/// Test helper: writes a 24-bit PCM Broadcast WAV file (or a plain WAV file
/// if `time_reference` is `None`) whose sample for (frame, channel) is
/// `sample(frame, channel)`, a value in -1..1.
#[cfg(test)]
pub fn write_bwf(
    path: &Path,
    channels: u16,
    rate: u32,
    frames: usize,
    time_reference: Option<u64>,
    sample: impl Fn(usize, usize) -> f32,
) {
    const BYTES: u16 = 3;
    let block_align = channels * BYTES;

    let mut data = Vec::with_capacity(frames * usize::from(block_align));
    for f in 0..frames {
        for c in 0..usize::from(channels) {
            let v = (sample(f, c) * 8_388_608.0)
                .round()
                .clamp(-8_388_608.0, 8_388_607.0) as i32;
            data.extend_from_slice(&v.to_le_bytes()[0..3]);
        }
    }

    let mut fmt = Vec::new();
    fmt.extend_from_slice(&1u16.to_le_bytes()); // PCM
    fmt.extend_from_slice(&channels.to_le_bytes());
    fmt.extend_from_slice(&rate.to_le_bytes());
    fmt.extend_from_slice(&(rate * u32::from(block_align)).to_le_bytes());
    fmt.extend_from_slice(&block_align.to_le_bytes());
    fmt.extend_from_slice(&(BYTES * 8).to_le_bytes());

    let mut chunks: Vec<(&[u8; 4], Vec<u8>)> = Vec::new();
    if let Some(tr) = time_reference {
        // A minimal version 1 bext chunk (602 bytes, no coding history).
        let mut bext = vec![0u8; 602];
        bext[..12].copy_from_slice(b"test fixture");
        bext[320..330].copy_from_slice(b"2026-11-16");
        bext[330..338].copy_from_slice(b"07:40:00");
        bext[338..346].copy_from_slice(&tr.to_le_bytes());
        bext[346..348].copy_from_slice(&1u16.to_le_bytes());
        // bext before fmt, so that the parser can't rely on fmt coming
        // first.
        chunks.push((b"bext", bext));
    }
    chunks.push((b"fmt ", fmt));
    chunks.push((b"data", data));

    let mut body = b"WAVE".to_vec();
    for (id, content) in chunks {
        body.extend_from_slice(id);
        body.extend_from_slice(&(content.len() as u32).to_le_bytes());
        body.extend_from_slice(&content);
        if content.len() % 2 == 1 {
            body.push(0);
        }
    }
    let mut file = b"RIFF".to_vec();
    file.extend_from_slice(&(body.len() as u32).to_le_bytes());
    file.extend_from_slice(&body);
    fs::write(path, file).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    const RATE: u32 = 48000;

    fn test_dir(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!(
            "h2n2flac-join-test-{}-{}",
            std::process::id(),
            name
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn info(time_reference: Option<u64>, frames: u64) -> WavInfo {
        WavInfo {
            channels: 2,
            sample_rate: RATE,
            block_align: 6,
            frames,
            time_reference,
        }
    }

    fn candidate(name: &str, time_reference: u64, frames: u64) -> Candidate {
        Candidate {
            path: PathBuf::from(name),
            info: info(Some(time_reference), frames),
        }
    }

    #[test]
    fn continuation_starts_exactly_where_the_previous_file_ends() {
        let a = info(Some(1000), 500);
        assert!(a.is_continued_by(&info(Some(1500), 10)));
        assert!(!a.is_continued_by(&info(Some(1499), 10)));
        assert!(!a.is_continued_by(&info(Some(1501), 10)));
        // A real gap, e.g. a new recording started a second later.
        assert!(!a.is_continued_by(&info(Some(1500 + u64::from(RATE)), 10)));
    }

    #[test]
    fn continuation_needs_time_references_and_equal_formats() {
        let a = info(Some(1000), 500);
        assert!(!a.is_continued_by(&info(None, 10)));
        assert!(!info(None, 500).is_continued_by(&info(Some(1500), 10)));
        assert!(!a.is_continued_by(&WavInfo {
            channels: 4,
            ..info(Some(1500), 10)
        }));
        assert!(!a.is_continued_by(&WavInfo {
            sample_rate: 44100,
            ..info(Some(1500), 10)
        }));
        assert!(!a.is_continued_by(&WavInfo {
            block_align: 4,
            ..info(Some(1500), 10)
        }));
        // Recorders without a clock write 0 everywhere: an empty file has no
        // end to be continued from, so such files never chain.
        assert!(!info(Some(0), 0).is_continued_by(&info(Some(0), 0)));
    }

    #[test]
    fn continuation_may_wrap_at_midnight() {
        let day = samples_per_day(RATE);
        let a = info(Some(day - 100), 500);
        assert!(a.is_continued_by(&info(Some(400), 10)));
        assert!(a.is_continued_by(&info(Some(day + 400), 10)));
        assert!(!a.is_continued_by(&info(Some(401), 10)));
    }

    #[test]
    fn chain_is_found_forwards_and_backwards_in_any_name_order() {
        let candidates = vec![
            candidate("SR001XY-0002.WAV", 3000, 100),
            candidate("SR001XY.WAV", 1000, 1000),
            candidate("SR001XY-0001.WAV", 2000, 1000),
            // Another recording later that day.
            candidate("SR002XY.WAV", 9000, 1000),
        ];
        let expected: Vec<PathBuf> = ["SR001XY.WAV", "SR001XY-0001.WAV", "SR001XY-0002.WAV"]
            .iter()
            .map(PathBuf::from)
            .collect();
        // Whichever file the user names, the whole recording is found.
        for start in &candidates[0..3] {
            assert_eq!(chain_from(start, &candidates), expected);
        }
        assert_eq!(
            chain_from(&candidates[3], &candidates),
            vec![PathBuf::from("SR002XY.WAV")]
        );
    }

    #[test]
    fn xy_and_ms_chains_are_kept_apart() {
        // In surround mode both pairs start at the same sample, so only the
        // name tells their continuations apart.
        let candidates = vec![
            candidate("SR001XY.WAV", 1000, 1000),
            candidate("SR001MS.WAV", 1000, 1000),
            candidate("SR002XY.WAV", 2000, 50),
            candidate("SR002MS.WAV", 2000, 50),
        ];
        assert_eq!(
            chain_from(&candidates[0], &candidates),
            vec![PathBuf::from("SR001XY.WAV"), PathBuf::from("SR002XY.WAV")]
        );
        assert_eq!(
            chain_from(&candidates[3], &candidates),
            vec![PathBuf::from("SR001MS.WAV"), PathBuf::from("SR002MS.WAV")]
        );
    }

    #[test]
    fn ambiguous_continuations_are_not_joined() {
        let candidates = vec![
            candidate("A.WAV", 1000, 1000),
            candidate("B.WAV", 2000, 10),
            candidate("C.WAV", 2000, 10),
        ];
        assert_eq!(
            chain_from(&candidates[0], &candidates),
            vec![PathBuf::from("A.WAV")]
        );
    }

    #[test]
    fn pair_is_recognised_from_the_file_name() {
        assert_eq!(pair_of(Path::new("dir/SR001XY.WAV")), Pair::Xy);
        assert_eq!(pair_of(Path::new("sr001ms.wav")), Pair::Ms);
        assert_eq!(pair_of(Path::new("STE-001.WAV")), Pair::Unknown);
    }

    #[test]
    fn header_of_a_broadcast_wav_file_is_parsed() {
        let dir = test_dir("parse");
        let path = dir.join("SR001XY.WAV");
        // An odd number of bytes in the data chunk tests the padding, too.
        write_bwf(&path, 1, RATE, 1001, Some(0x1_2345_6789), |_, _| 0.0);
        assert_eq!(
            read_wav_info(&path).unwrap(),
            WavInfo {
                channels: 1,
                sample_rate: RATE,
                block_align: 3,
                frames: 1001,
                time_reference: Some(0x1_2345_6789),
            }
        );

        let plain = dir.join("plain.wav");
        write_bwf(&plain, 2, RATE, 10, None, |_, _| 0.0);
        assert_eq!(read_wav_info(&plain).unwrap().time_reference, None);

        let junk = dir.join("junk.wav");
        fs::write(&junk, b"not a wav file at all").unwrap();
        assert!(read_wav_info(&junk).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_chain_scans_the_directory() {
        let dir = test_dir("scan");
        let silence = |_, _| 0.0;
        write_bwf(&dir.join("SR001XY.WAV"), 2, RATE, 300, Some(100), silence);
        write_bwf(&dir.join("SR002XY.WAV"), 2, RATE, 200, Some(400), silence);
        write_bwf(&dir.join("SR003XY.WAV"), 2, RATE, 200, Some(5000), silence);
        // Neither a WAV file nor readable as one: ignored.
        fs::write(dir.join("notes.txt"), b"hello").unwrap();
        fs::write(dir.join("broken.WAV"), b"RIFF").unwrap();

        assert_eq!(
            find_chain(&dir.join("SR002XY.WAV")),
            vec![dir.join("SR001XY.WAV"), dir.join("SR002XY.WAV")]
        );
        assert_eq!(
            find_chain(&dir.join("SR003XY.WAV")),
            vec![dir.join("SR003XY.WAV")]
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn plain_wav_files_are_never_joined() {
        let dir = test_dir("plain");
        write_bwf(&dir.join("a.wav"), 2, RATE, 300, None, |_, _| 0.0);
        write_bwf(&dir.join("b.wav"), 2, RATE, 300, None, |_, _| 0.0);
        assert_eq!(find_chain(&dir.join("a.wav")), vec![dir.join("a.wav")]);
        let _ = fs::remove_dir_all(&dir);
    }
}
