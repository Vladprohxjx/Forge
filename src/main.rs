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
use std::collections::HashMap;

#[derive(Parser)]
#[command(name = "forge", version = "0.1.2")]
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
    Test,
}

#[derive(Deserialize, Default)]
struct ForgeConfig {
    workspace: Option<WorkspaceConfig>,
    #[serde(default)]
    build: BuildSettings,
    #[serde(default)]
    env: HashMap<String, String>,
    #[serde(default)]
    prebuild: HookConfig,
    #[serde(default)]
    afterbuild: HookConfig,
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
    pub target: Option<String>,
    pub features: Option<Vec<String>>,
    #[serde(default)]
    pub all_features: bool,
}

#[derive(Deserialize, Default)]
struct HookConfig {
    #[serde(default)]
    commands: Vec<String>,
}

impl Default for BuildSettings {
    fn default() -> Self {
        Self { 
            strip: default_strip(), 
            threads: default_threads(), 
            log: default_log(),
            target: None,
            features: None,
            all_features: false,
        }
    }
}

fn default_strip() -> bool { false }
fn default_threads() -> usize { 2 }
fn default_log() -> bool { false }

fn calculate_project_hash(path: &Path) -> anyhow::Result<String> {
    let mut hasher = Sha256::new();
    for entry in WalkDir::new(path).into_iter().filter_map(|e| e.ok()) {
        if entry.file_type().is_file() {
            let p = entry.path();
            if !p.to_string_lossy().contains("target") && !p.to_string_lossy().contains(".git") && !p.to_string_lossy().contains(".logs") {
                if let Ok(mut file) = fs::File::open(p) {
                    let mut buffer = [0; 8192];
                    while let Ok(n) = file.read(&mut buffer) {
                        if n == 0 { break; }
                        hasher.update(&buffer[..n]);
                    }
                }
            }
        }
    }
    Ok(hasher.finalize().iter().map(|b| format!("{:02x}", b)).collect())
}

async fn run_hook(commands: &[String], cwd: &Path, envs: &HashMap<String, String>, forge_vars: HashMap<&str, String>) -> anyhow::Result<()> {
    for cmd_str in commands {
        let mut processed_cmd = cmd_str.clone();
        for (key, val) in &forge_vars {
            processed_cmd = processed_cmd.replace(&format!("${}", key), val);
        }

        let mut cmd = if cfg!(target_os = "windows") {
            let mut c = tokio::process::Command::new("cmd");
            c.args(["/C", &processed_cmd]);
            c
        } else {
            let mut c = tokio::process::Command::new("sh");
            c.args(["-c", &processed_cmd]);
            c
        };
        
        cmd.envs(envs);
        let status = cmd.current_dir(cwd).status().await?;
        if !status.success() { anyhow::bail!("Hook failed: {}", processed_cmd); }
    }
    Ok(())
}

async fn run_parallel_tool(name: String, path: String, tool: &'static str, pb: ProgressBar) -> anyhow::Result<()> {
    pb.set_message(format!("Running {} on {}", tool, name));
    let mut cmd = tokio::process::Command::new("cargo");
    cmd.arg(tool).current_dir(&path);
    if tool == "clippy" { cmd.args(["--", "-D", "warnings"]); }
    let output = cmd.output().await?;
    pb.finish_and_clear();
    if output.status.success() {
        println!("{} {}: {}", "OK".green().bold(), tool.to_uppercase(), name);
    } else {
        println!("{} {}: {}", "ERROR".red().bold(), tool.to_uppercase(), name);
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
    }
    Ok(())
}

