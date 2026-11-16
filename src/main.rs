use getopts::Options;
use sndfile::*;
use std::collections::HashSet;
use std::env;
use std::fs;
use std::io::{SeekFrom, Write};
use std::path::{Path, PathBuf};

mod join;
mod progress;
use progress::Progress;

/// Number of frames (one sample per channel) processed per read/write call.
const CHUNK_FRAMES: usize = 8192;

#[derive(Debug, Clone, Copy, PartialEq)]
enum OutputFormat {
    Ogg,
    Flac,
}

impl OutputFormat {
    /// The format is chosen by the name the program was called by:
    /// `h2n2flac` writes FLAC, anything else (`h2n2ogg`) Ogg Vorbis.
    fn from_program_name(program: &str) -> OutputFormat {
        if program.ends_with("flac") {
            OutputFormat::Flac
        } else {
            OutputFormat::Ogg
        }
    }

    fn major_format(self) -> MajorFormat {
        match self {
            OutputFormat::Ogg => MajorFormat::OGG,
            OutputFormat::Flac => MajorFormat::FLAC,
        }
    }

    fn subtype_format(self) -> SubtypeFormat {
        match self {
            OutputFormat::Ogg => SubtypeFormat::VORBIS,
            OutputFormat::Flac => SubtypeFormat::PCM_24,
        }
    }

    fn extension(self) -> &'static str {
        match self {
            OutputFormat::Ogg => ".ogg",
            OutputFormat::Flac => ".flac",
        }
    }
}

/// One recording of the H2n: the front-facing XY and/or the back-facing MS
/// stereo file (`SR001XY.WAV`, `SR001MS.WAV`) and the name of the output
/// file (`SR001.flac`).
///
/// Long recordings are split by the recorder into several files per
/// microphone pair (see [`join`]); `xy` and `ms` list all of them in
/// recording order. A pair that wasn't recorded has no files.
struct Recording {
    xy: Vec<String>,
    ms: Vec<String>,
    outfilename: String,
}

impl Recording {
    /// Derives the names of both stereo files and of the output file from
    /// the name of either stereo file. `name` may be any part of a split
    /// recording; with `join`, all parts are found.
    fn new(name: &str, format: OutputFormat, join: bool) -> Recording {
        let chain_of = |path: &str| -> Vec<String> {
            if join {
                join::find_chain(Path::new(path))
                    .iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect()
            } else {
                vec![path.to_string()]
            }
        };
        // Only the first part of a split recording carries the H2n's name
        // (the continuation files might be named differently), so the other
        // pair and the output file are named after the first part.
        let given = chain_of(name);
        let first = &given[0];
        let filename_ms: String = first.replace("XY.WAV", "MS.WAV");
        let filename_xy: String = first.replace("MS.WAV", "XY.WAV");
        if filename_ms == filename_xy {
            panic!("No H2n filename found!");
        }

        let xy_present: bool = match fs::exists(&filename_xy) {
            Ok(x) => x,
            Err(e) => panic!("Cannot access the XY file: {e}"),
        };
        let ms_present: bool = match fs::exists(&filename_ms) {
            Ok(x) => x,
            Err(e) => panic!("Cannot access the MS file: {e}"),
        };

        if !(xy_present || ms_present) {
            panic!("No audio files found for {}.", name);
        }
        let outfile: String = filename_ms.replace("MS.WAV", format.extension());

        let parts = |present: bool, filename: &String| -> Vec<String> {
            match (present, filename == first) {
                (false, _) => Vec::new(),
                (true, true) => given.clone(),
                (true, false) => chain_of(filename),
            }
        };
        Recording {
            xy: parts(xy_present, &filename_xy),
            ms: parts(ms_present, &filename_ms),
            outfilename: outfile,
        }
    }

    /// All input files, XY first.
    fn parts(&self) -> impl Iterator<Item = &String> {
        self.xy.iter().chain(&self.ms)
    }
}

/// A name that is the same for every spelling of the path of a file
/// (`a.wav`, `./a.wav`, `/home/me/a.wav`), if the file exists.
fn identity(path: &str) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path))
}

