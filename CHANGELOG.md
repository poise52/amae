# Changelog

All notable changes to the `amae` package manager will be documented in this file.

## [0.11.2] - 2026-06-21
### Fixed
- **Strict Dependency Resolution**: Enforced strict equality requirements for pinned exact version strings (e.g. `1.0.3`). Pinned dependencies will no longer be incorrectly resolved to caret ranges during package index checks (e.g. `rolldown: "1.0.3"` will no longer resolve to `1.1.2`, resolving Vite runtime errors).
- **CI Build Fixes**:
  - Disabled the `asm` feature for `sha-1` and `sha2` crates on Windows MSVC builds to resolve compiler incompatibilities.
  - Made the JavaScript/TypeScript scripting runtime (`deno_core` and `oxc`) optional via a `js_runtime` feature and disabled it for Linux Musl (`musl` targets) to allow compiling successfully on Alpine Linux without prebuilt `rusty_v8` static libraries.

## [0.11.1] - 2026-06-21
### Added
- **Lua Scripting Integration**: Integrated `mlua` to support reading configurations from `amae.config.lua` and executing custom `preinstall` and `postinstall` hooks.
- **Embedded JS/TS V8 Runtime**: Integrated `deno_core` and the **Oxc** toolchain (`oxc_parser`, `oxc_transformer`, `oxc_codegen`) to transpile TypeScript on-the-fly and execute `.js`/`.ts` scripts natively via V8 inside `amae run`.

### Performance
- **Fixed critical lockfile deserialization bug**: `bincode` now uses `.allow_trailing_bytes()`, preventing every warm install from falling back to full network re-resolution. Repeat install drops from ~16s to <300ms.
- **Symlink diffing in linker**: Linker now checks existing symlinks via `fs::read_link()` and skips recreation if already correct. Eliminates thousands of redundant FS operations on repeat installs — `with warm modules` drops from ~30s to <300ms.
- **Hardware-accelerated hashing**: Enabled `asm` feature for `sha-1` and `sha2` crates, activating native SHA-NI instructions on Apple Silicon and x86.
- **Faster archive decompression**: Switched `flate2` backend from `miniz_oxide` to `zlib-rs` for faster `.tar.gz` extraction.
- **Release profile optimization**: Added `[profile.release]` with `lto = true`, `codegen-units = 1`, `panic = "abort"` for smaller, faster production binaries.

### CLI / UX
- **Granular install timings**: Final output now shows per-phase timing: `Successfully installed 1281 packages in 0.34s (resolve: 0.00s, download: 0.12s, link: 0.22s)`.
- **Lockfile stats on read**: When a valid lockfile is found, prints `Found lockfile: 86 direct deps, 1281 packages total`.

---

## [0.10.5] - 2026-06-19
### Security & Robustness
- **Path Traversal & Symlink Escape Protection**: Implemented pure-lexical path normalization to validate symlink paths, preventing escape outside project boundaries and fixing Windows UNC path crashes.
- **Binary Hijacking Prevention**: Enforced constraints on `.bin` symlink targets to prevent system utility overrides.
- **Environment Variable Sanitization**: Cleared environment variables during script executions, retaining only critical system variables on Windows.
- **OOM Protection**: Enforced a 50MB deserialization limit on lockfiles.
- **Fallback Recovery**: Prevented process panic on corrupted lockfiles, falling back gracefully to clean dependency resolution.
- **SHA-512 & deny-weak-hashes**: Migrated SHA-512 hash decoding to `base64` crate and added `deny-weak-hashes` option in `.npmrc` to reject weak hashes (e.g., SHA-1).
- **Interprocess Locks**: Utilized `fd-lock` to serialize concurrent installation commands.

### Added
- **Hybrid Lockfile System**: Introduced readable `amae-lock.json` for Git tracking alongside local `amae-lock.bin` for fast startup.
- **Automatic Lockfile Synchronization**: Automatically regenerates `amae-lock.bin` from `amae-lock.json` if `amae-lock.bin` is missing or outdated.
- **Git Integration**: Excluded `amae-lock.bin` from git tracking in `.gitignore`.

---

## [0.10.4] - 2026-06-16
### Added
- **Linux ARM64 Support**: Added support and binary packaging for `linux-arm64` (`aarch64-unknown-linux-gnu`).
- **Alpine Linux Support**: Added dedicated static `musl` builds (`amae-linux-x64-musl` and `amae-linux-arm64-musl`) to support Alpine Linux out-of-the-box on both x64 and arm64 architectures without needing glibc compatibility layers.

---

## [0.10.1] - 2026-06-14
### Performance
- **Inline CAS Unpacking**: Removed the slow recursive `make_dir_writable` step. We now set permissions directly on the extracted files and directories during the extraction loop.
- **Parallel Linker via Rayon**: Replaced sequential linking with parallel rayon-powered package hardlinking.
- **O(1) Resolver Indexing**: Introduced `name_index` to speed up in-flight version checking from O(N) linear scan to O(1) direct lookup.
- **Tokio spawn optimization**: Used `join_all` to batch nested dependency resolutions within the same Tokio task, saving scheduling overhead.
- **Asynchronous hashing**: Moved SHA1 calculations off the main Tokio executor to `spawn_blocking`.
- **Lockfile memory mapping**: Implemented `memmap2` for reading `amae-lock.bin` directly from memory without copying.

