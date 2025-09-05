//! This crate provides a tool to modify the system's PATH environment variable.
//! It can add or remove directories from the PATH, either for the current user or
//! for the entire system. This is primarily used by the installer to make the
//! main application and its tools accessible from the command line.

use clap::Parser;
use dirs::data_local_dir;
use std::path::{Path, PathBuf, absolute};
use winreg::enums::*;
use winreg::RegKey;

/// Command-line arguments for the modify_path tool.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = Some("Updates environment variables of the tools needed for this pipeline."), disable_version_flag = true)]
struct Args {
    /// Installation directory of the main application`
    install_dir: PathBuf,

    /// User mode or system mode
    #[arg(long, short = 'm', value_enum, default_value_t = Mode::User)]
    mode: Mode,

    /// Broadcast a message to all top-level windows to notify them of the environment change.
    /// This allows other applications (like File Explorer) to recognize the new PATH immediately.
    #[arg(long, short = 'b')]
    broadcast: bool,

    /// Print version information.
    #[arg(short = 'v', long = "version", action = clap::ArgAction::Version)]
    version_flag: Option<bool>,
}

/// Enum representing the mode of operation (User or System).
#[derive(clap::ValueEnum, Clone, Debug, Copy)]
enum Mode {
    /// Modify the PATH for the current user.
    User,
    /// Modify the PATH for the entire system (requires administrator privileges).
    System,
}

/// Finds an executable within a directory, checking common locations.
///
/// # Arguments
///
/// * `dir` - The directory to search in.
/// * `name` - The name of the executable (without the extension).
///
/// # Returns
///
/// An `Option` containing the path to the executable if found.
fn find_executable(dir: &Path, name: &str) -> Option<PathBuf> {
    let exe_name = format!("{}.exe", name);
    let primary_path = dir.join(&exe_name);
    if primary_path.exists() {
        return Some(primary_path);
    }
    let bin_path = dir.join("bin").join(&exe_name);
    if bin_path.exists() {
        return Some(bin_path);
    }
    None
}

/// The main entry point of the application.
fn main() {
    let args = Args::parse();

    println!("====== DO NOT CLOSE THIS WINDOW. IT WILL CLOSE AUTOMATICALLY. ======");

    if let Err(e) = run(&args) {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

/// The main logic for adding paths to the PATH environment variable.
///
/// # Arguments
///
/// * `args` - The command-line arguments.
///
/// # Returns
///
/// A `Result` indicating success or failure.
fn run(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    let tools_base_dir = data_local_dir()
        .ok_or("Failed to get local data directory")?
        .join("polyfjord3d");

    if !tools_base_dir.exists() {
        println!("Tools directory not found. Nothing to add to PATH.");
        return Ok(());
    }

    let tools = ["colmap", "glomap", "ffmpeg"];

    let (reg_hive, reg_key_path) = match args.mode {
        Mode::User => (HKEY_CURRENT_USER, "Environment"),
        Mode::System => (
            HKEY_LOCAL_MACHINE,
            r"System\CurrentControlSet\Control\Session Manager\Environment",
        ),
    };

    let root = RegKey::predef(reg_hive);
    // Open the environment registry key with read and write permissions.
    let env_key = root.open_subkey_with_flags(reg_key_path, KEY_READ | KEY_WRITE)?;
    let current_path: String = env_key.get_value("Path")?;
    let mut new_paths = current_path.clone();
    let mut added_any = false;
    let time = std::time::Instant::now();

    // Add install dir to path
    let install_dir: PathBuf = absolute(args.install_dir.clone())?;
    if !current_path
        .split(';')
        .any(|p| Path::new(p) == install_dir)
    {
        println!(
            "Adding {} to PATH. ({} ms)",
            install_dir.display(),
            time.elapsed().as_millis()
        );
        new_paths.push(';');
        new_paths.push_str(install_dir.to_str().ok_or("Invalid path")?);
        added_any = true;
    } else {
        println!(
            "{} is already in PATH. ({} ms)",
            args.install_dir.display(),
            time.elapsed().as_millis()
        );
    }

    for tool_name in &tools {
        let tool_dir = tools_base_dir.join(tool_name);
        if let Some(executable_path) = find_executable(&tool_dir, tool_name) {
            if let Some(executable_parent_dir) = executable_path.parent() {
                if !current_path
                    .split(';')
                    .any(|p| std::path::Path::new(p) == executable_parent_dir)
                {
                    println!(
                        "Adding {} to PATH. ({} ms)",
                        executable_parent_dir.display(),
                        time.elapsed().as_millis()
                    );
                    new_paths.push(';');
                    new_paths.push_str(executable_parent_dir.to_str().ok_or("Invalid path")?);
                    added_any = true;
                } else {
                    println!(
                        "{} is already in PATH. ({} ms)",
                        executable_parent_dir.display(),
                        time.elapsed().as_millis()
                    );
                }
            }
        }
    }

    if !added_any {
        println!(
            "All tool paths are already in the PATH environment variable. ({} ms)",
            time.elapsed().as_millis()
        );
        return Ok(());
    }

    // Set the updated PATH environment variable in the registry.
    env_key.set_value("Path", &new_paths)?;
    println!(
        "Updated PATH environment variable. ({} ms)",
        time.elapsed().as_millis()
    );

    if args.broadcast {
        // Broadcast a message to all top-level windows to notify them of the environment change.
        // This allows other applications (like File Explorer) to recognize the new PATH immediately.
        use winapi::shared::minwindef::LPARAM;
        use winapi::shared::minwindef::WPARAM;
        use winapi::um::winuser::{
            SendMessageTimeoutA, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
        };

        let result = unsafe {
            SendMessageTimeoutA(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                0 as WPARAM,
                "Environment".as_ptr() as LPARAM,
                SMTO_ABORTIFHUNG,
                5000,
                std::ptr::null_mut(),
            )
        };

        if result == 0 {
            eprintln!(
                "Failed to broadcast environment variable change. ({} ms)",
                time.elapsed().as_millis()
            );
        }
    }

    println!("Modification complete. ({} ms)", time.elapsed().as_millis());

    Ok(())
}
