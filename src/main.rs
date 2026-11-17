//! The command line tools `h2n2flac` and `h2n2ogg`.

use getopts::Options;
use h2n2flac::convert::{Converted, OutputFormat, Recording, h2n2flac};
use h2n2flac::progress::Progress;
use h2n2flac::{integration, selection};
use std::env;
use std::process::ExitCode;

fn print_usage(program: &str, opts: Options) {
    let brief = format!("Usage: {} FILE(s) [options]", program);
    print!("{}", opts.usage(&brief));
}

fn print_version(program: &str) {
    println!("{} version {}", program, env!("CARGO_PKG_VERSION"));
}

/// Runs `--install-menu` or `--uninstall-menu` and reports the result.
fn change_menu(install: bool) -> ExitCode {
    let result = if install {
        integration::install()
    } else {
        integration::uninstall()
    };
    match result {
        Ok(paths) => {
            let verb = if install { "Installed" } else { "Removed" };
            if paths.is_empty() {
                println!("Nothing to remove.");
            }
            for path in paths {
                println!("{verb} {path}");
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("Error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let program = args[0].clone();

    let mut opts = Options::new();
    opts.optflag("h", "help", "print this help menu");
    opts.optflag("v", "version", "print the version info");
    opts.optflag("n", "normalize", "Normalize the output");
    opts.optflag(
        "",
        "install-menu",
        "Add conversion entries to the right-click menu of Dolphin, \
         GNOME Files, Nemo or Windows Explorer (needs h2n2flac-gui)",
    );
    opts.optflag(
        "",
        "uninstall-menu",
        "Remove the entries --install-menu added",
    );
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
        return ExitCode::SUCCESS;
    }
    if matches.opt_present("v") {
        print_version(&program);
        return ExitCode::SUCCESS;
    }
    if matches.opt_present("install-menu") {
        return change_menu(true);
    }
    if matches.opt_present("uninstall-menu") {
        return change_menu(false);
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
        return ExitCode::SUCCESS;
    }

    let format = OutputFormat::from_program_name(&program);

    let join = !matches.opt_present("no-join");

    // `h2n2flac *.WAV` names every recording twice (MS and XY file), and
    // a split recording once per part.
    for file in selection::unique_recordings(&matches.free, join) {
        let recording = Recording::new(&file, format, join);
        for parts in recording.joined() {
            eprintln!("Joining {}", parts.join(" + "));
        }
        // The bar appears only if stderr is a terminal.
        let mut progress = Progress::on_stderr(recording.output());
        if h2n2flac(normalize, &recording, format, &mut progress) == Converted::AlreadyRunning {
            eprintln!(
                "Skipping {file}: {} is being written by another process.",
                recording.output()
            );
        }
    }
    ExitCode::SUCCESS
}