async fn run_build(name: String, member_path: String, is_release: bool, config: Arc<ForgeConfig>, pb: ProgressBar) -> anyhow::Result<()> {
    let path_obj = PathBuf::from(&member_path);
    let current_hash = calculate_project_hash(&path_obj)?;
    let hash_dir = Path::new("target").join(".hashes");
    fs::create_dir_all(&hash_dir)?;
    let hash_file = hash_dir.join(format!("{}.hash", name));
    
    if hash_file.exists() && fs::read_to_string(&hash_file).unwrap_or_default() == current_hash {
        pb.finish_and_clear();
        println!("{} DONE: {} is up to date", "OK".green().bold(), name);
        return Ok(())
    }

    let profile = if is_release { "release" } else { "debug" };
    let mut forge_vars = HashMap::new();
    forge_vars.insert("FORGE_PROJECT", name.clone());
    forge_vars.insert("FORGE_PROFILE", profile.to_string());
    forge_vars.insert("FORGE_PATH", member_path.clone());

    pb.set_message(format!("Building {}", name));
    run_hook(&config.prebuild.commands, &path_obj, &config.env, forge_vars.clone()).await?;

    let mut cmd = tokio::process::Command::new("cargo");
    cmd.arg("build").arg("--manifest-path").arg(path_obj.join("Cargo.toml"));
    
    if is_release { cmd.arg("--release"); }
    if let Some(target) = &config.build.target { cmd.arg("--target").arg(target); }
    if config.build.all_features { cmd.arg("--all-features"); }
    if let Some(features) = &config.build.features {
        cmd.arg("--features").arg(features.join(","));
    }

    let output = cmd.output().await?;

    if config.build.log {
        let log_dir = Path::new(".logs");
        fs::create_dir_all(log_dir)?;
        let mut file = fs::File::create(log_dir.join(format!("{}.log", name)))?;
        file.write_all(&output.stdout)?;
        file.write_all(&output.stderr)?;
    }

    if output.status.success() {
        if config.build.strip && name != "forge" {
            let exe = if cfg!(target_os = "windows") { format!("{}.exe", name) } else { name.clone() };
            let bin_path = if member_path == "." { 
                Path::new("target").join(profile).join(&exe)
            } else {
                path_obj.join("target").join(profile).join(&exe)
            };
            if bin_path.exists() { let _ = std::process::Command::new("strip").arg(&bin_path).status(); }
        }
        
        run_hook(&config.afterbuild.commands, &path_obj, &config.env, forge_vars).await?;
        fs::write(hash_file, current_hash)?;
        pb.finish_and_clear();
        println!("{} OK: {}", "FINISHED".green().bold(), name);
    } else {
        pb.finish_and_clear();
        println!("{} ERROR: {}", "FAILED".red().bold(), name);
        if !config.build.log { eprintln!("{}", String::from_utf8_lossy(&output.stderr)); }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let config_raw = fs::read_to_string("forge.toml").or_else(|_| fs::read_to_string("Forge.toml")).unwrap_or_default();
    let config: Arc<ForgeConfig> = Arc::new(toml::from_str(&config_raw).unwrap_or_default());
    
    if let Ok(repo) = Repository::open(".") {
        let mut opts = git2::StatusOptions::new();
        if let Ok(statuses) = repo.statuses(Some(&mut opts)) {
            if !statuses.is_empty() { println!("{} Uncommitted changes detected.\n", "WARN:".yellow().bold()); }
        }
    }

    let mut projects = Vec::new();
    if let Some(ws) = &config.workspace {
        for m in &ws.members {
            let p = PathBuf::from(m).join("Cargo.toml");
            if p.exists() {
                let manifest = Manifest::from_slice(&fs::read(p)?)?;
                projects.push((manifest.package.unwrap().name, m.clone()));
            }
        }
    } else if Path::new("Cargo.toml").exists() {
        let manifest = Manifest::from_slice(&fs::read("Cargo.toml")?)?;
        projects.push((manifest.package.unwrap().name, ".".to_string()));
    }

    let sem = Arc::new(Semaphore::new(config.build.threads));
    let mp = MultiProgress::new();
    let main_pb = mp.add(ProgressBar::new(projects.len() as u64));
    main_pb.set_style(ProgressStyle::default_bar()
        .template("[{bar:40.blue}] {pos}/{len} projects")?
        .progress_chars("=>-"));

    match cli.command {
        Commands::Build { release } => {
            let mut tasks = vec![];
            for (name, path) in projects {
                let s = Arc::clone(&sem);
                let pb = mp.insert_before(&main_pb, ProgressBar::new_spinner());
                let m_pb = main_pb.clone();
                let cfg_c = Arc::clone(&config);
                tasks.push(tokio::spawn(async move {
                    let _p = s.acquire().await.unwrap();
                    let res = run_build(name, path, release, cfg_c, pb).await;
                    m_pb.inc(1);
                    res
                }));
            }
            for t in tasks { let _ = t.await; }
        },
        Commands::Fmt => {
            let mut tasks = vec![];
            for (name, path) in projects {
                let s = Arc::clone(&sem);
                let pb = mp.insert_before(&main_pb, ProgressBar::new_spinner());
                let m_pb = main_pb.clone();
                tasks.push(tokio::spawn(async move {
                    let _p = s.acquire().await.unwrap();
                    let res = run_parallel_tool(name, path, "fmt", pb).await;
                    m_pb.inc(1);
                    res
                }));
            }
            for t in tasks { let _ = t.await; }
        },
        Commands::Lint => {
            let mut tasks = vec![];
            for (name, path) in projects {
                let s = Arc::clone(&sem);
                let pb = mp.insert_before(&main_pb, ProgressBar::new_spinner());
                let m_pb = main_pb.clone();
                tasks.push(tokio::spawn(async move {
                    let _p = s.acquire().await.unwrap();
                    let res = run_parallel_tool(name, path, "clippy", pb).await;
                    m_pb.inc(1);
                    res
                }));
            }
            for t in tasks { let _ = t.await; }
        },
        Commands::Test => {
            let mut tasks = vec![];
            for (name, path) in projects {
                let s = Arc::clone(&sem);
                let pb = mp.insert_before(&main_pb, ProgressBar::new_spinner());
                let m_pb = main_pb.clone();
                tasks.push(tokio::spawn(async move {
                    let _p = s.acquire().await.unwrap();
                    let res = run_parallel_tool(name, path, "test", pb).await;
                    m_pb.inc(1);
                    res
                }));
            }
            for t in tasks { let _ = t.await; }
        },
        Commands::Clean => {
            main_pb.finish_and_clear();
            for dir in ["target", ".logs"] { if Path::new(dir).exists() { fs::remove_dir_all(dir)?; } }
            println!("{} Workspace cleaned.", "OK".green().bold());
        }
    }
    main_pb.finish_and_clear();
    Ok(())
}