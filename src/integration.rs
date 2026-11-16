//! Adding "convert" entries to the right-click menu of file managers.
//!
//! * Dolphin (KDE) reads *service menus*: `.desktop` files in
//!   `~/.local/share/kio/servicemenus/` (Plasma 5.85 and newer), which must
//!   be executable.
//! * GNOME Files (Nautilus) and Nemo (Cinnamon) offer the executable files
//!   in `~/.local/share/{nautilus,nemo}/scripts/` under "Scripts"; a folder
//!   there becomes a submenu. The selected files are passed as arguments.
//! * Windows Explorer reads verbs from the registry. The entries go to
//!   `HKEY_CURRENT_USER\Software\Classes\SystemFileAssociations\.wav`, so
//!   neither administrator rights nor the default program for WAV files
//!   are touched.
//!
//! All entries start `h2n2flac-gui`, which shows the progress in a window.
//! Everything here is per user; nothing needs root.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The name of the submenu that holds the entries.
pub const MENU_TITLE: &str = "H2n recording";

/// One entry of the submenu.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Action {
    /// An identifier without spaces (desktop file action, registry key).
    pub id: &'static str,
    pub label: &'static str,
    /// The options `h2n2flac-gui` is started with, before the file names.
    pub args: &'static [&'static str],
}

pub const ACTIONS: [Action; 4] = [
    Action {
        id: "flac",
        label: "Convert to FLAC",
        args: &[],
    },
    Action {
        id: "flacNormalized",
        label: "Convert to FLAC (normalized)",
        args: &["--normalize"],
    },
    Action {
        id: "ogg",
        label: "Convert to Ogg Vorbis",
        args: &["--ogg"],
    },
    Action {
        id: "oggNormalized",
        label: "Convert to Ogg Vorbis (normalized)",
        args: &["--ogg", "--normalize"],
    },
];

/// The name of the graphical front end, which is installed next to the
/// command line tools.
pub fn gui_executable_name() -> String {
    format!("h2n2flac-gui{}", std::env::consts::EXE_SUFFIX)
}

/// Finds `h2n2flac-gui` next to the running program.
pub fn find_gui_executable() -> io::Result<PathBuf> {
    let me = std::env::current_exe()?;
    let gui = me
        .parent()
        .map(|dir| dir.join(gui_executable_name()))
        .ok_or_else(|| io::Error::other("cannot determine the program's folder"))?;
    if !gui.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "{} not found; build it with `cargo build --release --features gui`",
                gui.display()
            ),
        ));
    }
    // An absolute, symlink-free path keeps working when the menu is used
    // from another folder.
    gui.canonicalize()
}

// ---------------------------------------------------------------------------
// Dolphin

