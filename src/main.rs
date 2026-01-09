extern crate getopts;
extern crate sndfile;
extern crate ndarray;
use getopts::Options;
use std::env;
use std::fs;
use sndfile::*;

enum OutputFormat {
    Ogg,
    Flac,
}

struct Recording {
    name_xy: String,
    name_ms: String,
    outfilename: String,
    xy_exists: bool,
    ms_exists: bool,
}

impl Recording {
    pub fn name_xy(&self) -> &String { &self.name_xy }
    pub fn name_ms(&self) -> &String { &self.name_ms }
    pub fn file_xy(&self) -> SndFile {
        return sndfile::OpenOptions::ReadOnly(ReadOptions::Auto).from_path(
            self.name_xy()).unwrap(); }
    pub fn file_ms(&self) -> SndFile {
        return sndfile::OpenOptions::ReadOnly(ReadOptions::Auto).from_path(
            self.name_ms()).unwrap(); }
    pub fn outfilename(&self) -> &String { &self.outfilename }
    pub fn xy_exists(&self) -> &bool { &self.xy_exists }
    pub fn ms_exists(&self) -> &bool { &self.ms_exists }
    pub fn major_format(format: &OutputFormat) -> sndfile::MajorFormat
    {
        return match format{            
            OutputFormat::Ogg  => {sndfile::MajorFormat::OGG}
            OutputFormat::Flac => {sndfile::MajorFormat::FLAC}
        }
    }
    pub fn subtype_format(format: &OutputFormat) -> sndfile::SubtypeFormat
    {
        return match format{            
            OutputFormat::Ogg  => {sndfile::SubtypeFormat::VORBIS}
            OutputFormat::Flac => {sndfile::SubtypeFormat::PCM_24}
        }
    }
    fn new(name: &str) -> Recording {
        let filename_ms: String = name.replace("XY.WAV", "MS.WAV");
        let filename_xy: String = name.replace("MS.WAV", "XY.WAV");
        if filename_ms == filename_xy { panic!("No H2n filename found!");}


        let xy_present: bool = match fs::exists(&filename_xy) {
            Ok(x) => {x},
            Err(e) => panic!("Cannot access the XY file: {e}")
        };
        let ms_present: bool = match fs::exists(&filename_ms) {
            Ok(x) => {x},
            Err(e) => panic!("Cannot access the MS file: {e}")
        };

        if !(xy_present || ms_present)
        {
            panic!("No audio files found for {}.", name);
        }
        let outfile: String = filename_ms.replace("MS.WAV", ".flac");
        
        Recording{
            name_xy: filename_xy,
            name_ms: filename_ms,
            xy_exists: xy_present,
            ms_exists: ms_present,
            outfilename: outfile
        }
    }
    
    pub fn maxval(mut file: SndFile) -> f32 {
        let mut maxval: f32 = 0.0;

        let channels = file.get_channels();
        if channels != 2
        {
            panic!("Channel count of the file is unexpected.");
        }
        let num_frames = 8192;
        let mut buffer = vec![0.0f32; num_frames * channels];

        loop {
            match file.read_to_slice(&mut buffer) {
                Ok(samples_read) => {
                    for i in 0 .. samples_read
                    {
                        let absval = buffer[i].abs();
                        if absval > maxval
                        {
                            maxval = absval;
                        }
                    }
                    if samples_read < 8192
                    {
                        break;
                    }
                }
                Err(e) => { panic!("Error reading file: {:?})", e); }
            }
        }
        return maxval;
    }
}

fn h2n2flac_4ch(normalize: bool,
                file: &Recording,
                format: &OutputFormat
)
{    
    // Determine how strong to scale the audio data for normalizing the file
    let mut scale_factor: f32 = 1.0;
    if normalize
    {
        let maxval_xy: f32 = Recording::maxval(file.file_xy());
        let maxval_ms: f32 = Recording::maxval(file.file_ms());
        if maxval_xy > maxval_ms
        {
            scale_factor = 1.0/maxval_ms;
        }
        else
        {
            scale_factor = 1.0/maxval_xy;
        }
    }
    
    let mut msfile = file.file_ms();
    let mut xyfile = file.file_xy();
    if msfile.get_samplerate() != xyfile.get_samplerate()
    {
        panic!("MS and XY file differ in sample rate!");
    }
    if msfile.len() != xyfile.len()
    {
        panic!("MS and XY file differ in size!");
    }
    let mut outfile = sndfile::OpenOptions::WriteOnly(
        WriteOptions::new(
            Recording::major_format(format),
            Recording::subtype_format(format),
            Endian::File,
            msfile.get_samplerate(),
            4
        )
    ).from_path(file.outfilename())
        .expect("Failed to open file for writing");
    
    let num_frames = 8192;
    let mut buffer_ms     = vec![0.0f32; num_frames * 2];
    let mut buffer_xy     = vec![0.0f32; num_frames * 2];
    let mut buffer_output = vec![0.0f32; num_frames * 4];

    loop {
        let n_samples: usize;
        match msfile.read_to_slice(&mut buffer_ms) {
            Ok(samples_read) => {
                n_samples = samples_read;
                for i in 0 .. samples_read / 2
                {
                    buffer_output[i*4+0] = buffer_ms[i*2+0] * scale_factor;
                    buffer_output[i*4+1] = buffer_ms[i*2+1] * scale_factor;
                }
            }
            Err(e) => { panic!("Error reading file: {:?})", e); }
        }
        match xyfile.read_to_slice(&mut buffer_xy) {
            Ok(samples_read) => {
                if n_samples != samples_read
                {
                    panic!("MS and XY files differ in length!");
                }
                for i in 0 .. samples_read / 2
                {
                    buffer_output[i*4+0] = buffer_xy[i*2+2] * scale_factor;
                    buffer_output[i*4+1] = buffer_xy[i*2+3] * scale_factor;
                }
            }
            Err(e) => { panic!("Error reading file: {:?})", e); }
        }
        outfile.write_from_slice(&mut buffer_output)
            .expect("Failed to write audio data");
        if n_samples < num_frames
        {
            break;
        }
    }
}

