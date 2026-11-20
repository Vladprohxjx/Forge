# Forge

A high-performance Cargo workflow orchestrator written in Rust.

Forge extends the standard Rust build pipeline by automating post-build optimizations, binary analysis, and environment-specific tasks. It acts as a transparent layer over Cargo, providing developers with deeper insights into their builds and automated distribution workflows.

## Key Features

- **Workspace Orchestration:** Parallel execution of builds across multiple workspace members with configurable thread limits.
- **Build Cache:** SHA-256 fingerprints include project inputs, build profile, target and feature settings.
- **Isolated Logging:** Per-project log files in `.logs/` directory to prevent terminal noise.
- **Post-processing:** Integrated binary stripping for production-ready artifacts.
- **Git Integration:** Automatic detection of uncommitted changes before build operations.
- **Unified CLI:** Built-in commands for `build`, `fmt`, `clippy` (lint), `test`, and `clean`.
- **Cross-compilation Support:** Configure target triples and features directly in the config file.
- **Hook System:** Pre-build and post-build hooks with environment variable injection.

## Installation

Download the latest release from the [releases page](https://github.com/Vladprohxjx/Forge/releases/latest).

Alternatively, build from source:

```bash
git clone https://github.com/Vladprohxjx/Forge.git
cd Forge
cargo build --release
```

The binary will be available at `target/release/forge`.

## Configuration (Forge.toml)

Create a `forge.toml` or `Forge.toml` file in your workspace root:

```toml
[workspace]
members = ["apps/api", "libs/core"]

[build]
threads = 4          # Number of parallel jobs (default: 2)
strip = true         # Strip symbols from binaries
log = true           # Save stdout/stderr to .logs/project.log
target = "x86_64-unknown-linux-gnu"  # Optional: cross-compilation target
all_features = false
features = ["serde_json", "tokio"]

[prebuild]
commands = [
    "echo 'Starting build for $FORGE_PROJECT'",
    "echo 'Profile: $FORGE_PROFILE'"
]

[afterbuild]
commands = [
    "echo 'Build completed for $FORGE_PROJECT'",
    "cp target/$FORGE_PROFILE/$FORGE_PROJECT ./dist/"
]

[env]
DEST_DIR = "./dist"
BUILD_VERSION = "1.0.0"
```

## Environment Variables

Forge automatically injects the following environment variables into prebuild and afterbuild hooks:

| Variable | Description |
|----------|-------------|
| `$FORGE_PROJECT` | Name of the current package |
| `$FORGE_PROFILE` | Build profile (`debug` or `release`) |
| `$FORGE_PATH` | Relative path to the project directory |

You can also define custom variables in the `[env]` section. These are passed to all hook commands.

## Build Configuration Options

The `[build]` section supports the following options:

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `threads` | integer | 2 | Number of parallel build jobs |
| `strip` | boolean | false | Strip symbols from compiled binaries |
| `log` | boolean | false | Save build output to `.logs/` directory |
| `target` | string | - | Target triple for cross-compilation |
| `all_features` | boolean | false | Enable all features for all packages |
| `features` | array | - | List of features to enable |

## Commands

### Build

Build all workspace members with smart caching:

```bash
forge build              # Debug build
forge build --release    # Release build
```

### Format

Format all workspace members using `cargo fmt`:

```bash
forge fmt
```

### Lint

Run clippy on all workspace members with strict warnings:

```bash
forge lint
```

### Test

Run tests for all workspace members:

```bash
forge test
```

### Clean

Remove `target/` and `.logs/` directories:

```bash
forge clean
```

## Hook System

Forge supports pre-build and post-build hooks that execute custom commands:

- **prebuild:** Commands executed before each project build
- **afterbuild:** Commands executed after successful project build

Hooks use native shell environment expansion. Forge passes configured `[env]` values and `FORGE_PROJECT`, `FORGE_PROFILE`, and `FORGE_PATH` through the process environment rather than inserting values into shell command text.

Example:

```toml
[prebuild]
commands = [
    "echo 'Building $FORGE_PROJECT in $FORGE_PROFILE mode'",
    "mkdir -p $DEST_DIR"
]

[afterbuild]
commands = [
    "cp target/$FORGE_PROFILE/$FORGE_PROJECT $DEST_DIR/",
    "strip $DEST_DIR/$FORGE_PROJECT"
]
```

## Smart Caching

Forge uses SHA-256 hashing to track file changes. Only relevant source files (`.rs`, `.toml`, `.md`, `.json`, `.yaml`, `.yml`) are included in the hash calculation. Files in `target/`, `.git/`, `.logs/`, and `Cargo.lock` are automatically excluded.

If no changes are detected, Forge skips the build and reports the project as "up to date".

## Error Handling

Forge provides detailed error messages for:

- Missing or invalid `Cargo.toml` files
- Failed build commands
- Hook execution failures
- File system errors

When `log = true` is enabled, full build output is saved to `.logs/<project>.log` for debugging.

## Security Considerations

- Hook commands are trusted shell scripts from your configuration and run with Forge's permissions.
- Variable values are passed through environment variables, not interpolated into shell source.
- Never place untrusted input directly in hook command strings.
- `forge clean` only removes project-root `target/` and `.logs/` directories and refuses symlinks.

## License

This project is licensed under the GPLv3 License.
