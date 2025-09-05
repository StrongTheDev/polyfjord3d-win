//! This crate provides a command-line tool for processing videos to generate 3D scenes
//! using photogrammetry tools like COLMAP or GLOMAP. It automates the process of
//! extracting frames, feature matching, and sparse reconstruction.
//!
//! Original credit: [Polyfjord](https://www.youtube.com/@Polyfjord)

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use dirs::data_local_dir;
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::blocking::Client;
use serde::Deserialize;
use std::env;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

/// GitHub repository for COLMAP.
const COLMAP_REPO: &str = "colmap/colmap";
/// GitHub repository for GLOMAP.
const GLOMAP_REPO: &str = "colmap/glomap";
/// GitHub repository for FFmpeg builds.
const FFMPEG_REPO: &str = "BtbN/FFmpeg-Builds";

/// polyfjord3d command-line utility.
/// This tool converts your videos into photogrammetry models - for 3D tracking in Blender 3D.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None, disable_version_flag = true, color = clap::ColorChoice::Always, after_help = "Example:\n    polyfjord3d  video.mp4  video.mov")]
struct Args {
    /// List of video files to process.
    #[arg(required = true)]
    videos: Vec<PathBuf>,

    /// Photogrammetry tool to use.
    #[arg(long, short = 't', value_enum, default_value_t = Tool::Glomap)]
    tool: Tool,

    /// Path to the scenes directory.
    #[arg(long, default_value = "scenes")]
    scenes_dir: PathBuf,

    /// Force re-processing of existing scenes.
    #[arg(long, short = 'f')]
    force: bool,

    /// Path to ffmpeg executable.
    #[arg(long)]
    ffmpeg_path: Option<PathBuf>,

    /// Path to colmap or glomap executable.
    #[arg(long)]
    tool_path: Option<PathBuf>,

    /// Print version information.
    #[arg(short = 'v', long = "version", action = clap::ArgAction::Version)]
    version_flag: Option<bool>,
}

/// Enum representing the available photogrammetry tools.
#[derive(clap::ValueEnum, Clone, Debug, Copy)]
enum Tool {
    /// Use COLMAP for reconstruction.
    Colmap,
    /// Use GLOMAP for reconstruction.
    Glomap,
}

/// Represents a GitHub release.
#[derive(Deserialize, Debug)]
struct Release {
    assets: Vec<Asset>,
    tag_name: String,
}

/// Represents a downloadable asset from a GitHub release.
#[derive(Deserialize, Debug, Clone)]
struct Asset {
    name: String,
    browser_download_url: String,
}

fn get_latest_release(repo: &str) -> Result<Release> {
    let url = format!("https://api.github.com/repos/{}/releases/latest", repo);
    let client = Client::builder().user_agent("polyfjord3d-rust").build()?;
    let release = client.get(&url).send()?.json::<Release>()?;
    Ok(release)
}

fn download_file(url: &str, path: &Path) -> Result<()> {
    let mut response = reqwest::blocking::get(url)?;
    let total_size = response.content_length().unwrap_or(0);

    let pb = ProgressBar::new(total_size);
    pb.set_style(ProgressStyle::default_bar()
        .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")?
        .progress_chars("#>-"));

    let mut file = File::create(path)?;
    let mut downloaded: u64 = 0;
    let mut buffer = [0; 8192];

    loop {
        let bytes_read = response.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        file.write_all(&buffer[..bytes_read])?;
        downloaded += bytes_read as u64;
        pb.set_position(downloaded);
    }

    pb.finish_with_message("Downloaded");
    Ok(())
}

fn unzip_file(path: &Path, dest: &Path) -> Result<()> {
    let file = File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)?;

    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let outpath = match file.enclosed_name() {
            Some(path) => dest.join(path),
            None => continue,
        };

        if (*file.name()).ends_with('/') {
            fs::create_dir_all(&outpath)?;
        } else {
            if let Some(p) = outpath.parent() {
                if !p.exists() {
                    fs::create_dir_all(p)?;
                }
            }
            let mut outfile = File::create(&outpath)?;
            io::copy(&mut file, &mut outfile)?;
        }
    }
    Ok(())
}

fn get_install_dir() -> Result<PathBuf> {
    let dir = data_local_dir()
        .ok_or_else(|| anyhow!("Failed to get local data directory"))?
        .join("polyfjord3d");
    if !dir.exists() {
        fs::create_dir_all(&dir)?;
    }
    Ok(dir)
}

