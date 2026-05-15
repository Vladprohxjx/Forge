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
* [ ] **UPX Compression:** Automated compression for further binary size reduction.
* [ ] **Build Reports:** Exportable timing and size analysis in machine-readable formats.

### Installation
[Download a latest release](https://github.com/Vladprohxjx/Forge/releases/latest)

### Configuration (Forge.toml)
Create a Forge.toml file in your workspace root:
```toml
[workspace]
members = ["apps/api", "libs/core"]

[build]
threads = 4      # Number of parallel jobs (default: 2)
strip = true     # Strip symbols from binaries
log = true       # Save stdout/stderr to .logs/project.log
```

### Usage
* **Build:** forge build [--release]
* **Format:** forge fmt
* **Lint:** forge lint (runs clippy with -D warnings)
* **Clean:** forge clean (removes target/ and .logs/)
### License
This project is licensed under the GPLv3 License.
