# amae

> A fast JavaScript package manager written in Rust.

amae resolves, downloads, and links your dependencies the same way npm does — but stores every file exactly once on disk and uses hard links to wire packages into `node_modules`. No duplicates, no copies, just pointers.

---

## Why

npm copies files. amae doesn't.

Every package you install goes into a global content-addressable store at `~/.amae/store`. When the same file is needed in ten different projects, it lives on disk once and is hard-linked into each `node_modules`. Installs after the first are near-instant because there's nothing to download or unpack — only links to create.

The resolved dependency graph is serialized into a binary lockfile (`amae-lock.bin`) using [bincode](https://github.com/bincode-org/bincode). Reading it back is orders of magnitude faster than parsing JSON.

---

## Install

```sh
npm install -g amae-cli
```

Or with npx (no install needed):

```sh
npx amae-cli install
```

Prebuilt native binaries ship for:
- macOS arm64 (Apple Silicon)
- macOS x64 (Intel)
- Linux x64
- Windows x64

---

## Usage

```sh
amae --version                   # Show amae version
amae install                     # Install all dependencies from package.json
amae install --frozen-lockfile   # Fail if lockfile is out of sync or missing (CI)
amae install --production        # Skip devDependencies (production builds)
amae install --store-dir=./cache # Use a custom local store instead of ~/.amae/store
amae add axios                   # Add a package and install
amae add -D vitest               # Add a dev dependency
amae remove axios                # Remove a package and reinstall
amae update                      # Update all dependencies to their latest versions (semver)
amae update axios                # Update a specific package and its transitives
amae outdated                    # List dependencies that are out of date
amae why axios                   # Traces and prints why a package is installed
amae completions zsh             # Generate shell completion scripts (bash/zsh/fish...)
amae run build                   # Run a script from package.json
amae test                        # Run the "test" script
amae start                       # Run the "start" script
amae list                        # List installed packages with resolved versions
amae clean                       # Delete node_modules and lockfile
amae prune                       # Clear the global ~/.amae/store cache
```

---

## Workspaces

amae understands monorepos. It reads `"workspaces"` from the root `package.json` or `pnpm-workspace.yaml` and resolves local packages directly without touching the registry.

```
my-monorepo/
├── package.json            # { "workspaces": ["packages/*"] }
├── packages/
│   ├── math-utils/
│   │   └── package.json    # { "name": "math-utils", "version": "1.0.0" }
│   └── calc-app/
│       └── package.json    # { "dependencies": { "math-utils": "workspace:*" } }
```

```sh
amae install
# math-utils symlinked directly to packages/math-utils — no registry request
# external packages downloaded once, hard-linked everywhere
```

---

## How it works

```
amae install
  │
  ├─ 1. Read package.json (and all workspace packages if monorepo)
  │
  ├─ 2. Resolve — async semver resolution against npm registry
  │      Workspace packages are resolved locally, skipping the network
  │
  ├─ 3. Download — parallel .tgz fetching with SHA integrity check
  │      Each package extracted once into ~/.amae/store/<name>@<version>/
  │      Store is set read-only after extraction to prevent corruption
  │
  ├─ 4. Link — hard links from store into node_modules/.store/
  │      Symlinks from node_modules/<name> → .store/<name>@<version>/
  │      Binaries linked into node_modules/.bin/
  │
  └─ 5. Lifecycle — preinstall / install / postinstall scripts run
         in topological dependency order
```

The lockfile (`amae-lock.bin`) captures the full resolved graph. On subsequent installs amae reads the binary lockfile directly — no network, no resolution, just linking.

---

## .npmrc

amae reads both local `.npmrc` and `~/.npmrc`. Private registries, scoped registries, and auth tokens work out of the box:

```ini
registry=https://registry.npmjs.org/
//registry.npmjs.org/:_authToken=your_token_here

# Scoped registry for @mycompany packages
@mycompany:registry=https://npm.mycompany.com/
```

---

## Build from source

```sh
git clone https://github.com/poise52/amae
cd amae
cargo build --release
./target/release/amae install
```

Requires Rust 1.75+.

---

## License

MIT