fn prompt_and_download_tool(tool_name: &str, repo: &str, dest_dir: &Path) -> Result<PathBuf> {
    println!(
        "[INFO] {} not found in PATH or at ({})",
        tool_name,
        dest_dir.display()
    );
    println!("[INFO] Fetching latest releases from GitHub...");

    let release = get_latest_release(repo)?;
    println!("[INFO] Latest release is {}", release.tag_name);

    let mut downloadable_assets: Vec<Asset> = release
        .assets
        .into_iter()
        .filter(|a| a.name.contains("win") && a.name.ends_with(".zip"))
        .collect();

    if downloadable_assets.is_empty() {
        return Err(anyhow!("No suitable Windows .zip assets found in the latest release. Please install {} manually.", tool_name));
    }

    println!("Please choose a package to download:");
    for (i, asset) in downloadable_assets.iter().enumerate() {
        println!("[{}] {}", i + 1, asset.name);
    }

    let choice: usize = loop {
        print!("> ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        match input.trim().parse::<usize>() {
            Ok(n) if n > 0 && n <= downloadable_assets.len() => break n - 1,
            _ => println!("Invalid choice. Please enter a number from the list."),
        }
    };

    let asset = downloadable_assets.remove(choice);
    let download_url = asset.browser_download_url;
    let file_name = asset.name;
    let zip_path = dest_dir.join(&file_name);

    println!("[INFO] Downloading {}...", file_name);
    download_file(&download_url, &zip_path)?;

    println!("[INFO] Unzipping {}...", file_name);
    unzip_file(&zip_path, dest_dir)?;

    println!("[INFO] Cleaning up downloaded archive...");
    fs::remove_file(&zip_path)?;

    println!("[INFO] {} installed successfully.", tool_name);

    find_executable(dest_dir, tool_name).ok_or_else(|| {
        anyhow!(
            "Failed to find {} executable after installation.",
            tool_name
        )
    })
}

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

fn check_dependency(
    name: &str,
    repo: &str,
    arg_path: Option<PathBuf>,
    install_dir_name: &str,
) -> Result<(PathBuf, bool)> {
    if let Some(path) = arg_path {
        if path.exists() {
            return Ok((path, false));
        } else {
            return Err(anyhow!(
                "Provided path for {} does not exist: {}",
                name,
                path.display()
            ));
        }
    }

    if let Ok(path) = which::which(name) {
        println!("[INFO] Found {} in PATH: {}", name, path.display());
        return Ok((path, false));
    }

    let install_dir = get_install_dir()?.join(install_dir_name);
    if !install_dir.exists() {
        fs::create_dir_all(&install_dir)?;
    }

    if let Some(path) = find_executable(&install_dir, name) {
        println!(
            "[INFO] Found {} in {}: {}",
            name,
            install_dir_name,
            path.display()
        );
        return Ok((path, false));
    }

    prompt_and_download_tool(name, repo, &install_dir).map(|path| (path, true))
    // Err(anyhow!("{} not found. Please install it and ensure it's in your PATH, or place it in the install directory.", name))
}

fn run_command(command: &mut Command, video_name: &str, step_name: &str) -> Result<()> {
    let output = command
        .output()
        .with_context(|| format!("Failed to execute {}", step_name))?;

    if !output.status.success() {
        io::stderr().write_all(&output.stderr)?;
        Err(anyhow!("{} failed for {}", step_name, video_name))
    } else {
        Ok(())
    }
}