/// The recordings to convert for the files given on the command line.
///
/// A file that is already part of an earlier recording isn't converted a
/// second time: `h2n2flac *.WAV` names every part of a split recording and
/// both the XY and the MS file of a surround recording, but each recording
/// is meant to be converted once.
fn plan(files: &[String], format: OutputFormat, join: bool) -> Vec<Recording> {
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut recordings = Vec::new();
    for file in files {
        if seen.contains(&identity(file)) {
            continue;
        }
        let recording = Recording::new(file, format, join);
        seen.extend(recording.parts().map(|p| identity(p)));
        recordings.push(recording);
    }
    recordings
}

fn open_read(path: &str) -> SndFile {
    OpenOptions::ReadOnly(ReadOptions::Auto)
        .from_path(path)
        .unwrap_or_else(|_| panic!("Cannot open {path} for reading"))
}

/// The number of frames in `file`. `SndFile::len()` leaves the read cursor
/// at the end of the file, so this rewinds it to the start afterwards.
fn frame_count(file: &mut SndFile) -> u64 {
    let frames = file.len().expect("Cannot determine the length of a file");
    file.seek(SeekFrom::Start(0)).expect("Cannot rewind a file");
    frames
}

/// Reads the parts of a split recording one after another, as if they were
/// one file. A read that reaches the end of a part continues with the next
/// one, so the joined audio has no gaps, and the buffer is always filled
/// completely, except at the very end.
struct Source {
    paths: Vec<String>,
    /// The index of the part `current` reads.
    index: usize,
    current: SndFile,
    channels: usize,
    samplerate: usize,
    /// The number of frames of all parts together.
    frames: u64,
}

impl Source {
    /// Opens the parts `paths` (at least one), which must all have the same
    /// channel count and sample rate.
    fn open(paths: &[String]) -> Source {
        let mut first = open_read(&paths[0]);
        let channels = first.get_channels();
        let samplerate = first.get_samplerate();
        let mut frames = frame_count(&mut first);
        for path in &paths[1..] {
            let mut part = open_read(path);
            if part.get_channels() != channels || part.get_samplerate() != samplerate {
                panic!("{path} differs in format from {}!", paths[0]);
            }
            frames += frame_count(&mut part);
        }
        Source {
            paths: paths.to_vec(),
            index: 0,
            current: first,
            channels,
            samplerate,
            frames,
        }
    }

    /// Starts reading from the beginning of the first part again.
    fn rewind(&mut self) {
        if self.index != 0 {
            self.index = 0;
            self.current = open_read(&self.paths[0]);
        } else {
            self.current
                .seek(SeekFrom::Start(0))
                .unwrap_or_else(|_| panic!("Cannot rewind {}", self.paths[0]));
        }
    }

    /// Reads as many whole frames as fit into `buffer` and returns their
    /// number; fewer only at the end of the last part.
    fn read(&mut self, buffer: &mut [f32]) -> usize {
        let capacity = buffer.len() / self.channels;
        let mut filled = 0;
        while filled < capacity {
            // read_to_slice() returns the number of *frames* read.
            let frames = self
                .current
                .read_to_slice(&mut buffer[filled * self.channels..capacity * self.channels])
                .unwrap_or_else(|e| panic!("Error reading {}: {:?}", self.paths[self.index], e));
            filled += frames;
            if frames == 0 {
                if self.index + 1 == self.paths.len() {
                    break;
                }
                self.index += 1;
                self.current = open_read(&self.paths[self.index]);
            }
        }
        filled
    }
}

/// The largest absolute sample value in `file`, reading it from the
/// current position to the end. Advances `progress` by one unit per frame.
fn maxval<W: Write>(file: &mut Source, progress: &mut Progress<W>) -> f32 {
    let channels = file.channels;
    let mut buffer = vec![0.0f32; CHUNK_FRAMES * channels];
    let mut maxval: f32 = 0.0;

    loop {
        let frames: usize = file.read(&mut buffer);
        for sample in &buffer[..frames * channels] {
            maxval = maxval.max(sample.abs());
        }
        progress.advance(frames as u64);
        if frames < CHUNK_FRAMES {
            break;
        }
    }
    maxval
}

/// The factor that scales a signal with the peak `maxval` to full scale.
/// Silence is left untouched instead of being divided by zero.
fn normalize_factor(maxval: f32) -> f32 {
    if maxval > 0.0 { 1.0 / maxval } else { 1.0 }
}

