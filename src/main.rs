use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use colored::*;
use cargo_toml::Manifest;
use git2::Repository;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use walkdir::WalkDir;

#[derive(Parser)]
#[command(name = "forge", version, about = "A high-performance Cargo workflow orchestrator")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Clone, Copy)]
enum Commands {
    /// Build all configured workspace members
    Build { #[arg(short, long)] release: bool },
    /// Format all configured workspace members
    Fmt,
    /// Run Clippy for all configured workspace members
    Lint,
    /// Run tests for all configured workspace members
    Test,
    /// Remove target and log directories from the project root
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
    #[serde(default)]
    strip: bool,
    #[serde(default = "default_threads")]
    threads: usize,
    #[serde(default)]
    log: bool,
    target: Option<String>,
    features: Option<Vec<String>>,
    #[serde(default)]
    all_features: bool,
}

impl Default for BuildSettings {
    fn default() -> Self {
        Self { strip: false, threads: default_threads(), log: false, target: None, features: None, all_features: false }
    }
}

#[derive(Deserialize, Default)]
struct HookConfig {
    #[serde(default)]
    commands: Vec<String>,
}

fn default_threads() -> usize { 2 }

fn load_config() -> Result<ForgeConfig> {
    let raw = fs::read_to_string("forge.toml")
        .or_else(|_| fs::read_to_string("Forge.toml"))
        .unwrap_or_default();
    if raw.is_empty() {
        return Ok(ForgeConfig::default());
    }
    toml::from_str(&raw).context("failed to parse forge.toml / Forge.toml")
}

fn load_projects(config: &ForgeConfig) -> Result<Vec<(String, String)>> {
    let mut projects = Vec::new();
    let members: Vec<String> = if let Some(workspace) = &config.workspace {
        workspace.members.clone()
    } else if Path::new("Cargo.toml").is_file() {
        vec![".".to_string()]
    } else {
        Vec::new()
    };

    for member in members {
        let manifest_path = PathBuf::from(&member).join("Cargo.toml");
        if !manifest_path.is_file() {
            eprintln!("{} Missing {}", "WARN".yellow().bold(), manifest_path.display());
            continue;
        }
        let manifest = Manifest::from_slice(&fs::read(&manifest_path)?)
            .with_context(|| format!("failed to parse {}", manifest_path.display()))?;
        if let Some(package) = manifest.package {
            projects.push((package.name, member));
        } else {
            eprintln!("{} No [package] section in {}", "WARN".yellow().bold(), manifest_path.display());
        }
    }
    Ok(projects)
}

fn calculate_project_hash(path: &Path, context: &str) -> Result<String> {
    let mut hasher = Sha256::new();
    hasher.update(context.as_bytes());

    for entry in WalkDir::new(path).into_iter() {
        let entry = entry?;
        if !entry.file_type().is_file()
            || entry.path().components().any(|c| matches!(c.as_os_str().to_str(), Some("target" | ".git" | ".logs")))
        {
            continue;
        }
        hasher.update(entry.path().strip_prefix(path)?.to_string_lossy().as_bytes());
        let mut file = fs::File::open(entry.path())?;
        let mut buffer = [0u8; 8192];
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 { break; }
            hasher.update(&buffer[..count]);
        }
    }

