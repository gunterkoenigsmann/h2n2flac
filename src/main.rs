use getopts::Options;
use sndfile::*;
use std::env;
use std::fs;
use std::io::{SeekFrom, Write};

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
struct Recording {
    name_xy: String,
    name_ms: String,
    outfilename: String,
    xy_exists: bool,
    ms_exists: bool,
}

impl Recording {
    /// Derives the names of both stereo files and of the output file from
    /// the name of either stereo file.
    fn new(name: &str, format: OutputFormat) -> Recording {
        let filename_ms: String = name.replace("XY.WAV", "MS.WAV");
        let filename_xy: String = name.replace("MS.WAV", "XY.WAV");
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

        Recording {
            name_xy: filename_xy,
            name_ms: filename_ms,
            xy_exists: xy_present,
            ms_exists: ms_present,
            outfilename: outfile,
        }
    }
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

/// The largest absolute sample value in `file`, reading it from the
/// current position to the end. Advances `progress` by one unit per frame.
fn maxval<W: Write>(file: &mut SndFile, progress: &mut Progress<W>) -> f32 {
    let channels = file.get_channels();
    let mut buffer = vec![0.0f32; CHUNK_FRAMES * channels];
    let mut maxval: f32 = 0.0;

    loop {
        // read_to_slice() returns the number of *frames* read.
        let frames: usize = file
            .read_to_slice(&mut buffer)
            .unwrap_or_else(|e| panic!("Error reading file: {:?}", e));
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
    let mut msfile = open_read(&file.name_ms);
    let mut xyfile = open_read(&file.name_xy);
    if msfile.get_channels() != 2 || xyfile.get_channels() != 2 {
        panic!("MS and XY file are expected to be stereo files!");
    }
    if msfile.get_samplerate() != xyfile.get_samplerate() {
        panic!("MS and XY file differ in sample rate!");
    }
    let frames = frame_count(&mut msfile);
    if frames != frame_count(&mut xyfile) {
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
        msfile
            .seek(SeekFrom::Start(0))
            .expect("Cannot rewind the MS file");
        xyfile
            .seek(SeekFrom::Start(0))
            .expect("Cannot rewind the XY file");
    }

    let mut outfile = OpenOptions::WriteOnly(WriteOptions::new(
        format.major_format(),
        format.subtype_format(),
        Endian::File,
        msfile.get_samplerate(),
        4,
    ))
    .from_path(&file.outfilename)
    .expect("Failed to open file for writing");

    let mut buffer_ms = vec![0.0f32; CHUNK_FRAMES * 2];
    let mut buffer_xy = vec![0.0f32; CHUNK_FRAMES * 2];
    let mut buffer_output = vec![0.0f32; CHUNK_FRAMES * 4];

    loop {
        let frames_ms: usize = msfile
            .read_to_slice(&mut buffer_ms)
            .unwrap_or_else(|e| panic!("Error reading the MS file: {:?}", e));
        let frames_xy: usize = xyfile
            .read_to_slice(&mut buffer_xy)
            .unwrap_or_else(|e| panic!("Error reading the XY file: {:?}", e));
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

/// Converts a single file (usually a stereo file) to `format`, keeping its
/// channel count.
fn h2n2flac_2ch<W: Write>(
    normalize: bool,
    infilename: &str,
    outfilename: &str,
    format: OutputFormat,
    progress: &mut Progress<W>,
) {
    let mut infile = open_read(infilename);
    let channels = infile.get_channels();
    let frames = frame_count(&mut infile);
    // Normalizing reads the file once more before converting it.
    progress.set_total(if normalize { 2 * frames } else { frames });

    let mut scale_factor: f32 = 1.0;
    if normalize {
        scale_factor = normalize_factor(maxval(&mut infile, progress));
        infile
            .seek(SeekFrom::Start(0))
            .expect("Cannot rewind the input file");
    }

    let mut outfile = OpenOptions::WriteOnly(WriteOptions::new(
        format.major_format(),
        format.subtype_format(),
        Endian::File,
        infile.get_samplerate(),
        channels,
    ))
    .from_path(outfilename)
    .expect("Failed to open file for writing");

    let mut buffer = vec![0.0f32; CHUNK_FRAMES * channels];

    loop {
        let frames: usize = infile
            .read_to_slice(&mut buffer)
            .unwrap_or_else(|e| panic!("Error reading file: {:?}", e));
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
    match (file.xy_exists, file.ms_exists) {
        (true, true) => h2n2flac_4ch(normalize, file, format, progress),
        (true, false) => h2n2flac_2ch(
            normalize,
            &file.name_xy,
            &file.outfilename,
            format,
            progress,
        ),
        (false, true) => h2n2flac_2ch(
            normalize,
            &file.name_ms,
            &file.outfilename,
            format,
            progress,
        ),
        // Recording::new() refuses recordings without any file.
        (false, false) => unreachable!(),
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

    for file in &matches.free {
        // The bar appears only if stderr is a terminal.
        let mut progress = Progress::on_stderr(file);
        h2n2flac(
            normalize,
            &Recording::new(file, format),
            format,
            &mut progress,
        );
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

        let rec = Recording::new(&name, OutputFormat::Ogg);
        assert!(rec.ms_exists);
        assert!(!rec.xy_exists);
        assert_eq!(rec.name_ms, ms.to_string_lossy());
        assert_eq!(rec.outfilename, dir.join("SR001.ogg").to_string_lossy());

        let rec = Recording::new(&name, OutputFormat::Flac);
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
        let mut file = open_read(path.to_str().unwrap());
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
        let rec = Recording::new(input.to_str().unwrap(), OutputFormat::Flac);

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
        let rec = Recording::new(input.to_str().unwrap(), OutputFormat::Flac);

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
        let rec = Recording::new(xy.to_str().unwrap(), OutputFormat::Flac);

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
        let rec = Recording::new(ms.to_str().unwrap(), OutputFormat::Flac);

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
        let rec = Recording::new(input.to_str().unwrap(), OutputFormat::Flac);
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
}