/// Quotes one argument of a desktop file's `Exec=` key.
///
/// The desktop entry specification asks for two rounds: inside a quoted
/// argument `"`, `` ` ``, `$` and `\` get a backslash, then the whole value
/// is escaped as a string, which doubles every backslash again. A literal
/// `%` must be written `%%`, or it would be read as a field code like `%F`.
pub fn desktop_exec_arg(arg: &str) -> String {
    let mut quoted = String::from("\"");
    for c in arg.chars() {
        match c {
            '"' | '`' | '$' => {
                quoted.push_str("\\\\");
                quoted.push(c);
            }
            '\\' => quoted.push_str("\\\\\\\\"),
            '%' => quoted.push_str("%%"),
            _ => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

/// The service menu for Dolphin: a submenu with all [`ACTIONS`] for WAV
/// files. `%F` stands for all selected files, so one window converts them.
pub fn dolphin_service_menu(gui: &Path) -> String {
    let exe = desktop_exec_arg(&gui.to_string_lossy());
    let ids: Vec<&str> = ACTIONS.iter().map(|a| a.id).collect();
    // ServiceTypes and X-KDE-ServiceTypes are what Plasma 5 looks for,
    // Plasma 6 only needs MimeType and Actions; both are harmless.
    let mut text = format!(
        "[Desktop Entry]\n\
         Type=Service\n\
         Name={MENU_TITLE}\n\
         ServiceTypes=KonqPopupMenu/Plugin\n\
         X-KDE-ServiceTypes=KonqPopupMenu/Plugin\n\
         MimeType=audio/x-wav;audio/wav;audio/vnd.wave;\n\
         Actions={};\n\
         X-KDE-Submenu={MENU_TITLE}\n\
         Icon=audio-x-generic\n",
        ids.join(";")
    );
    for action in &ACTIONS {
        let mut exec = exe.clone();
        for arg in action.args {
            exec.push(' ');
            exec.push_str(arg);
        }
        text.push_str(&format!(
            "\n[Desktop Action {}]\nName={}\nIcon=audio-x-generic\nExec={exec} %F\n",
            action.id, action.label
        ));
    }
    text
}

// ---------------------------------------------------------------------------
// Nautilus and Nemo

/// Quotes `arg` for a POSIX shell.
pub fn sh_quote(arg: &str) -> String {
    format!("'{}'", arg.replace('\'', r"'\''"))
}

/// The script Nautilus or Nemo runs for one action. The file managers pass
/// the selected files as arguments.
pub fn file_manager_script(gui: &Path, action: &Action) -> String {
    let mut command = sh_quote(&gui.to_string_lossy());
    for arg in action.args {
        command.push(' ');
        command.push_str(arg);
    }
    format!(
        "#!/bin/sh\n\
         # Added by `h2n2flac --install-menu`; `h2n2flac --uninstall-menu` removes it.\n\
         exec {command} \"$@\"\n"
    )
}

/// `$XDG_DATA_HOME`, or `~/.local/share` if that isn't set.
pub fn data_home() -> io::Result<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_DATA_HOME").filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(|home| PathBuf::from(home).join(".local/share"))
        .ok_or_else(|| io::Error::other("neither XDG_DATA_HOME nor HOME is set"))
}

fn service_menu_path(data_home: &Path) -> PathBuf {
    data_home.join("kio/servicemenus/h2n2flac.desktop")
}

/// The submenu folders of the file managers that use scripts.
fn script_dirs(data_home: &Path) -> [PathBuf; 2] {
    [
        data_home.join("nautilus/scripts").join(MENU_TITLE),
        data_home.join("nemo/scripts").join(MENU_TITLE),
    ]
}

/// Writes `contents` to `path`, creating its folder, and makes it
/// executable (which Plasma 6 requires for service menus, too).
fn write_executable(path: &Path, contents: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

/// Installs the Dolphin service menu and the Nautilus and Nemo scripts
/// below `data_home`. Returns the files written.
pub fn install_freedesktop(data_home: &Path, gui: &Path) -> io::Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    let menu = service_menu_path(data_home);
    write_executable(&menu, &dolphin_service_menu(gui))?;
    written.push(menu);
    for dir in script_dirs(data_home) {
        for action in &ACTIONS {
            let script = dir.join(action.label);
            write_executable(&script, &file_manager_script(gui, action))?;
            written.push(script);
        }
    }
    Ok(written)
}

/// Removes what [`install_freedesktop`] installed. Returns what it removed;
/// missing files are no error.
pub fn uninstall_freedesktop(data_home: &Path) -> io::Result<Vec<PathBuf>> {
    let mut removed = Vec::new();
    let menu = service_menu_path(data_home);
    if menu.exists() {
        fs::remove_file(&menu)?;
        removed.push(menu);
    }
    for dir in script_dirs(data_home) {
        if dir.exists() {
            // The folder holds nothing but our scripts.
            fs::remove_dir_all(&dir)?;
            removed.push(dir);
        }
    }
    Ok(removed)
}

// ---------------------------------------------------------------------------
// Windows Explorer

/// The registry key of the submenu. Deleting it removes everything.
pub const WINDOWS_MENU_KEY: &str =
    r"HKCU\Software\Classes\SystemFileAssociations\.wav\shell\h2n2flac";

/// One registry value; `name` `None` is the key's default value.
#[derive(Debug, Clone, PartialEq)]
pub struct RegistryValue {
    pub key: String,
    pub name: Option<&'static str>,
    pub data: String,
}

/// The registry values of the Explorer submenu (a "cascading menu":
/// `SubCommands` set to an empty string makes Explorer look for the
/// entries in the `shell` subkey).
///
/// Explorer starts one process per selected file (`%1`). When both files
/// of a recording are selected, the second process finds the first one's
/// `.part` file and steps aside (see `convert::claim_part`).
pub fn windows_registry_values(gui: &Path) -> Vec<RegistryValue> {
    let value = |key: String, name, data: &str| RegistryValue {
        key,
        name,
        data: data.to_string(),
    };
    let mut values = vec![
        value(WINDOWS_MENU_KEY.to_string(), Some("MUIVerb"), MENU_TITLE),
        value(WINDOWS_MENU_KEY.to_string(), Some("SubCommands"), ""),
    ];
    for action in &ACTIONS {
        let key = format!(r"{WINDOWS_MENU_KEY}\shell\{}", action.id);
        let mut command = format!("\"{}\"", gui.display());
        for arg in action.args {
            command.push(' ');
            command.push_str(arg);
        }
        command.push_str(" \"%1\"");
        values.push(value(key.clone(), Some("MUIVerb"), action.label));
        values.push(value(format!(r"{key}\command"), None, &command));
    }
    values
}

#[cfg(windows)]
fn run_reg(args: &[&str]) -> io::Result<()> {
    let status = std::process::Command::new("reg").args(args).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("reg {} failed", args.join(" "))))
    }
}