    let lock = Path::new("Cargo.lock");
    if lock.is_file() {
        hasher.update(fs::read(lock)?);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn validate_hook(command: &str) -> Result<()> {
    if command.contains('\n') || command.contains('\r') {
        bail!("hook command contains a newline");
    }
    Ok(())
}

async fn run_hooks(
    commands: &[String],
    cwd: &Path,
    envs: &HashMap<String, String>,
    forge_vars: &HashMap<&str, String>,
) -> Result<()> {
    for command in commands {
        validate_hook(command)?;
        let mut process = if cfg!(windows) {
            let mut cmd = tokio::process::Command::new("cmd");
            cmd.args(["/C", command]);
            cmd
        } else {
            let mut cmd = tokio::process::Command::new("sh");
            cmd.args(["-c", command]);
            cmd
        };
        // Values are environment data, never concatenated into shell source.
        process.envs(envs);
        process.envs(forge_vars.iter().map(|(key, value)| (*key, value)));
        let status = process.current_dir(cwd).status().await?;
        if !status.success() {
            bail!("hook failed with exit code {:?}", status.code());
        }
    }
    Ok(())
}

async fn run_tool(name: &str, path: &str, tool: &str) -> Result<()> {
    let manifest = Path::new(path).join("Cargo.toml");
    let mut command = tokio::process::Command::new("cargo");
    command.arg(tool).arg("--manifest-path").arg(manifest);
    match tool {
        "clippy" => { command.args(["--", "-D", "warnings"]); }
        "test" => { command.arg("--quiet"); }
        _ => {}
    }
    let output = command.output().await?;
    if output.status.success() {
        println!("{} {}: {}", "OK".green().bold(), tool.to_uppercase(), name);
        Ok(())
    } else {
        eprintln!("{} {}: {}", "FAILED".red().bold(), tool.to_uppercase(), name);
        if !output.stdout.is_empty() { eprintln!("{}", String::from_utf8_lossy(&output.stdout)); }
        if !output.stderr.is_empty() { eprintln!("{}", String::from_utf8_lossy(&output.stderr)); }
        bail!("cargo {tool} failed for {name} (exit code {:?})", output.status.code())
    }
}

async fn run_build(
    name: &str,
    member_path: &str,
    release: bool,
    config: &ForgeConfig,
) -> Result<()> {
    let path = PathBuf::from(member_path);
    if !path.is_dir() {
        bail!("workspace member does not exist: {}", path.display());
    }

    let profile = if release { "release" } else { "debug" };
    let context = format!(
        "profile={profile};target={:?};features={:?};all_features={};path={}",
        config.build.target, config.build.features, config.build.all_features, path.display()
    );
    let fingerprint = calculate_project_hash(&path, &context)?;
    let mut key = Sha256::new();
    key.update(name.as_bytes());
    key.update(path.to_string_lossy().as_bytes());
    key.update(context.as_bytes());
    let cache_dir = Path::new("target").join(".hashes");
    fs::create_dir_all(&cache_dir)?;
    let cache_file = cache_dir.join(format!("{}.hash", hex::encode(key.finalize())));
    if fs::read_to_string(&cache_file).map(|v| v == fingerprint).unwrap_or(false) {
        println!("{} CACHED: {}", "OK".green().bold(), name);
        return Ok(());
    }

    let mut vars = HashMap::new();
    vars.insert("FORGE_PROJECT", name.to_string());
    vars.insert("FORGE_PROFILE", profile.to_string());
    vars.insert("FORGE_PATH", member_path.to_string());
    run_hooks(&config.prebuild.commands, &path, &config.env, &vars).await?;

    let mut command = tokio::process::Command::new("cargo");
    command.arg("build").arg("--manifest-path").arg(path.join("Cargo.toml"));
    if release { command.arg("--release"); }
    if let Some(target) = &config.build.target { command.args(["--target", target]); }
    if config.build.all_features { command.arg("--all-features"); }
    if let Some(features) = &config.build.features {
        if !features.is_empty() { command.arg("--features").arg(features.join(",")); }
    }
    let output = command.output().await?;

    if config.build.log {
        let log_dir = Path::new(".logs");
        fs::create_dir_all(log_dir)?;
        let safe_name: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
        fs::write(log_dir.join(format!("{safe_name}.log")), [&output.stdout[..], &output.stderr[..]].concat())?;
    }

    if !output.status.success() {
        eprintln!("{} BUILD: {}", "FAILED".red().bold(), name);
        if !config.build.log {
            if !output.stdout.is_empty() { eprintln!("{}", String::from_utf8_lossy(&output.stdout)); }
            if !output.stderr.is_empty() { eprintln!("{}", String::from_utf8_lossy(&output.stderr)); }
        }
        bail!("cargo build failed for {name} (exit code {:?})", output.status.code());
    }

    if config.build.strip {
        let mut target_dir = std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("target"));
        if let Some(target) = &config.build.target { target_dir.push(target); }
        target_dir.push(profile);
        let executable = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
        let binary = target_dir.join(executable);
        if binary.is_file() {
            let status = StdCommand::new("strip").arg(&binary).status()
                .with_context(|| format!("failed to start strip for {}", binary.display()))?;
            if !status.success() { bail!("strip failed for {} (exit code {:?})", binary.display(), status.code()); }
        } else {
            eprintln!("{} No binary named '{}' found to strip; skipping", "WARN".yellow().bold(), name);
        }
    }

    run_hooks(&config.afterbuild.commands, &path, &config.env, &vars).await?;
    fs::write(cache_file, fingerprint)?;
    println!("{} BUILT: {}", "OK".green().bold(), name);
    Ok(())
}

async fn run_projects(
    projects: Vec<(String, String)>,
    action: Commands,
    config: Arc<ForgeConfig>,
) -> Result<()> {
    let semaphore = Arc::new(Semaphore::new(config.build.threads));
    let mut tasks = JoinSet::new();
    for (name, path) in projects {
        let semaphore = Arc::clone(&semaphore);
        let config = Arc::clone(&config);
        tasks.spawn(async move {
            let _permit = semaphore.acquire_owned().await.context("failed to acquire build slot")?;
            match action {
                Commands::Build { release } => run_build(&name, &path, release, &config).await,
                Commands::Fmt => run_tool(&name, &path, "fmt").await,
                Commands::Lint => run_tool(&name, &path, "clippy").await,
                Commands::Test => run_tool(&name, &path, "test").await,
                Commands::Clean => Ok(()),
            }
        });
    }

    let mut failures = Vec::new();
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => failures.push(format!("{error:#}")),
            Err(error) => failures.push(format!("task failed: {error}")),
        }
    }
    if !failures.is_empty() {
        bail!("{} task(s) failed:\n{}", failures.len(), failures.join("\n"));
    }
    Ok(())
}

