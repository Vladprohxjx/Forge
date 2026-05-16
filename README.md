# Forge

A high-performance Cargo workflow orchestrator written in Rust. 

Forge extends the standard Rust build pipeline by automating post-build 
optimizations, binary analysis, and environment-specific tasks. It acts 
as a transparent layer over Cargo, providing developers with deeper 
insights into their builds and automated distribution workflows.

### Key Features 
* [x] **Workspace Orchestration:** Parallel execution of builds across multiple members.
* [x] **Smart Caching:** SHA-256 based file tracking to skip redundant builds.
* [x] **Isolated Logging:** Per-project log files in `.logs/` directory to prevent terminal noise.
* [x] **Post-processing:** Integrated binary stripping for production-ready artifacts.
* [x] **Git Integration:** Automatic detection of uncommitted changes before build.
* [x] **Unified CLI:** Built-in commands for `fmt`, `clippy` (lint), and `clean`.
* [x] **Build Tasks:** Exportable timing and size analysis in machine-readable formats.
* [ ] **UPX Compression:** Automated compression for further binary size reduction.
* [ ] **Self-Update:** Built-in command to update Forge to the latest version.

### Installation
[Download a latest release](https://github.com/Vladprohxjx/Forge/releases/latest)

### Configuration (Forge.toml)
Create a Forge.toml file in your workspace root:
```toml
[workspace]
members = ["apps/api", "libs/core"]

[afterbuild]
commands = ["echo 'Forge build finished!'"]

[prebuild]
commands = ["echo 'Forge build started!'"]

[build]
threads = 4      # Number of parallel jobs (default: 2)
strip = true     # Strip symbols from binaries
log = true       # Save stdout/stderr to .logs/project.log
```

### Environment Variables
Forge automatically injects context into your hooks. You can use these variables in any `prebuild` or `afterbuild` command:

- `$FORGE_PROJECT`: Name of the current package.
- `$FORGE_PROFILE`: Build profile (`debug` or `release`).
- `$FORGE_PATH`: Relative path to the project directory.

You can also define custom variables in the `[env]` section:
```toml
[env]
DEST_DIR = "./dist"
API_KEY = "secret_value"

[afterbuild]
commands = [
    "mkdir -p $DEST_DIR",
    "cp ./target/$FORGE_PROFILE/$FORGE_PROJECT $DEST_DIR/"
]
```

### Compilation Settings
The [build] section supports direct Cargo flags for cross-compilation and feature management:
```toml
[build]
# Target triple (e.g., x86_64-unknown-linux-gnu)
target = "x86_64-pc-windows-msvc"

# Feature management
all_features = false
features = ["extra-logs", "serde_json"]

```

### Usage
* **Build:** forge build [--release]
* **Format:** forge fmt
* **Lint:** forge lint (runs clippy with -D warnings)
* **Clean:** forge clean (removes target/ and .logs/)
### License
This project is licensed under the GPLv3 License.