fn h2n2flac_2ch(normalize: bool,
                infilename: &String,
                outfilename: &String,
                filetype: &OutputFormat
) {

    // Determine how strong to scale the audio data for normalizing the file
    let mut scale_factor = 1.0;
    if normalize
    {
        scale_factor = 1.0/Recording::maxval(
            sndfile::OpenOptions::ReadOnly(ReadOptions::Auto).from_path(
                infilename).unwrap());
    }
    
    let mut infile = sndfile::OpenOptions::ReadOnly(ReadOptions::Auto).from_path(
        infilename).unwrap();
    
    let mut outfile = sndfile::OpenOptions::WriteOnly(
        WriteOptions::new(
            Recording::major_format(filetype),
            Recording::subtype_format(filetype),
            Endian::File,
            infile.get_samplerate(),
            2
        )
    ).from_path(outfilename)
        .expect("Failed to open file for writing");
    

    let num_frames = 8192;
    let mut buffer  = vec![0.0f32; num_frames];

    loop {
        let n_samples: usize;
        match infile.read_to_slice(&mut buffer) {
            Ok(samples_read) => {
                n_samples = samples_read;
                for i in 0 .. samples_read
                {
                    buffer[i] = buffer[i] * scale_factor;
                }
            }
            Err(e) => { panic!("Error reading file: {:?})", e); }
        }
        outfile.write_from_slice(&mut buffer)
            .expect("Failed to write audio data");
        if n_samples < num_frames
        {
            break;
        }
    }
}

fn h2n2flac(normalize: bool, file: Recording, format: &OutputFormat) {
    
    if *file.xy_exists() && *file.ms_exists()
    {
        h2n2flac_4ch(normalize, &file, format);
    }    

    if *file.xy_exists() && (!*file.ms_exists())
    {
        h2n2flac_2ch(normalize, file.name_xy(), file.outfilename(), format);
    }

    if *file.ms_exists() && (!*file.xy_exists())
    {
        h2n2flac_2ch(normalize, file.name_ms(), file.outfilename(), format);
    }
}

fn print_usage(program: &str, opts: Options) {
    let brief = format!("Usage: {} FILE(s) [options]", program);
    print!("{}", opts.usage(&brief));
}

fn print_version(program: &str) {
    let versioninfo = format!("{} version {}", program, env!("CARGO_PKG_VERSION"));
    print!("{}\n", (&versioninfo));
}

fn main() {
    let mut normalize: bool = false;
    let args: Vec<String> = env::args().collect();
    let program = args[0].clone();

    let mut opts = Options::new();
    opts.optflag("h", "help",      "print this help menu");
    opts.optflag("v", "version",   "print the version info");
    opts.optflag("n", "normalize", "Normalize the output");
//    opts.optflagopt("c", "clip", "Allow clipping of n permille of the samples", "CLIP_PERCENTAGE");
    let matches = match opts.parse(&args[1..]) {
        Ok(m) => { m }
        Err(f) => { panic!("{}", f.to_string()); }
    };
    if matches.opt_present("h") {
        print_usage(&program, opts);
        return;
    }
    if matches.opt_present("v") {
        print_version(&program);
        return;
    }
    if matches.opt_present("n") {
        normalize = true;
    }
    // if matches.opt_present("c") {
    //     clip = match matches.opt_str("c") {
    //         Some(s) => match s.parse::<f32>() {
    //             Ok(n) => n,
    //             Err(e) => panic!("The argument to c must be an integer. Got {e}"),
    //         }
    //         None => 1,
    //     };
    // }

    if matches.free.len() < 1
    {
        print_usage(&program, opts);
        return;        
    }

    let format : &OutputFormat;
    if program.ends_with("flac")
    {
        format = &OutputFormat::Flac;
    }
    else
    {
        format = &OutputFormat::Ogg;
    }
    
    // Rust doesn't like matches.free.for_each(move |file| h2n2flac(normalize, clip, file));
    for file in &*matches.free
    {
        h2n2flac(normalize, Recording::new(file), format);
    }

}