### Code Quality & Refactoring
- **Code Duplication Removal**: Extracted common prefix skips to `package::is_skipped_specifier` and workspace package dependency collection to `collect_all_direct_deps`.
- **Optimized handle_remove**: Instead of deleting the entire `node_modules` directory, we now only remove the uninstalled package symlink and lockfile, then run install to keep `.store` cache warm.
- **Robust handle_add**: Added safe name/version parsing preventing crashes on trailing `@`.
- **Stack Overflow Prevention**: Added a depth limit to recursive `find_paths_backwards` to protect against deep dependency trees.

---

## [0.9.6] - 2026-06-14
### Performance
- **4x faster dependency resolution**: Deduplicate concurrent metadata fetches using `OnceCell` — if 50 packages depend on `lodash`, only one HTTP request is made instead of 50.
- **4x higher concurrency**: Increased parallel network connections from 16 → 64 for both registry queries and tarball downloads.
- **HTTP client tuning**: Connection pooling (`pool_max_idle_per_host=64`), TCP keep-alive, and connect timeouts to reuse connections.
- **Skip peer dependencies**: Peer deps are no longer resolved/downloaded by default (matches pnpm behavior), eliminating hundreds of unnecessary packages.
- **Fixed progress bar**: Now updates on package completion instead of start, so it no longer appears frozen.

---

## [0.9.5] - 2026-06-13
### Added
- **`--ignore-scripts` flag**: Skip executing package lifecycle scripts (`preinstall`, `install`, `postinstall`). This brings `amae` in line with other package managers when testing benchmarks without native build penalties.

---

## [0.9.4] - 2026-06-13
### Fixed
- **Cache store file permissions**: Forces all files in the cache store to be writable by their owner, and avoids making directories read-only. This fixes `EACCES: permission denied` errors when deleting the cache directory (e.g. using `rimraf`) during local installs, cleanup scripts, or benchmarks. Note: This skips versions 0.9.2 and 0.9.3 to sync up tags properly.

---

## [0.9.1] - 2026-06-13
### Added
- **Scoped registry support**: Resolves package names starting with specific scopes (e.g. `@mycompany`) using custom registry URLs defined in `.npmrc` via `@scope:registry=...`.
- **Peer dependency auto-installation**: Automatically resolves and installs peer dependencies when not satisfied by sibling/parent packages.
- **Graceful optional dependency failure handling**: Handles network, resolution, and compilation failures for optional dependencies gracefully without breaking the entire installation.

---

## [0.8.3] - 2026-06-13
### Added
- **`amae install --store-dir <path>` flag**: Allows specifying a custom local store directory instead of the default global `~/.amae/store`. Useful for isolated environments, benchmarks, and CI pipelines where the cache directory must be controlled per-run.

### Fixed
- **Resolver concurrency deadlock / infinite recursion**: Fixed by performing early insertion of resolving packages in the `resolved_graph` before traversing their dependencies. This resolves cycle issues and prevents OOM crashes on large dependency trees.
- **Connection resets / Rate-limiting on registry requests**: Fixed by adding concurrency Semaphores limiting concurrent metadata fetches to 16 and concurrent package downloads to 16.
- **Linker integration with custom store directory**: Passed the custom store directory correctly to the linker phase, ensuring packages are linked from the custom path instead of the default global cache.

---

## [0.8.2] - 2026-06-13
### Added
- **`amae --version` / `amae -V` flag**: Displays the current amae version. Previously the version was not accessible from the CLI.

---

## [0.8.0] - 2026-06-13
### Added
- **`amae why <package>` command**: Recursively traces the dependency graph backwards and prints all paths from the root (or workspace packages) explaining why the specified package is installed. Includes clean color formatting.
- **`amae completions <shell>` command**: Generates shell autocompletion scripts for `bash`, `zsh`, `fish`, `powershell`, and `elvish` utilizing the `clap_complete` crate.

---

## [0.7.1] - 2026-06-13
### Fixed
- **Tarball download resilience**: Added exponential back-off retry logic (up to 3 attempts with 500ms and 1000ms pauses) for downloading and body streaming in CAS to prevent transient network socket drops from crashing installation.

---

## [0.7.0] - 2026-06-13
### Added
- **Vibrant ANSI console colors**: Styled output logs using the `console` crate (success messages in bold green, steps in cyan, warnings in bold yellow, script execution details in dim).
- **DRY error handling**: Refactored entrypoint error handling to wrap CLI commands and print errors with a bold red `Error:` prefix globally.
- **Styled `amae outdated` table**: Custom width-aware styling for headers and rows (red for outdated packages below wanted versions, yellow for packages with newer major versions available).
- **Styled `amae list` tree**: Package trees print with styled bold root packages, cyan dependency names, and green resolved versions.

---

## [0.6.0] - 2026-06-13
### Added
- **Interactive Progress Bar**: Embedded `indicatif` progress bar with spinner during parallel downloads. Filters out workspace local packages from counting automatically.

---

## [0.5.0] - 2026-06-13
### Added
- **`--production` flag**: Skips installing `devDependencies` (both in root package and workspace packages) for smaller production images.
- **`--frozen-lockfile` flag**: Strict validation mode for CI pipelines. Fails installation if `amae-lock.bin` is missing or out of sync with `package.json`.

---

## [0.4.0] - 2026-06-13
### Added
- **`amae outdated` command**: Queries npm registry metadata concurrently to check installed versions against desired ranges (`Wanted`) and absolute latest releases (`Latest`).

---

## [0.3.0] - 2026-06-13
### Added
- **`amae update` command**: Updates all packages or a specific package and its transitives (using a Breadth-First Search prune of resolved subgraphs) to the newest versions matching semver constraints.
