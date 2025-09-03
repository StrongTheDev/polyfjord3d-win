use anyhow::{anyhow, Context, Result};
use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::blocking::Client;
use serde::Deserialize;
use std::env;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

const COLMAP_REPO: &str = "colmap/colmap";
const GLOMAP_REPO: &str = "colmap/glomap";

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None, disable_version_flag = true)]
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

#[derive(clap::ValueEnum, Clone, Debug, Copy)]
enum Tool {
    Colmap,
    Glomap,
}

#[derive(Deserialize, Debug)]
struct Release {
    assets: Vec<Asset>,
    tag_name: String,
}

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

fn prompt_and_download_tool(tool_name: &str, repo: &str, dest_dir: &Path) -> Result<PathBuf> {
    println!("[INFO] {} not found.", tool_name);
    println!("[INFO] Fetching latest releases from GitHub...");

    let release = get_latest_release(repo)?;
    println!("[INFO] Latest release is {}", release.tag_name);

    let mut downloadable_assets: Vec<Asset> = release
        .assets
        .into_iter()
        .filter(|a| a.name.contains("windows") && a.name.ends_with(".zip"))
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
) -> Result<PathBuf> {
    if let Some(path) = arg_path {
        if path.exists() {
            return Ok(path);
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
        return Ok(path);
    }

    let install_dir = env::current_dir()?.join(install_dir_name);
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
        return Ok(path);
    }

    prompt_and_download_tool(name, repo, &install_dir)
}

fn check_ffmpeg(arg_path: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = arg_path {
        if path.exists() {
            return Ok(path);
        } else {
            return Err(anyhow!(
                "Provided path for ffmpeg does not exist: {}",
                path.display()
            ));
        }
    }

    if let Ok(path) = which::which("ffmpeg") {
        println!("[INFO] Found ffmpeg in PATH: {}", path.display());
        return Ok(path);
    }

    let install_dir = env::current_dir()?.join("ffmpeg");
    if !install_dir.exists() {
        fs::create_dir_all(&install_dir)?;
    }

    if let Some(path) = find_executable(&install_dir, "ffmpeg") {
        println!("[INFO] Found ffmpeg in : {}", path.display());
        return Ok(path);
    }

    Err(anyhow!("ffmpeg not found. Please install it and ensure it's in your PATH, or place it in a 'ffmpeg' folder."))
}

fn process_video(
    video_path: &Path,
    scenes_dir: &Path,
    ffmpeg_path: &Path,
    tool_path: &Path,
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

    // 1. Extract frames
    println!("[1/4] Extracting frames...");
    let output = Command::new(ffmpeg_path)
        .arg("-i")
        .arg(video_path)
        .arg("-qscale:v")
        .arg("2")
        .arg(images_dir.join("frame_%06d.jpg"))
        .output()
        .context("Failed to execute ffmpeg.")?;

    if !output.status.success() {
        io::stderr().write_all(&output.stderr)?;
        return Err(anyhow!("ffmpeg failed for {}", video_name));
    }

    // 2. Feature extraction
    println!("[2/4] Feature extraction...");
    let db_path = scene_dir.join("database.db");
    let mut extractor_cmd = Command::new(tool_path);
    extractor_cmd
        .arg("feature_extractor")
        .arg("--database_path")
        .arg(&db_path)
        .arg("--image_path")
        .arg(&images_dir);

    if let Tool::Colmap = tool {
        extractor_cmd
            .arg("--ImageReader.single_camera")
            .arg("1")
            .arg("--SiftExtraction.use_gpu")
            .arg("1");
    }

    let output = extractor_cmd
        .output()
        .context("Failed to execute feature_extractor")?;
    if !output.status.success() {
        io::stderr().write_all(&output.stderr)?;
        return Err(anyhow!("feature_extractor failed for {}", video_name));
    }

    // 3. Feature matching
    println!("[3/4] Feature matching...");
    let mut matcher_cmd = Command::new(tool_path);
    matcher_cmd
        .arg("sequential_matcher")
        .arg("--database_path")
        .arg(&db_path);

    if let Tool::Colmap = tool {
        matcher_cmd.arg("--SequentialMatching.overlap").arg("15");
    }

    let output = matcher_cmd
        .output()
        .context("Failed to execute sequential_matcher")?;
    if !output.status.success() {
        io::stderr().write_all(&output.stderr)?;
        return Err(anyhow!("sequential_matcher failed for {}", video_name));
    }

    // 4. Sparse reconstruction (mapping)
    println!("[4/4] Sparse reconstruction...");
    let num_threads = num_cpus::get().to_string();
    let output = Command::new(tool_path)
        .arg("mapper")
        .arg("--database_path")
        .arg(&db_path)
        .arg("--image_path")
        .arg(&images_dir)
        .arg("--output_path")
        .arg(&sparse_dir)
        .arg("--Mapper.num_threads")
        .arg(num_threads)
        .output()
        .context("Failed to execute mapper")?;

    if !output.status.success() {
        io::stderr().write_all(&output.stderr)?;
        return Err(anyhow!("mapper failed for {}", video_name));
    }

    // Export model to TXT
    let model_path = sparse_dir.join("0");
    if model_path.exists() {
        Command::new(tool_path)
            .arg("model_converter")
            .arg("--input_path")
            .arg(model_path)
            .arg("--output_path")
            .arg(&sparse_dir)
            .arg("--output_type")
            .arg("TXT")
            .output()
            .context("Failed to execute model_converter")?;
    }

    println!("✔ Finished {}", video_name);
    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();

    let ffmpeg_path = check_ffmpeg(args.ffmpeg_path)?;
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

    let tool_path = check_dependency(tool_name, repo_name, args.tool_path, install_dir)?;

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
