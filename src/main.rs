use clap::{Parser, Subcommand};
use colored::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Semaphore;
use git2::Repository;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use serde::Deserialize;
use cargo_toml::Manifest;
use sha2::{Sha256, Digest};
use walkdir::WalkDir;
use std::io::{Read, Write};

#[derive(Parser)]
#[command(name = "forge", version = "0.1.0")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Build { #[arg(short, long)] release: bool },
    Clean,
    Fmt,
    Lint,
}

#[derive(Deserialize, Default)]
struct ForgeConfig {
    workspace: Option<WorkspaceConfig>,
    #[serde(default)]
    build: BuildSettings,
}

#[derive(Deserialize, Default)]
struct WorkspaceConfig {
    #[serde(default)]
    members: Vec<String>,
}

#[derive(Deserialize)]
struct BuildSettings {
    #[serde(default = "default_strip")]
    strip: bool,
    #[serde(default = "default_threads")]
    threads: usize,
    #[serde(default = "default_log")]
    log: bool,
}

impl Default for BuildSettings {
    fn default() -> Self {
        Self { 
            strip: default_strip(), 
            threads: default_threads(), 
            log: default_log() 
        }
    }
}

fn default_strip() -> bool { false }
fn default_threads() -> usize { 2 }
fn default_log() -> bool { false }

fn calculate_project_hash(path: &Path) -> anyhow::Result<String> {
    let mut hasher = Sha256::new();
    let entries: Vec<_> = WalkDir::new(path).into_iter().filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| !e.path().to_string_lossy().contains("target"))
        .collect();
    
    for entry in entries {
        if let Ok(mut file) = fs::File::open(entry.path()) {
            let mut buffer = [0; 8192];
            while let Ok(n) = file.read(&mut buffer) {
                if n == 0 { break; }
                hasher.update(&buffer[..n]);
            }
        }
    }
    Ok(hex::encode(hasher.finalize()))
}

fn is_cache_valid(name: &str, current_hash: &str) -> bool {
    let path = PathBuf::from("target").join(format!("{}.hash", name));
    if let Ok(old) = fs::read_to_string(path) { return old == current_hash; }
    false
}

fn save_cache_hash(name: &str, hash: &str) -> anyhow::Result<()> {
    let dir = PathBuf::from("target");
    if !dir.exists() { fs::create_dir_all(&dir)?; }
    fs::write(dir.join(format!("{}.hash", name)), hash)?;
    Ok(())
}

fn write_to_log(name: &str, stdout: &[u8], stderr: &[u8]) -> anyhow::Result<()> {
    let log_dir = Path::new(".logs");
    if !log_dir.exists() { fs::create_dir_all(log_dir)?; }
    let mut file = fs::File::create(log_dir.join(format!("{}.log", name)))?;
    file.write_all(b"--- STDOUT ---\n")?;
    file.write_all(stdout)?;
    file.write_all(b"\n--- STDERR ---\n")?;
    file.write_all(stderr)?;
    Ok(())
}

async fn run_tool(name: String, path: String, tool: &str, pb: ProgressBar) -> anyhow::Result<()> {
    pb.set_message(format!("Running {} on {}", tool, name));
    let manifest_path = if path == "." { PathBuf::from("Cargo.toml") } else { Path::new(&path).join("Cargo.toml") };
    
    let mut cmd = tokio::process::Command::new("cargo");
    cmd.arg(tool).arg("--manifest-path").arg(manifest_path);
    if tool == "clippy" { cmd.arg("--").arg("-D").arg("warnings"); }

    let output = cmd.output().await?;
    pb.finish_and_clear();

    if output.status.success() {
        println!("{} {}: {}", "Success:".green().bold(), tool, name);
    } else {
        println!("{} {}: {}", "Error:".red().bold(), tool, name);
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
    }
    Ok(())
}