/// Combines an MS and an XY stereo file to one 4-channel file: channels 1
/// and 2 are MS, channels 3 and 4 are XY.
fn h2n2flac_4ch<W: Write>(
    normalize: bool,
    file: &Recording,
    format: OutputFormat,
    progress: &mut Progress<W>,
) {
    let mut msfile = Source::open(&file.ms);
    let mut xyfile = Source::open(&file.xy);
    if msfile.channels != 2 || xyfile.channels != 2 {
        panic!("MS and XY file are expected to be stereo files!");
    }
    if msfile.samplerate != xyfile.samplerate {
        panic!("MS and XY file differ in sample rate!");
    }
    let frames = msfile.frames;
    if frames != xyfile.frames {
        panic!("MS and XY file differ in size!");
    }
    // Normalizing reads both files once more before converting them.
    progress.set_total(if normalize { 3 * frames } else { frames });

    // Both files get the same factor, so that their relative levels stay
    // as recorded; the louder one determines it.
    let mut scale_factor: f32 = 1.0;
    if normalize {
        let peak = maxval(&mut msfile, progress).max(maxval(&mut xyfile, progress));
        scale_factor = normalize_factor(peak);
        msfile.rewind();
        xyfile.rewind();
    }

    let mut outfile = OpenOptions::WriteOnly(WriteOptions::new(
        format.major_format(),
        format.subtype_format(),
        Endian::File,
        msfile.samplerate,
        4,
    ))
    .from_path(&file.outfilename)
    .expect("Failed to open file for writing");

    let mut buffer_ms = vec![0.0f32; CHUNK_FRAMES * 2];
    let mut buffer_xy = vec![0.0f32; CHUNK_FRAMES * 2];
    let mut buffer_output = vec![0.0f32; CHUNK_FRAMES * 4];

    loop {
        // The parts of MS and XY may end at different frames; Source hides
        // that, so the two always stay in step.
        let frames_ms: usize = msfile.read(&mut buffer_ms);
        let frames_xy: usize = xyfile.read(&mut buffer_xy);
        if frames_ms != frames_xy {
            panic!("MS and XY files differ in length!");
        }
        for i in 0..frames_ms {
            buffer_output[i * 4] = buffer_ms[i * 2] * scale_factor;
            buffer_output[i * 4 + 1] = buffer_ms[i * 2 + 1] * scale_factor;
            buffer_output[i * 4 + 2] = buffer_xy[i * 2] * scale_factor;
            buffer_output[i * 4 + 3] = buffer_xy[i * 2 + 1] * scale_factor;
        }
        // The last chunk is usually shorter than the buffer.
        outfile
            .write_from_slice(&buffer_output[..frames_ms * 4])
            .expect("Failed to write audio data");
        progress.advance(frames_ms as u64);
        if frames_ms < CHUNK_FRAMES {
            break;
        }
    }
}

/// Converts a single file (usually a stereo file), or the parts of one, to
/// `format`, keeping its channel count.
fn h2n2flac_2ch<W: Write>(
    normalize: bool,
    infilenames: &[String],
    outfilename: &str,
    format: OutputFormat,
    progress: &mut Progress<W>,
) {
    let mut infile = Source::open(infilenames);
    let channels = infile.channels;
    let frames = infile.frames;
    // Normalizing reads the file once more before converting it.
    progress.set_total(if normalize { 2 * frames } else { frames });

    let mut scale_factor: f32 = 1.0;
    if normalize {
        scale_factor = normalize_factor(maxval(&mut infile, progress));
        infile.rewind();
    }

    let mut outfile = OpenOptions::WriteOnly(WriteOptions::new(
        format.major_format(),
        format.subtype_format(),
        Endian::File,
        infile.samplerate,
        channels,
    ))
    .from_path(outfilename)
    .expect("Failed to open file for writing");

    let mut buffer = vec![0.0f32; CHUNK_FRAMES * channels];

    loop {
        let frames: usize = infile.read(&mut buffer);
        let samples = &mut buffer[..frames * channels];
        for sample in samples.iter_mut() {
            *sample *= scale_factor;
        }
        outfile
            .write_from_slice(samples)
            .expect("Failed to write audio data");
        progress.advance(frames as u64);
        if frames < CHUNK_FRAMES {
            break;
        }
    }
}