/// Processes a single video file.
///
/// # Arguments
///
/// * `video_path` - The path to the video file.
/// * `scenes_dir` - The directory to store the processed scenes.
/// * `ffmpeg_path` - The path to the ffmpeg executable.
/// * `tool_path` - The path to the photogrammetry tool executable.
/// * `colmap_path` - The path to the COLMAP executable.
/// * `tool` - The photogrammetry tool to use.
/// * `force` - Whether to force re-processing of existing scenes.
///
/// # Returns
///
/// A `Result` indicating success or failure.
fn process_video(
    video_path: &Path,
    scenes_dir: &Path,
    ffmpeg_path: &Path,
    tool_path: &Path,
    colmap_path: &Path,
    tool: Tool,
    force: bool,
) -> Result<()> {
    let video_name = video_path.file_stem().unwrap().to_str().unwrap();
    println!("\n=== Processing {} ===", video_name);

    let scene_dir = scenes_dir.join(video_name);
    let images_dir = scene_dir.join("images");
    let sparse_dir = scene_dir.join("sparse");

    if scene_dir.exists() {
        if force {
            println!("[INFO] Scene directory exists. Forcing overwrite.");
            fs::remove_dir_all(&scene_dir)?;
        } else {
            println!("[INFO] Skipping {} - already processed.", video_name);
            return Ok(());
        }
    }

    fs::create_dir_all(&images_dir)?;
    fs::create_dir_all(&sparse_dir)?;

    // 1. Extract frames from the video using ffmpeg.
    println!("[1/4] Extracting frames...");
    run_command(
        Command::new(ffmpeg_path)
            .arg("-i")
            .arg(video_path)
            .arg("-qscale:v")
            .arg("2")
            .arg(images_dir.join("frame_%06d.jpg")),
        video_name,
        "ffmpeg",
    )?;

    // 2. Run COLMAP feature extractor to detect keypoints in the images.
    println!("[2/4] Feature extraction...");
    let db_path = scene_dir.join("database.db");
    run_command(
        Command::new(colmap_path)
            .arg("feature_extractor")
            .arg("--database_path")
            .arg(&db_path)
            .arg("--image_path")
            .arg(&images_dir)
            .arg("--ImageReader.single_camera")
            .arg("1")
            .arg("--SiftExtraction.use_gpu")
            .arg("1")
            .arg("--SiftExtraction.max_image_size")
            .arg("4096"),
        video_name,
        "feature_extractor",
    )?;

    // 3. Run COLMAP sequential matcher to find corresponding features between images.
    println!("[3/4] Feature matching...");
    run_command(
        Command::new(colmap_path)
            .arg("sequential_matcher")
            .arg("--database_path")
            .arg(&db_path)
            .arg("--SequentialMatching.overlap")
            .arg("15"),
        video_name,
        "sequential_matcher",
    )?;

    // 4. Perform sparse reconstruction to create a 3D point cloud.
    println!("[4/4] Sparse reconstruction...");
    let mut mapper_cmd = Command::new(tool_path);
    mapper_cmd
        .arg("mapper")
        .arg("--database_path")
        .arg(&db_path)
        .arg("--image_path")
        .arg(&images_dir)
        .arg("--output_path")
        .arg(&sparse_dir);

    if let Tool::Colmap = tool {
        let num_threads = num_cpus::get().to_string();
        mapper_cmd.arg("--Mapper.num_threads").arg(num_threads);
    }

    run_command(&mut mapper_cmd, video_name, "mapper")?;

    // Export the reconstructed model to a human-readable TXT format.
    let model_path = sparse_dir.join("0");
    if model_path.exists() {
        println!("[INFO] Exporting model to TXT...");
        if let Tool::Glomap = tool {
            // For Glomap, the model needs to be converted twice.
            run_command(
                Command::new(colmap_path)
                    .arg("model_converter")
                    .arg("--input_path")
                    .arg(&model_path)
                    .arg("--output_path")
                    .arg(&model_path)
                    .arg("--output_type")
                    .arg("TXT"),
                video_name,
                "model_converter (for glomap)",
            )?;
        }
        run_command(
            Command::new(colmap_path)
                .arg("model_converter")
                .arg("--input_path")
                .arg(&model_path)
                .arg("--output_path")
                .arg(&sparse_dir)
                .arg("--output_type")
                .arg("TXT"),
            video_name,
            "model_converter",
        )?;
    }

    println!("✔ Finished {}", video_name);
    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();

    let mut need_to_modify_path = false;
    let (ffmpeg_path, did_download) = check_dependency("ffmpeg", FFMPEG_REPO, args.ffmpeg_path, "ffmpeg")?;
    if did_download {
        need_to_modify_path = true;
    }

    let tool_name = match args.tool {
        Tool::Colmap => "colmap",
        Tool::Glomap => "glomap",
    };
    let repo_name = match args.tool {
        Tool::Colmap => COLMAP_REPO,
        Tool::Glomap => GLOMAP_REPO,
    };
    let install_dir = match args.tool {
        Tool::Colmap => "colmap",
        Tool::Glomap => "glomap",
    };

    let (tool_path, did_download) = check_dependency(tool_name, repo_name, args.tool_path, install_dir)?;
    if did_download {
        need_to_modify_path = true;
    }

    // For Glomap, we also need colmap
    let (colmap_path, did_download) = if let Tool::Glomap = args.tool {
        println!("[INFO] Glomap pipeline requires COLMAP for some steps.");
        check_dependency("colmap", COLMAP_REPO, None, "colmap")?
    } else {
        (tool_path.clone(), did_download)
    };

    if did_download {
        need_to_modify_path = true;
    }

    if need_to_modify_path {
        println!("[INFO] Need to modify PATH environment variable.");
        run_command(Command::new("modify_polyfjord_path").arg(&colmap_path.parent().unwrap()), "modify_path", "modify_path")?;
    }

    let colmap_install_dir = get_install_dir()?.join("colmap");
    let plugins_path = colmap_install_dir.join("plugins");
    let mut qt_plugin_path = plugins_path.into_os_string();
    if let Ok(existing_path) = env::var("QT_PLUGIN_PATH") {
        qt_plugin_path.push(";");
        qt_plugin_path.push(existing_path);
    }
    env::set_var("QT_PLUGIN_PATH", qt_plugin_path);

    if !args.scenes_dir.exists() {
        fs::create_dir_all(&args.scenes_dir)?;
    }

    println!("==============================================================");
    println!(" Starting on {} video(s)...", args.videos.len());
    println!("==============================================================");

    for video_path in &args.videos {
        if let Err(e) = process_video(
            video_path,
            &args.scenes_dir,
            &ffmpeg_path,
            &tool_path,
            &colmap_path,
            args.tool,
            args.force,
        ) {
            eprintln!("[ERROR] Failed to process {}: {}", video_path.display(), e);
        }
    }

    println!("\n--------------------------------------------------------------");
    println!(
        " All jobs finished - results are in {}",
        args.scenes_dir.display()
    );
    println!("--------------------------------------------------------------");

    Ok(())
}