#[cfg(windows)]
fn install_windows(gui: &Path) -> io::Result<Vec<String>> {
    // Start from a clean state so that renamed entries don't linger.
    let _ = uninstall_windows();
    for v in windows_registry_values(gui) {
        let mut args = vec!["add", v.key.as_str()];
        match v.name {
            Some(name) => args.extend(["/v", name]),
            None => args.push("/ve"),
        }
        args.extend(["/d", v.data.as_str(), "/f"]);
        run_reg(&args)?;
    }
    Ok(vec![WINDOWS_MENU_KEY.to_string()])
}

#[cfg(windows)]
fn uninstall_windows() -> io::Result<Vec<String>> {
    run_reg(&["delete", WINDOWS_MENU_KEY, "/f"])?;
    Ok(vec![WINDOWS_MENU_KEY.to_string()])
}

// ---------------------------------------------------------------------------
// Entry points for `h2n2flac --install-menu` / `--uninstall-menu`

/// Adds the menu entries for the file managers of this platform. Returns a
/// description of what was installed.
pub fn install() -> io::Result<Vec<String>> {
    let gui = find_gui_executable()?;
    #[cfg(windows)]
    {
        install_windows(&gui)
    }
    #[cfg(not(windows))]
    {
        Ok(install_freedesktop(&data_home()?, &gui)?
            .iter()
            .map(|p| p.display().to_string())
            .collect())
    }
}