fn clean_project() -> Result<()> {
    if !Path::new("Cargo.toml").is_file()
        && !Path::new("forge.toml").is_file()
        && !Path::new("Forge.toml").is_file()
    {
        bail!("refusing to clean: current directory is not a Cargo/Forge project root");
    }
    let root = std::env::current_dir()?.canonicalize()?;
    for name in ["target", ".logs"] {
        let path = root.join(name);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => bail!("refusing to remove symlink: {}", path.display()),
            Ok(metadata) if metadata.is_dir() => {
                let canonical = path.canonicalize()?;
                if !canonical.starts_with(&root) { bail!("refusing to remove path outside project root"); }
                fs::remove_dir_all(canonical)?;
                println!("{} Removed {name}/", "OK".green().bold());
            }
            Ok(_) => bail!("refusing to remove non-directory path: {}", path.display()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = Arc::new(load_config()?);
    if config.build.threads == 0 { bail!("build.threads must be greater than zero"); }

    if let Ok(repo) = Repository::open(".") {
        let mut options = git2::StatusOptions::new();
        if let Ok(statuses) = repo.statuses(Some(&mut options)) {
            if !statuses.is_empty() {
                println!("{} Uncommitted changes detected.", "WARN".yellow().bold());
            }
        }
    }

    if let Commands::Clean = cli.command {
        return clean_project();
    }

    let projects = load_projects(&config)?;
    if projects.is_empty() {
        bail!("no Cargo packages found; configure [workspace].members or run Forge from a package root");
    }
    run_projects(projects, cli.command, config).await
}
