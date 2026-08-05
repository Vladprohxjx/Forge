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
use std::process::Command as StdCommand;

#[derive(Parser)]
#[command(name = "forge", version = "1.0.0", about = "A high-performance Cargo workflow orchestrator")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Build all workspace members with smart caching
    Build { 
        #[arg(short, long, help = "Build in release mode")]
        release: bool 
    },
    /// Format all workspace members
    Fmt,
    /// Lint all workspace members using clippy
    Lint,
    /// Run tests for all workspace members
    Test,
    /// Clean target and log directories
    Clean,
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

fn is_relevant_file(path: &Path) -> bool {
    let path_str = path.to_string_lossy();
    !path_str.contains("target") 
        && !path_str.contains(".git") 
        && !path_str.contains(".logs")
        && !path_str.contains("Cargo.lock")
        && path.extension().map_or(false, |ext| {
            matches!(ext.to_str().unwrap_or(""), "rs" | "toml" | "md" | "json" | "yaml" | "yml")
        })
}

fn calculate_project_hash(path: &Path) -> anyhow::Result<String> {
    let mut hasher = Sha256::new();
    
    if !path.exists() {
        return Ok(String::new());
    }
    
    for entry in WalkDir::new(path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let p = entry.path();
        if is_relevant_file(p) {
            if let Ok(mut file) = fs::File::open(p) {
                let mut buffer = [0; 8192];
                while let Ok(n) = file.read(&mut buffer) {
                    if n == 0 { break; }
                    hasher.update(&buffer[..n]);
                }
            }
        }
    }
    let hash_bytes = hasher.finalize();
    let mut hash_string = String::with_capacity(hash_bytes.len() * 2);
    for byte in hash_bytes.iter() {
        hash_string.push_str(&format!("{:02x}", byte));
    }
    Ok(hash_string)
}

fn sanitize_command(cmd: &str) -> anyhow::Result<String> {
    // Basic validation to prevent command injection
    if cmd.contains('\n') || cmd.contains('\r') {
        anyhow::bail!("Invalid command: contains newline characters");
    }
    Ok(cmd.to_string())
}