/// Removes the entries [`install`] added.
pub fn uninstall() -> io::Result<Vec<String>> {
    #[cfg(windows)]
    {
        uninstall_windows()
    }
    #[cfg(not(windows))]
    {
        Ok(uninstall_freedesktop(&data_home()?)?
            .iter()
            .map(|p| p.display().to_string())
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convert::tests::test_dir;

    #[test]
    fn exec_arguments_are_quoted_per_desktop_entry_spec() {
        assert_eq!(desktop_exec_arg("/usr/bin/x"), "\"/usr/bin/x\"");
        assert_eq!(desktop_exec_arg("/my apps/x"), "\"/my apps/x\"");
        // " → \" in the argument → \\" in the file.
        assert_eq!(desktop_exec_arg("a\"b"), "\"a\\\\\"b\"");
        assert_eq!(desktop_exec_arg("$HOME"), "\"\\\\$HOME\"");
        // \ → \\ in the argument → \\\\ in the file.
        assert_eq!(desktop_exec_arg("a\\b"), "\"a\\\\\\\\b\"");
        assert_eq!(desktop_exec_arg("100%"), "\"100%%\"");
    }

    #[test]
    fn service_menu_offers_every_action_for_wav_files() {
        let menu = dolphin_service_menu(Path::new("/opt/h2n/h2n2flac-gui"));
        assert!(menu.starts_with("[Desktop Entry]\n"));
        assert!(menu.contains("MimeType=audio/x-wav;"));
        assert!(menu.contains("Actions=flac;flacNormalized;ogg;oggNormalized;\n"));
        assert!(menu.contains(&format!("X-KDE-Submenu={MENU_TITLE}\n")));
        assert!(menu.contains(
            "[Desktop Action oggNormalized]\n\
             Name=Convert to Ogg Vorbis (normalized)\n\
             Icon=audio-x-generic\n\
             Exec=\"/opt/h2n/h2n2flac-gui\" --ogg --normalize %F\n"
        ));
        assert_eq!(menu.matches("[Desktop Action ").count(), ACTIONS.len());
    }

    #[test]
    fn shell_quoting_survives_single_quotes() {
        assert_eq!(sh_quote("/a b/c"), "'/a b/c'");
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
    }

    #[test]
    fn script_passes_options_and_all_selected_files() {
        let script = file_manager_script(Path::new("/opt/h2n2flac-gui"), &ACTIONS[3]);
        assert!(script.starts_with("#!/bin/sh\n"));
        assert!(script.ends_with("exec '/opt/h2n2flac-gui' --ogg --normalize \"$@\"\n"));
    }

    #[test]
    fn install_and_uninstall_round_trip() {
        let home = test_dir("install");
        let gui = Path::new("/opt/h2n2flac-gui");

        let written = install_freedesktop(&home, gui).unwrap();

        let menu = home.join("kio/servicemenus/h2n2flac.desktop");
        assert!(written.contains(&menu));
        assert_eq!(
            fs::read_to_string(&menu).unwrap(),
            dolphin_service_menu(gui)
        );
        let script = home
            .join("nautilus/scripts")
            .join(MENU_TITLE)
            .join("Convert to FLAC");
        assert_eq!(
            fs::read_to_string(&script).unwrap(),
            file_manager_script(gui, &ACTIONS[0])
        );
        assert!(home.join("nemo/scripts").join(MENU_TITLE).is_dir());
        assert_eq!(written.len(), 1 + 2 * ACTIONS.len());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for file in &written {
                let mode = fs::metadata(file).unwrap().permissions().mode();
                assert_eq!(mode & 0o111, 0o111, "{} not executable", file.display());
            }
        }

        // Installing again (e.g. after moving the program) just overwrites.
        install_freedesktop(&home, Path::new("/elsewhere/h2n2flac-gui")).unwrap();
        assert!(fs::read_to_string(&menu).unwrap().contains("/elsewhere/"));

        let removed = uninstall_freedesktop(&home).unwrap();
        assert_eq!(removed.len(), 3);
        assert!(!menu.exists());
        assert!(!script.exists());
        // Our folders are gone, the file managers' own are kept.
        assert!(home.join("nautilus/scripts").is_dir());
        // A second uninstall finds nothing to do.
        assert!(uninstall_freedesktop(&home).unwrap().is_empty());
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn registry_entries_form_a_submenu_with_one_command_per_action() {
        let gui = Path::new(r"C:\Program Files\h2n2flac\h2n2flac-gui.exe");
        let values = windows_registry_values(gui);
        assert_eq!(values.len(), 2 + 2 * ACTIONS.len());
        assert_eq!(
            values[0],
            RegistryValue {
                key: WINDOWS_MENU_KEY.to_string(),
                name: Some("MUIVerb"),
                data: MENU_TITLE.to_string(),
            }
        );
        assert_eq!(values[1].name, Some("SubCommands"));
        assert_eq!(values[1].data, "");
        let ogg_command = values
            .iter()
            .find(|v| v.key == format!(r"{WINDOWS_MENU_KEY}\shell\ogg\command"))
            .unwrap();
        assert_eq!(ogg_command.name, None);
        assert_eq!(
            ogg_command.data,
            r#""C:\Program Files\h2n2flac\h2n2flac-gui.exe" --ogg "%1""#
        );
    }
}