async fn run_build(name: String, member_path: String, is_release: bool, strip: bool, log: bool, pb: ProgressBar) -> anyhow::Result<()> {
    let path_obj = Path::new(&member_path);
    let current_hash = calculate_project_hash(path_obj)?;
    
    if is_cache_valid(&name, &current_hash) {
        pb.finish_and_clear();
        println!("{} Cached: {}", "Success:".green().bold(), name);
        return Ok(());
    }

    pb.set_message(format!("Building {}", name));
    let manifest_path = if member_path == "." { PathBuf::from("Cargo.toml") } else { path_obj.join("Cargo.toml") };
    
    let mut cmd = tokio::process::Command::new("cargo");
    cmd.arg("build").arg("--manifest-path").arg(manifest_path);
    if is_release { cmd.arg("--release"); }

    let output = cmd.output().await?;

    if log {
        let _ = write_to_log(&name, &output.stdout, &output.stderr);
    }

    if output.status.success() {
        let profile = if is_release { "release" } else { "debug" };
        let bin_path = PathBuf::from("target").join(profile).join(&name);
        if bin_path.exists() && strip { 
            let _ = std::process::Command::new("strip").arg(&bin_path).status(); 
        }
        save_cache_hash(&name, &current_hash)?;
        pb.finish_and_clear();
        println!("{} Finished: {}", "Success:".green().bold(), name);
    } else {
        pb.finish_and_clear();
        println!("{} Failed: {}", "Error:".red().bold(), name);
        if !log {
            eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        } else {
            println!("Check logs in .logs/{}.log", name);
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let config: ForgeConfig = toml::from_str(&fs::read_to_string("Forge.toml").unwrap_or_default()).unwrap_or_default();
    
    if let Ok(repo) = Repository::open(".") {
        let mut opts = git2::StatusOptions::new();
        if let Ok(statuses) = repo.statuses(Some(&mut opts)) {
            if !statuses.is_empty() { 
                println!("{} Uncommitted changes detected.\n", "WARN:".yellow().bold()); 
            }
        }
    }

    let mut projects = Vec::new();
    if let Some(ws) = config.workspace {
        for m in ws.members {
            let p = PathBuf::from(&m).join("Cargo.toml");
            if p.exists() {
                let manifest = Manifest::from_slice(&fs::read(p)?)?;
                projects.push((manifest.package.unwrap().name, m));
            }
        }
    }
    if projects.is_empty() && Path::new("Cargo.toml").exists() {
        let manifest = Manifest::from_slice(&fs::read("Cargo.toml")?)?;
        projects.push((manifest.package.unwrap().name, ".".to_string()));
    }

    match cli.command {
        Commands::Build { release } => {
            let mp = MultiProgress::new();
            let main_pb = mp.add(ProgressBar::new(projects.len() as u64));
            main_pb.set_style(ProgressStyle::default_bar()
                .template("[{bar:40.cyan/blue}] {pos}/{len} projects")?
                .progress_chars("#>-"));
            
            let sem = Arc::new(Semaphore::new(config.build.threads));
            let mut tasks = vec![];
            
            for (name, path) in projects {
                let s = Arc::clone(&sem);
                let pb = mp.insert_before(&main_pb, ProgressBar::new_spinner());
                let m_pb = main_pb.clone();
                let strip = config.build.strip;
                let log = config.build.log;
                
                tasks.push(tokio::spawn(async move {
                    let _p = s.acquire().await.unwrap();
                    let res = run_build(name, path, release, strip, log, pb).await;
                    m_pb.inc(1);
                    res
                }));
            }
            for t in tasks { let _ = t.await; }
            main_pb.finish_and_clear();
        },
        Commands::Fmt => {
            for (name, path) in projects {
                let pb = ProgressBar::new_spinner();
                run_tool(name, path, "fmt", pb).await?;
            }
        },
        Commands::Lint => {
            for (name, path) in projects {
                let pb = ProgressBar::new_spinner();
                run_tool(name, path, "clippy", pb).await?;
            }
        },
        Commands::Clean => {
            if Path::new("target").exists() {
                fs::remove_dir_all("target")?;
            }
            if Path::new(".logs").exists() {
                fs::remove_dir_all(".logs")?;
            }
            println!("Project cleaned.");
        }
    }
    Ok(())
}