async fn run_hook(commands: &[String], cwd: &Path, envs: &HashMap<String, String>, forge_vars: HashMap<&str, String>) -> anyhow::Result<()> {
    for cmd_str in commands {
        let sanitized_cmd = sanitize_command(cmd_str)?;
        let mut processed_cmd = sanitized_cmd;
        
        for (key, val) in &forge_vars {
            // Only allow alphanumeric variable names
            if key.chars().all(|c| c.is_alphanumeric() || c == '_') {
                processed_cmd = processed_cmd.replace(&format!("${}", key), val);
                processed_cmd = processed_cmd.replace(&format!("${{{}}}", key), val);
            }
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
        cmd.current_dir(cwd);
        
        let status = cmd.status().await?;
        if !status.success() { 
            anyhow::bail!("Hook failed with exit code {:?}: {}", status.code(), processed_cmd); 
        }
    }
    Ok(())
}

async fn run_parallel_tool(name: String, path: String, tool: &'static str, pb: ProgressBar) -> anyhow::Result<()> {
    pb.set_message(format!("Running {} on {}", tool, name));
    let mut cmd = tokio::process::Command::new("cargo");
    cmd.arg(tool).current_dir(&path);
    
    if tool == "clippy" { 
        cmd.args(["--", "-D", "warnings"]); 
    } else if tool == "test" {
        cmd.arg("--quiet");
    }
    
    let output = cmd.output().await?;
    pb.finish_and_clear();
    
    if output.status.success() {
        println!("{} {}: {}", "OK".green().bold(), tool.to_uppercase(), name);
    } else {
        println!("{} {}: {}", "ERROR".red().bold(), tool.to_uppercase(), name);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        if !stderr.is_empty() { eprintln!("{}", stderr); }
        if !stdout.is_empty() { eprintln!("{}", stdout); }
    }
    Ok(())
}

async fn run_build(name: String, member_path: String, is_release: bool, config: Arc<ForgeConfig>, pb: ProgressBar) -> anyhow::Result<()> {
    let path_obj = PathBuf::from(&member_path);
    
    if !path_obj.exists() {
        pb.finish_and_clear();
        println!("{} SKIP: {} path does not exist", "WARN".yellow().bold(), name);
        return Ok(());
    }
    
    let current_hash = calculate_project_hash(&path_obj)?;
    let hash_dir = Path::new("target").join(".hashes");
    fs::create_dir_all(&hash_dir)?;
    let hash_file = hash_dir.join(format!("{}.hash", name));
    
    // Check if project is up to date using cached hash
    let cached_hash = fs::read_to_string(&hash_file).unwrap_or_default();
    if !cached_hash.is_empty() && cached_hash == current_hash {
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
        let log_file = log_dir.join(format!("{}.log", name));
        if let Ok(mut file) = fs::File::create(&log_file) {
            let _ = file.write_all(&output.stdout);
            let _ = file.write_all(&output.stderr);
        }
    }

    if output.status.success() {
        if config.build.strip && name != "forge" {
            let exe_name = if cfg!(target_os = "windows") { 
                format!("{}.exe", name) 
            } else { 
                name.clone() 
            };
            
            let bin_path = if member_path == "." { 
                Path::new("target").join(profile).join(&exe_name)
            } else {
                path_obj.join("target").join(profile).join(&exe_name)
            };
            
            if bin_path.exists() {
                let strip_result = StdCommand::new("strip").arg(&bin_path).status();
                if let Err(e) = strip_result {
                    eprintln!("{} Warning: Could not strip binary: {}", "WARN".yellow().bold(), e);
                }
            }
        }
        
        run_hook(&config.afterbuild.commands, &path_obj, &config.env, forge_vars).await?;
        fs::write(hash_file, current_hash)?;
        pb.finish_and_clear();
        println!("{} OK: {}", "FINISHED".green().bold(), name);
    } else {
        pb.finish_and_clear();
        println!("{} ERROR: {}", "FAILED".red().bold(), name);
        if !config.build.log { 
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.is_empty() { eprintln!("{}", stderr); }
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    
    // Load configuration with graceful fallback to defaults
    let config_raw = fs::read_to_string("forge.toml")
        .or_else(|_| fs::read_to_string("Forge.toml"))
        .unwrap_or_default();
    
    let config: ForgeConfig = match toml::from_str(&config_raw) {
        Ok(cfg) => cfg,
        Err(e) if config_raw.is_empty() => ForgeConfig::default(),
        Err(e) => {
            eprintln!("{} Failed to parse config file: {}", "WARN".yellow().bold(), e);
            ForgeConfig::default()
        }
    };
    let config: Arc<ForgeConfig> = Arc::new(config);
    
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
                match Manifest::from_slice(&fs::read(&p)?) {
                    Ok(manifest) => {
                        if let Some(package) = manifest.package {
                            projects.push((package.name, m.clone()));
                        } else {
                            eprintln!("{} No [package] section in {}", "WARN".yellow().bold(), p.display());
                        }
                    }
                    Err(e) => {
                        eprintln!("{} Failed to parse {}: {}", "ERROR".red().bold(), p.display(), e);
                    }
                }
            } else {
                eprintln!("{} Cargo.toml not found in {}", "WARN".yellow().bold(), m);
            }
        }
    } else if Path::new("Cargo.toml").exists() {
        match Manifest::from_slice(&fs::read("Cargo.toml")?) {
            Ok(manifest) => {
                if let Some(package) = manifest.package {
                    projects.push((package.name, ".".to_string()));
                } else {
                    eprintln!("{} No [package] section in Cargo.toml", "WARN".yellow().bold());
                }
            }
            Err(e) => {
                eprintln!("{} Failed to parse Cargo.toml: {}", "ERROR".red().bold(), e);
            }
        }
    }
    
    if projects.is_empty() {
        println!("{} No projects found to process", "INFO".blue().bold());
        return Ok(());
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
                    match s.acquire().await {
                        Ok(_p) => {
                            let res = run_build(name, path, release, cfg_c, pb).await;
                            m_pb.inc(1);
                            res
                        }
                        Err(e) => {
                            pb.finish_and_clear();
                            eprintln!("{} Failed to acquire semaphore: {}", "ERROR".red().bold(), e);
                            m_pb.inc(1);
                            Err(anyhow::anyhow!("Semaphore error"))
                        }
                    }
                }));
            }
            for t in tasks { 
                if let Err(e) = t.await {
                    eprintln!("{} Task failed: {}", "ERROR".red().bold(), e);
                }
            }
        },
        Commands::Fmt => {
            let mut tasks = vec![];
            for (name, path) in projects {
                let s = Arc::clone(&sem);
                let pb = mp.insert_before(&main_pb, ProgressBar::new_spinner());
                let m_pb = main_pb.clone();
                tasks.push(tokio::spawn(async move {
                    match s.acquire().await {
                        Ok(_p) => {
                            let res = run_parallel_tool(name, path, "fmt", pb).await;
                            m_pb.inc(1);
                            res
                        }
                        Err(e) => {
                            pb.finish_and_clear();
                            eprintln!("{} Failed to acquire semaphore: {}", "ERROR".red().bold(), e);
                            m_pb.inc(1);
                            Err(anyhow::anyhow!("Semaphore error"))
                        }
                    }
                }));
            }
            for t in tasks { 
                if let Err(e) = t.await {
                    eprintln!("{} Task failed: {}", "ERROR".red().bold(), e);
                }
            }
        },
        Commands::Lint => {
            let mut tasks = vec![];
            for (name, path) in projects {
                let s = Arc::clone(&sem);
                let pb = mp.insert_before(&main_pb, ProgressBar::new_spinner());
                let m_pb = main_pb.clone();
                tasks.push(tokio::spawn(async move {
                    match s.acquire().await {
                        Ok(_p) => {
                            let res = run_parallel_tool(name, path, "clippy", pb).await;
                            m_pb.inc(1);
                            res
                        }
                        Err(e) => {
                            pb.finish_and_clear();
                            eprintln!("{} Failed to acquire semaphore: {}", "ERROR".red().bold(), e);
                            m_pb.inc(1);
                            Err(anyhow::anyhow!("Semaphore error"))
                        }
                    }
                }));
            }
            for t in tasks { 
                if let Err(e) = t.await {
                    eprintln!("{} Task failed: {}", "ERROR".red().bold(), e);
                }
            }
        },
        Commands::Test => {
            let mut tasks = vec![];
            for (name, path) in projects {
                let s = Arc::clone(&sem);
                let pb = mp.insert_before(&main_pb, ProgressBar::new_spinner());
                let m_pb = main_pb.clone();
                tasks.push(tokio::spawn(async move {
                    match s.acquire().await {
                        Ok(_p) => {
                            let res = run_parallel_tool(name, path, "test", pb).await;
                            m_pb.inc(1);
                            res
                        }
                        Err(e) => {
                            pb.finish_and_clear();
                            eprintln!("{} Failed to acquire semaphore: {}", "ERROR".red().bold(), e);
                            m_pb.inc(1);
                            Err(anyhow::anyhow!("Semaphore error"))
                        }
                    }
                }));
            }
            for t in tasks { 
                if let Err(e) = t.await {
                    eprintln!("{} Task failed: {}", "ERROR".red().bold(), e);
                }
            }
        },
        Commands::Clean => {
            main_pb.finish_and_clear();
            let mut cleaned = Vec::new();
            for dir in ["target", ".logs"] { 
                if Path::new(dir).exists() { 
                    if let Err(e) = fs::remove_dir_all(dir) {
                        eprintln!("{} Failed to remove {}: {}", "WARN".yellow().bold(), dir, e);
                    } else {
                        cleaned.push(dir);
                    }
                }
            }
            if !cleaned.is_empty() {
                println!("{} Cleaned: {}", "OK".green().bold(), cleaned.join(", "));
            } else {
                println!("{} Nothing to clean", "INFO".blue().bold());
            }
        }
    }
    
    main_pb.finish_and_clear();
    Ok(())
}