/// Converts one recording: a 4-channel file if both stereo files exist,
/// otherwise the one that exists.
fn h2n2flac<W: Write>(
    normalize: bool,
    file: &Recording,
    format: OutputFormat,
    progress: &mut Progress<W>,
) {
    match (file.xy.is_empty(), file.ms.is_empty()) {
        (false, false) => h2n2flac_4ch(normalize, file, format, progress),
        (false, true) => h2n2flac_2ch(normalize, &file.xy, &file.outfilename, format, progress),
        (true, false) => h2n2flac_2ch(normalize, &file.ms, &file.outfilename, format, progress),
        // Recording::new() refuses recordings without any file.
        (true, true) => unreachable!(),
    }
    progress.finish();
}

fn print_usage(program: &str, opts: Options) {
    let brief = format!("Usage: {} FILE(s) [options]", program);
    print!("{}", opts.usage(&brief));
}

fn print_version(program: &str) {
    println!("{} version {}", program, env!("CARGO_PKG_VERSION"));
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let program = args[0].clone();

    let mut opts = Options::new();
    opts.optflag("h", "help", "print this help menu");
    opts.optflag("v", "version", "print the version info");
    opts.optflag("n", "normalize", "Normalize the output");
    opts.optflag(
        "",
        "no-join",
        "Don't join recordings the recorder split into several files",
    );
    //    opts.optflagopt("c", "clip", "Allow clipping of n permille of the samples", "CLIP_PERCENTAGE");
    let matches = match opts.parse(&args[1..]) {
        Ok(m) => m,
        Err(f) => {
            panic!("{}", f.to_string());
        }
    };
    if matches.opt_present("h") {
        print_usage(&program, opts);
        return;
    }
    if matches.opt_present("v") {
        print_version(&program);
        return;
    }
    let normalize = matches.opt_present("n");
    // if matches.opt_present("c") {
    //     clip = match matches.opt_str("c") {
    //         Some(s) => match s.parse::<f32>() {
    //             Ok(n) => n,
    //             Err(e) => panic!("The argument to c must be an integer. Got {e}"),
    //         }
    //         None => 1,
    //     };
    // }

    if matches.free.is_empty() {
        print_usage(&program, opts);
        return;
    }

    let format = OutputFormat::from_program_name(&program);

    let join = !matches.opt_present("no-join");

    for recording in plan(&matches.free, format, join) {
        for parts in [&recording.xy, &recording.ms] {
            if parts.len() > 1 {
                eprintln!("Joining {}", parts.join(" + "));
            }
        }
        // The bar appears only if stderr is a terminal.
        let mut progress = Progress::on_stderr(&recording.outfilename);
        h2n2flac(normalize, &recording, format, &mut progress);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const RATE: usize = 48000;

    /// A fresh directory for one test. The process id keeps the two test
    /// binaries (h2n2flac and h2n2ogg share this file) apart when cargo
    /// runs them in parallel.
    fn test_dir(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("h2n2flac-test-{}-{}", std::process::id(), name));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Writes a 32-bit float WAV file whose sample for (frame, channel) is
    /// `sample(frame, channel)`.
    fn write_wav(
        path: &PathBuf,
        channels: usize,
        frames: usize,
        sample: impl Fn(usize, usize) -> f32,
    ) {
        let mut data = Vec::with_capacity(frames * channels);
        for f in 0..frames {
            for c in 0..channels {
                data.push(sample(f, c));
            }
        }
        let mut file = OpenOptions::WriteOnly(WriteOptions::new(
            MajorFormat::WAV,
            SubtypeFormat::FLOAT,
            Endian::File,
            RATE,
            channels,
        ))
        .from_path(path)
        .unwrap();
        file.write_from_slice(&data).unwrap();
    }

    /// Reads a whole file: (channels, interleaved samples).
    fn read_all(path: &str) -> (usize, Vec<f32>) {
        let mut file = open_read(path);
        let data: Vec<f32> = file.read_all_to_vec().unwrap();
        (file.get_channels(), data)
    }

    /// A test signal that differs per frame and channel and stays well
    /// below full scale (|x| <= 0.5).
    fn signal(frame: usize, channel: usize) -> f32 {
        let x = ((frame * 7 + channel * 13) % 101) as f32 / 100.0 - 0.5;
        if channel.is_multiple_of(2) { x } else { -x }
    }

    /// A progress bar that draws nothing, but still counts.
    fn quiet() -> Progress<Vec<u8>> {
        Progress::new("test", None)
    }

    /// Converted (24-bit) samples differ from the input by rounding only.
    fn assert_close(actual: f32, expected: f32, what: &str) {
        assert!(
            (actual - expected).abs() < 1e-5,
            "{what}: {actual} != {expected}"
        );
    }

    // Not a multiple of CHUNK_FRAMES, and longer than two chunks, so that
    // full chunks and a short last chunk are both exercised.
    const FRAMES: usize = 2 * CHUNK_FRAMES + 1234;

    #[test]
    fn format_follows_program_name() {
        assert_eq!(
            OutputFormat::from_program_name("/usr/bin/h2n2flac"),
            OutputFormat::Flac
        );
        assert_eq!(
            OutputFormat::from_program_name("h2n2ogg"),
            OutputFormat::Ogg
        );
    }

    #[test]
    fn recording_finds_files_and_names_output_by_format() {
        let dir = test_dir("names");
        let ms = dir.join("SR001MS.WAV");
        write_wav(&ms, 2, 10, signal);
        let name = dir.join("SR001XY.WAV").to_string_lossy().into_owned();

        let rec = Recording::new(&name, OutputFormat::Ogg, true);
        assert!(rec.xy.is_empty());
        assert_eq!(rec.ms, vec![ms.to_string_lossy().into_owned()]);
        assert_eq!(rec.outfilename, dir.join("SR001.ogg").to_string_lossy());

        let rec = Recording::new(&name, OutputFormat::Flac, true);
        assert_eq!(rec.outfilename, dir.join("SR001.flac").to_string_lossy());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn frame_count_rewinds_the_file() {
        let dir = test_dir("rewind");
        let path = dir.join("a.wav");
        write_wav(&path, 2, FRAMES, signal);
        let mut file = open_read(path.to_str().unwrap());
        assert_eq!(frame_count(&mut file), FRAMES as u64);
        // Reading right after it must start at the first frame.
        let mut buffer = vec![0.0f32; 2];
        assert_eq!(file.read_to_slice(&mut buffer).unwrap(), 1);
        assert_close(buffer[0], signal(0, 0), "first sample");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn maxval_scans_every_sample() {
        let dir = test_dir("maxval");
        let path = dir.join("a.wav");
        // The peak sits in the right channel of the very last frame.
        write_wav(&path, 2, FRAMES, |f, c| {
            if f == FRAMES - 1 && c == 1 {
                -0.75
            } else {
                0.25
            }
        });
        let mut file = Source::open(&[path.to_string_lossy().into_owned()]);
        let mut progress = quiet();
        progress.set_total(FRAMES as u64);
        assert_close(maxval(&mut file, &mut progress), 0.75, "peak");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn normalize_factor_leaves_silence_alone() {
        assert_eq!(normalize_factor(0.0), 1.0);
        assert_eq!(normalize_factor(0.5), 2.0);
    }

    #[test]
    fn stereo_file_is_converted_completely() {
        let dir = test_dir("stereo");
        let input = dir.join("SR002XY.WAV");
        write_wav(&input, 2, FRAMES, signal);
        let rec = Recording::new(input.to_str().unwrap(), OutputFormat::Flac, true);

        h2n2flac(false, &rec, OutputFormat::Flac, &mut quiet());

        let (channels, data) = read_all(&rec.outfilename);
        assert_eq!(channels, 2);
        assert_eq!(data.len(), FRAMES * 2);
        for f in [0, 1, CHUNK_FRAMES - 1, CHUNK_FRAMES, FRAMES - 1] {
            for c in 0..2 {
                assert_close(
                    data[f * 2 + c],
                    signal(f, c),
                    &format!("frame {f} channel {c}"),
                );
            }
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn stereo_file_is_normalized_to_full_scale() {
        let dir = test_dir("stereo-normalized");
        let input = dir.join("SR003MS.WAV");
        write_wav(&input, 2, FRAMES, signal);
        let rec = Recording::new(input.to_str().unwrap(), OutputFormat::Flac, true);

        h2n2flac(true, &rec, OutputFormat::Flac, &mut quiet());

        let (_, data) = read_all(&rec.outfilename);
        assert_eq!(data.len(), FRAMES * 2);
        // signal() peaks at 0.5, so everything is doubled.
        assert_close(data[5], signal(2, 1) * 2.0, "scaled sample");
        let peak = data.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak > 0.999 && peak <= 1.0, "peak {peak}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ms_and_xy_are_combined_to_four_channels() {
        let dir = test_dir("surround");
        let ms = dir.join("SR004MS.WAV");
        let xy = dir.join("SR004XY.WAV");
        // Different signals per file, so that a mix-up shows.
        write_wav(&ms, 2, FRAMES, |f, c| signal(f, c) * 0.5);
        write_wav(&xy, 2, FRAMES, |f, c| signal(f + 3, c) * 0.25);
        let rec = Recording::new(xy.to_str().unwrap(), OutputFormat::Flac, true);

        h2n2flac(false, &rec, OutputFormat::Flac, &mut quiet());

        let (channels, data) = read_all(&rec.outfilename);
        assert_eq!(channels, 4);
        assert_eq!(data.len(), FRAMES * 4);
        for f in [0, 1, CHUNK_FRAMES - 1, CHUNK_FRAMES, FRAMES - 1] {
            let what = format!("frame {f}");
            assert_close(data[f * 4], signal(f, 0) * 0.5, &what);
            assert_close(data[f * 4 + 1], signal(f, 1) * 0.5, &what);
            assert_close(data[f * 4 + 2], signal(f + 3, 0) * 0.25, &what);
            assert_close(data[f * 4 + 3], signal(f + 3, 1) * 0.25, &what);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn four_channel_normalization_uses_the_louder_file() {
        let dir = test_dir("surround-normalized");
        let ms = dir.join("SR005MS.WAV");
        let xy = dir.join("SR005XY.WAV");
        write_wav(&ms, 2, FRAMES, |_, _| 0.5);
        write_wav(&xy, 2, FRAMES, |_, _| 0.25);
        let rec = Recording::new(ms.to_str().unwrap(), OutputFormat::Flac, true);

        h2n2flac(true, &rec, OutputFormat::Flac, &mut quiet());

        let (_, data) = read_all(&rec.outfilename);
        assert_eq!(data.len(), FRAMES * 4);
        // MS (the louder one) reaches full scale, XY keeps its relative level;
        // nothing clips.
        assert_close(data[0], 0.5 * 2.0, "MS");
        assert_close(data[2], 0.25 * 2.0, "XY");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn conversion_reports_progress_up_to_100_percent() {
        let dir = test_dir("progress");
        let input = dir.join("SR006XY.WAV");
        write_wav(&input, 2, FRAMES, signal);
        let rec = Recording::new(input.to_str().unwrap(), OutputFormat::Flac, true);
        let mut progress = Progress::new("p", Some(Vec::new()));

        h2n2flac(true, &rec, OutputFormat::Flac, &mut progress);

        let text = String::from_utf8(progress.output().unwrap().clone()).unwrap();
        // Scan and conversion are drawn as one bar that ends at 100%.
        assert!(
            text.ends_with(&format!("{}\n", progress::render("p", 100))),
            "{text:?}"
        );
        assert!(text.contains(&progress::render("p", 50)), "{text:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// A path as the String the program gets on its command line.
    fn arg(path: &Path) -> String {
        path.to_string_lossy().into_owned()
    }

    /// Writes `signal` (scaled by `gain`, shifted by `offset` frames) split
    /// into BWF files of `lengths` frames each that start at time reference
    /// `start` and continue each other seamlessly.
    fn write_split(
        dir: &Path,
        names: &[&str],
        lengths: &[usize],
        start: u64,
        gain: f32,
        offset: usize,
    ) {
        let mut first_frame = 0;
        for (name, &frames) in names.iter().zip(lengths) {
            join::write_bwf(
                &dir.join(name),
                2,
                RATE as u32,
                frames,
                Some(start + first_frame as u64),
                |f, c| signal(first_frame + f + offset, c) * gain,
            );
            first_frame += frames;
        }
    }

    // Part lengths that end inside a CHUNK_FRAMES block, so that reads
    // have to continue across file boundaries.
    const PARTS: [usize; 3] = [CHUNK_FRAMES + 100, 777, CHUNK_FRAMES + 5];

    #[test]
    fn split_stereo_recording_is_joined_seamlessly() {
        let dir = test_dir("split-stereo");
        // The continuation files are deliberately named unlike the first
        // one: only the time reference connects them.
        write_split(
            &dir,
            &["SR010XY.WAV", "SR011XY.WAV", "ZZZ-XY.WAV"],
            &PARTS,
            4800,
            1.0,
            0,
        );
        // An unrelated recording that starts later must stay out.
        join::write_bwf(
            &dir.join("SR012XY.WAV"),
            2,
            RATE as u32,
            10,
            Some(10_000_000),
            signal,
        );
        let total: usize = PARTS.iter().sum();

        // Naming a middle part converts the whole recording.
        let rec = Recording::new(&arg(&dir.join("SR011XY.WAV")), OutputFormat::Flac, true);
        assert_eq!(rec.outfilename, arg(&dir.join("SR010.flac")));
        assert_eq!(rec.xy.len(), 3);
        h2n2flac(false, &rec, OutputFormat::Flac, &mut quiet());

        let (channels, data) = read_all(&rec.outfilename);
        assert_eq!(channels, 2);
        assert_eq!(data.len(), total * 2);
        // Every frame, so that a lost or doubled frame at a seam shows.
        for f in 0..total {
            for c in 0..2 {
                assert_close(
                    data[f * 2 + c],
                    signal(f, c),
                    &format!("frame {f} channel {c}"),
                );
            }
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn split_surround_recording_is_joined_and_normalized_as_a_whole() {
        let dir = test_dir("split-surround");
        let total: usize = PARTS.iter().sum();
        // MS and XY are split at different frames, which mustn't put them
        // out of step.
        let xy_parts = [CHUNK_FRAMES * 2, total - CHUNK_FRAMES * 2];
        write_split(
            &dir,
            &["SR020MS.WAV", "SR021MS.WAV", "SR022MS.WAV"],
            &PARTS,
            0,
            0.5,
            0,
        );
        write_split(&dir, &["SR020XY.WAV", "SR021XY.WAV"], &xy_parts, 0, 0.25, 3);

        let rec = Recording::new(&arg(&dir.join("SR020XY.WAV")), OutputFormat::Flac, true);
        assert_eq!(rec.ms.len(), 3);
        assert_eq!(rec.xy.len(), 2);
        h2n2flac(true, &rec, OutputFormat::Flac, &mut quiet());

        let (channels, data) = read_all(&rec.outfilename);
        assert_eq!(channels, 4);
        assert_eq!(data.len(), total * 4);
        // The peak (0.25, in the MS file) is scaled to full scale: the
        // peak of all parts counts, not that of the first one.
        for f in 0..total {
            let what = format!("frame {f}");
            assert_close(data[f * 4], signal(f, 0) * 0.5 * 4.0, &what);
            assert_close(data[f * 4 + 1], signal(f, 1) * 0.5 * 4.0, &what);
            assert_close(data[f * 4 + 2], signal(f + 3, 0) * 0.25 * 4.0, &what);
            assert_close(data[f * 4 + 3], signal(f + 3, 1) * 0.25 * 4.0, &what);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_join_converts_only_the_named_part() {
        let dir = test_dir("no-join");
        write_split(
            &dir,
            &["SR030XY.WAV", "SR031XY.WAV"],
            &PARTS[..2],
            0,
            1.0,
            0,
        );
        let rec = Recording::new(&arg(&dir.join("SR030XY.WAV")), OutputFormat::Flac, false);
        assert_eq!(rec.xy, vec![arg(&dir.join("SR030XY.WAV"))]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn plan_converts_every_recording_once() {
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

        let recordings = plan(&files, OutputFormat::Flac, true);
        let outputs: Vec<&str> = recordings.iter().map(|r| r.outfilename.as_str()).collect();
        assert_eq!(
            outputs,
            vec![arg(&dir.join("SR040.flac")), arg(&dir.join("SR050.flac"))]
        );
        assert_eq!(recordings[0].xy.len(), 2);
        assert_eq!(recordings[0].ms.len(), 2);

        // Without joining, only the second file of each surround pair is
        // left out.
        assert_eq!(plan(&files, OutputFormat::Flac, false).len(), 3);
        let _ = fs::remove_dir_all(&dir);
    }
}
