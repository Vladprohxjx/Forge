# Forge Documentation

[![License: GPLv3](https://img.shields.io/badge/License-GPLv3-blue.svg)](https://www.gnu.org/licenses/gpl-3.0)
[![Rust](https://img.shields.io/badge/rust-1.70%2B-orange.svg)](https://www.rust-lang.org)

**Forge** is a high-performance Cargo workflow orchestrator that extends the standard Rust build pipeline with post-build optimizations, binary analysis, and environment-specific tasks.

## Table of Contents

- [Installation](#installation)
- [Quick Start](#quick-start)
- [Configuration](#configuration)
  - [Workspace Configuration](#workspace-configuration)
  - [Build Settings](#build-settings)
  - [Environment Variables](#environment-variables)
  - [Pre/Post Build Hooks](#prepost-build-hooks)
- [Command Line Interface](#command-line-interface)
- [Project Overrides](#project-overrides)
- [Use Cases](#use-cases)
  - [Cross-Compilation](#cross-compilation)
  - [Workspace Orchestration](#workspace-orchestration)
  - [Custom Deployments](#custom-deployments)
- [Troubleshooting](#troubleshooting)
- [Contributing](#contributing)

## Installation

### From Releases

Download the latest binary from [Releases](https://github.com/Vladprohxjx/Forge/releases/latest)

```bash
# Linux/macOS
Soon!
```
# Windows (add to PATH manually)

From Source

```bash
git clone https://github.com/Vladprohxjx/Forge
cd Forge
cargo build --release
cp target/release/forge /usr/local/bin/
```

Quick Start

1. Create a Forge.toml in your project root:

```toml
[project]
name ="Forge"

[build]
threads = 4
strip = true
```

1. Build your project:

```bash
forge build --release
```

1. Add post-build automation:

```toml
[afterbuild]
commands = [
    "cp target/release/myapp /usr/local/bin/",
    "chmod +x /usr/local/bin/myapp"
]
```

Configuration

Workspace Configuration

Define a multi-crate workspace:

```toml
[workspace]
members = [
    "crates/api",
    "crates/core",
    "apps/cli",
    "apps/server"
]

```

Key Fields:

Field Type Description
members string[] Paths to workspace members

Build Settings

Configure build behavior in the [build] section:

```toml
[build]
# Number of parallel compilation jobs (default: 2)
threads = 8

# Strip debug symbols from binaries
strip = true

# Save build logs to .logs/ directory
log = true

# Target triple for cross-compilation
target = "x86_64-unknown-linux-gnu"

# Feature management
all_features = false
features = ["serde"]
```

Build Configuration Matrix:

Setting Values Default Description
threads 1-32 2 Parallel job count
strip true/false false Remove symbols
log true/false false Enable logging
target Any target triple Host triple Cross-compilation
all_features true/false false Enable all Cargo features
features string[] [] Specific features to enable

Environment Variables

Define variables for use in hooks:

```toml
[env]
# System paths
DEPLOY_DIR = "/opt/myapp"
CONFIG_DIR = "/etc/myapp"

# Build parameters
STRIP_BINARIES = "true"
LOG_LEVEL = "info"

# API credentials (use with caution)
API_ENDPOINT = "https://api.example.com"
```

Access in Commands:

```toml
[afterbuild]
commands = [
    "mkdir -p $DEPLOY_DIR",
    "cp target/release/myapp $DEPLOY_DIR/",
    "echo 'Deployed to $DEPLOY_DIR'"
]
```

Pre/Post Build Hooks

Execute commands before or after build:

```toml
[prebuild]
commands = [
    "echo 'Starting build of $FORGE_PROJECT'",
    "cargo fmt -- --check",
    "cargo clippy -- -D warnings"
]

[afterbuild]
commands = [
    "echo 'Build completed for $FORGE_PROJECT'",
    "size target/release/$FORGE_PROJECT",
    "./scripts/post-process.sh"
]
```

Available Hook Variables:

Variable Description Example
$FORGE_PROJECT Current package name myapp
$FORGE_PROFILE Build profile debug/release
$FORGE_PATH Relative project path crates/api

Command Rules:

· Commands execute sequentially
· Non-zero exit codes abort the build
· Non-zero build, hook, formatting, lint and test exit codes are reported as failures
· Hook values are passed as environment variables rather than interpolated into shell source
· Both stdout and stderr are captured when log = true

Command Line Interface

Basic Commands

```bash
# Build
forge build                # Debug build
forge build --release      # Release build
forge build -p specific_crate  # Build specific member

# Code Quality
forge fmt                  # Format code
forge lint                 # Run clippy with -D warnings

# Maintenance
forge clean                # Remove target/ and .logs/
```

Project Overrides

Override workspace settings in child directories:

```toml
# apps/server/Forge.toml
[project]
name = "server"
version = "2.0.0"

[build]
strip = false      # Override workspace strip setting
threads = 2        # Override parallelism

[afterbuild]
commands = [
    "docker build -t myapp/server .",
    "docker push myapp/server"
]
```

Override Priority:

1. Child Forge.toml settings
2. Workspace Forge.toml settings
3. Forge defaults

Use Cases

Cross-Compilation

```toml
[build]
target = "x86_64-pc-windows-msvc"
strip = true

[afterbuild]
commands = [
    "cp target/x86_64-pc-windows-msvc/release/app.exe ./dist/"
]
```

Workspace Orchestration

```toml
[workspace]
members = ["libs/*", "apps/*"]

[build]
threads = 8

[afterbuild]
commands = [
    "echo 'Building workspace complete'",
    "ls -la target/release/"
]
```

Custom Deployment

```toml
[env]
DEPLOY_HOST = "deploy.example.com"
DEPLOY_PATH = "/var/www/app"

[afterbuild]
commands = [
    "tar -czf app.tar.gz -C target/release/ .",
    "scp app.tar.gz $DEPLOY_HOST:$DEPLOY_PATH/",
    "ssh $DEPLOY_HOST 'cd $DEPLOY_PATH && tar -xzf app.tar.gz'"
]
```

Troubleshooting

Common Issues

Issue: Commands not running

Solution: Check command exit codes and ensure they're executable:

```toml
[afterbuild]
commands = [
    "bash -c 'echo Running as bash'",  # Use shell explicitly
    "./scripts/deploy.sh"               # Ensure executable
]
```

Issue: Variables not expanding

Solution: Both `$VAR` and `${VAR}` are supported by Unix shells; choose the form appropriate to your shell.

```toml
# Both forms work with Unix shells
commands = ["cp $BUILD_DIR/app ./"]
commands = ["cp ${BUILD_DIR}/app ./"]
```

Issue: Slow parallel builds

Solution: Adjust thread count based on CPU cores:

```toml
[build]
threads = 4  # Or 2x CPU cores
```

Debugging

Enable verbose output:

```bash
RUST_LOG=debug forge build --release
```

Check build logs:

```bash
cat .logs/forge.log
```

Contributing

We welcome contributions!

Development Setup

```bash
git clone https://github.com/Vladprohxjx/Forge
cd Forge
cargo build
cargo test
```
---

License: GPLv3
