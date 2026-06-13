# Changelog

All notable changes to the `amae` package manager will be documented in this file.

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
