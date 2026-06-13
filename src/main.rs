mod cli;
mod package;
mod resolver;
mod cas;
mod linker;
mod lock;
mod npmrc;
mod workspace;

use clap::{Parser, CommandFactory};
use cli::{Cli, Commands};
use package::PackageJson;
use resolver::Resolver;
use linker::Linker;
use lock::Lockfile;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;
use indicatif::{ProgressBar, ProgressStyle};
use console::style;

#[tokio::main]
async fn main() {
    if let Err(e) = run_app().await {
        eprintln!("{}: {}", style("Error").red().bold(), e);
        std::process::exit(1);
    }
}

async fn run_app() -> Result<(), String> {
    let args = Cli::parse();
    let project_dir = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());

    match args.command {
        Commands::Init => handle_init(&project_dir),
        Commands::Install { frozen_lockfile, production, ignore_scripts, store_dir } => handle_install(&project_dir, frozen_lockfile, production, ignore_scripts, store_dir.as_deref()).await,
        Commands::Update { package } => handle_update(&project_dir, &package).await,
        Commands::Outdated => handle_outdated(&project_dir).await,
        Commands::Add { package, dev } => handle_add(&project_dir, &package, dev).await,
        Commands::Remove { package } => handle_remove(&project_dir, &package).await,
        Commands::Run { script } => handle_run(&project_dir, &script).await,
        Commands::Test => handle_run(&project_dir, "test").await,
        Commands::Start => handle_run(&project_dir, "start").await,
        Commands::Clean => handle_clean(&project_dir),
        Commands::List => handle_list(&project_dir),
        Commands::Prune => handle_prune(),
        Commands::Why { package } => handle_why(&project_dir, &package),
        Commands::Completions { shell } => handle_completions(shell),
    }
}

fn handle_init(project_dir: &Path) -> Result<(), String> {
    let pkg_path = project_dir.join("package.json");
    if pkg_path.exists() {
        return Err("package.json already exists in this directory".to_string());
    }

    let default_pkg = PackageJson {
        name: Some(project_dir.file_name().unwrap().to_string_lossy().to_string()),
        version: Some("1.0.0".to_string()),
        dependencies: BTreeMap::new(),
        dev_dependencies: BTreeMap::new(),
        scripts: BTreeMap::new(),
        bin: None,
    };

    default_pkg.write_to_dir(project_dir)?;
    println!("{}", style("Initialized package.json").green().bold());
    Ok(())
}

async fn handle_install(project_dir: &Path, frozen_lockfile: bool, production: bool, ignore_scripts: bool, store_dir: Option<&str>) -> Result<(), String> {
    let pkg = PackageJson::read_from_dir(project_dir)?;
    let lock_path = project_dir.join("amae-lock.bin");
    let npmrc = Arc::new(npmrc::Npmrc::load());
    let workspace = Arc::new(workspace::Workspace::load(project_dir));

    let is_skipped_specifier = |v: &str| {
        v.starts_with("file:")
            || v.starts_with("link:")
            || v.starts_with("git+")
            || v.starts_with("git:")
            || v.starts_with("https:")
            || v.starts_with("http:")
            || v.starts_with('/')
            || v.starts_with('.')
    };

    let mut direct_deps = BTreeMap::new();
    for (k, v) in pkg.dependencies.iter() {
        if !is_skipped_specifier(v) {
            direct_deps.insert(k.clone(), v.clone());
        }
    }
    if !production {
        for (k, v) in pkg.dev_dependencies.iter() {
            if !is_skipped_specifier(v) {
                direct_deps.insert(k.clone(), v.clone());
            }
        }
    }

    let mut all_direct_deps = BTreeMap::new();
    for (k, v) in &direct_deps {
        all_direct_deps.insert(k.clone(), v.clone());
    }
    for (_, ws_pkg) in &workspace.members {
        for (k, v) in &ws_pkg.dependencies {
            if !is_skipped_specifier(v) {
                all_direct_deps.insert(k.clone(), v.clone());
            }
        }
        if !production {
            for (k, v) in &ws_pkg.dev_dependencies {
                if !is_skipped_specifier(v) {
                    all_direct_deps.insert(k.clone(), v.clone());
                }
            }
        }
    }

    let mut resolved_packages: HashMap<String, resolver::ResolvedPackage>;

    if lock_path.exists() {
        println!("{}", style("Found lockfile. Reading dependencies...").cyan());
        let lockfile = Lockfile::read_from_file(&lock_path)?;
        
        let mut match_ok = true;
        for (k, v) in &all_direct_deps {
            if lockfile.direct_dependencies.get(k) != Some(v) {
                match_ok = false;
                break;
            }
        }

        if match_ok {
            resolved_packages = lockfile.packages.into_iter().collect();
        } else {
            if frozen_lockfile {
                return Err("amae-lock.bin is out of sync with package.json, but --frozen-lockfile was specified".to_string());
            }
            println!("{}", style("Lockfile out of date. Resolving dependencies...").yellow());
            resolved_packages = run_resolver(&all_direct_deps, npmrc.clone(), workspace.clone()).await?;
            let lockfile = Lockfile::new(all_direct_deps.clone(), resolved_packages.clone());
            lockfile.write_to_file(&lock_path)?;
        }
    } else {
        if frozen_lockfile {
            return Err("amae-lock.bin not found, but --frozen-lockfile was specified".to_string());
        }
        println!("{}", style("Resolving dependencies...").cyan().bold());
        resolved_packages = run_resolver(&all_direct_deps, npmrc.clone(), workspace.clone()).await?;
        let lockfile = Lockfile::new(all_direct_deps.clone(), resolved_packages.clone());
        lockfile.write_to_file(&lock_path)?;
    }

    let external_packages: Vec<&resolver::ResolvedPackage> = resolved_packages.values()
        .filter(|pkg| !pkg.tarball_url.starts_with("workspace:"))
        .collect();

    let pb = ProgressBar::new(external_packages.len() as u64);
    pb.set_style(
        ProgressStyle::with_template("{spinner:.green} Downloading [{bar:30.cyan/blue}] {pos}/{len} {msg}")
            .unwrap()
            .progress_chars("██░")
    );

    let cas = Arc::new(match store_dir {
        Some(dir) => cas::Cas::with_store_dir(std::path::PathBuf::from(dir)),
        None => cas::Cas::new(),
    });
    let client = Arc::new(reqwest::Client::new());
    let mut download_handles = Vec::new();

    for pkg in external_packages {
        let cas_clone = cas.clone();
        let client_clone = client.clone();
        let npmrc_clone = npmrc.clone();
        let name = pkg.name.clone();
        let version = pkg.version.clone();
        let tarball_url = pkg.tarball_url.clone();
        let shasum = pkg.shasum.clone();
        let pb_clone = pb.clone();
        let is_optional = pkg.is_optional;

        download_handles.push(tokio::spawn(async move {
            pb_clone.set_message(format!("{}@{}", name, version));
            let res = cas_clone.download_and_extract(&client_clone, &npmrc_clone, &name, &version, &tarball_url, &shasum).await;
            pb_clone.inc(1);
            (name, version, res, is_optional)
        }));
    }

    let mut failed_optional_packages = std::collections::HashSet::new();

    for handle in download_handles {
        let (name, version, res, is_optional) = handle.await.map_err(|e| format!("Download thread crashed: {}", e))?;
        if let Err(e) = res {
            if is_optional {
                eprintln!("Warning: Failed to download optional dependency {}@{}: {}. Skipping.", name, version, e);
                failed_optional_packages.insert(format!("{}@{}", name, version));
            } else {
                return Err(e);
            }
        }
    }
    pb.finish_and_clear();

    if !failed_optional_packages.is_empty() {
        resolved_packages.retain(|key, _| !failed_optional_packages.contains(key));
        for pkg in resolved_packages.values_mut() {
            pkg.dependencies.retain(|dep_name, dep_version| {
                let dep_key = format!("{}@{}", dep_name, dep_version);
                !failed_optional_packages.contains(&dep_key)
            });
        }
    }

    println!("{}", style("Linking dependencies...").cyan().bold());
    let linker = Linker::new(project_dir, workspace.clone(), store_dir.map(std::path::PathBuf::from));
    linker.prepare()?;

    let mut direct_resolved = Vec::new();
    for (name, _) in &direct_deps {
        let mut resolved_ver = None;
        for (key, resolved) in &resolved_packages {
            if key.starts_with(&format!("{}@", name)) {
                resolved_ver = Some(resolved.version.clone());
                break;
            }
        }
        if let Some(ver) = resolved_ver {
            direct_resolved.push((name.clone(), ver));
        } else {
            return Err(format!("Could not find resolved version for direct dependency {}", name));
        }
    }

    linker.link(&resolved_packages, &direct_resolved)?;
    if !ignore_scripts {
        linker.run_lifecycle_scripts(&resolved_packages, &direct_resolved)?;
    }
    println!("{}", style("Successfully installed dependencies.").green().bold());
    Ok(())
}

async fn handle_update(project_dir: &Path, package_to_update: &Option<String>) -> Result<(), String> {
    let pkg = PackageJson::read_from_dir(project_dir)?;
    let lock_path = project_dir.join("amae-lock.bin");
    let npmrc = Arc::new(npmrc::Npmrc::load());
    let workspace = Arc::new(workspace::Workspace::load(project_dir));

    let is_skipped_specifier = |v: &str| {
        v.starts_with("file:")
            || v.starts_with("link:")
            || v.starts_with("git+")
            || v.starts_with("git:")
            || v.starts_with("https:")
            || v.starts_with("http:")
            || v.starts_with('/')
            || v.starts_with('.')
    };

    let mut direct_deps = BTreeMap::new();
    for (k, v) in pkg.dependencies.iter() {
        if !is_skipped_specifier(v) {
            direct_deps.insert(k.clone(), v.clone());
        }
    }
    for (k, v) in pkg.dev_dependencies.iter() {
        if !is_skipped_specifier(v) {
            direct_deps.insert(k.clone(), v.clone());
        }
    }

    let mut all_direct_deps = BTreeMap::new();
    for (k, v) in &direct_deps {
        all_direct_deps.insert(k.clone(), v.clone());
    }
    for (_, ws_pkg) in &workspace.members {
        for (k, v) in &ws_pkg.dependencies {
            if !is_skipped_specifier(v) {
                all_direct_deps.insert(k.clone(), v.clone());
            }
        }
        for (k, v) in &ws_pkg.dev_dependencies {
            if !is_skipped_specifier(v) {
                all_direct_deps.insert(k.clone(), v.clone());
            }
        }
    }

    let mut resolved_packages: HashMap<String, resolver::ResolvedPackage>;

    match package_to_update {
        None => {
            println!("{}", style("Updating all dependencies...").cyan().bold());
            resolved_packages = run_resolver(&all_direct_deps, npmrc.clone(), workspace.clone()).await?;
            let lockfile = Lockfile::new(all_direct_deps.clone(), resolved_packages.clone());
            lockfile.write_to_file(&lock_path)?;
        }
        Some(pkg_name) => {
            if lock_path.exists() {
                println!("{}", style(format!("Updating package {} and its transitive dependencies...", pkg_name)).cyan().bold());
                let lockfile = Lockfile::read_from_file(&lock_path)?;
                let mut prepopulated: HashMap<String, resolver::ResolvedPackage> = lockfile.packages.into_iter().collect();

                let mut pkg_version = None;
                for key in prepopulated.keys() {
                    if key.starts_with(&format!("{}@", pkg_name)) {
                        if let Some(resolved) = prepopulated.get(key) {
                            pkg_version = Some(resolved.version.clone());
                            break;
                        }
                    }
                }

                if let Some(version) = pkg_version {
                    let mut to_remove = std::collections::HashSet::new();
                    let mut queue = std::collections::VecDeque::new();
                    let start_key = format!("{}@{}", pkg_name, version);

                    to_remove.insert(start_key.clone());
                    queue.push_back(start_key);

                    while let Some(current_key) = queue.pop_front() {
                        if let Some(resolved_pkg) = prepopulated.get(&current_key) {
                            for (dep_name, dep_version) in &resolved_pkg.dependencies {
                                let dep_key = format!("{}@{}", dep_name, dep_version);
                                if !to_remove.contains(&dep_key) {
                                    to_remove.insert(dep_key.clone());
                                    queue.push_back(dep_key);
                                }
                            }
                        }
                    }

                    for key in to_remove {
                        prepopulated.remove(&key);
                    }
                }

                let resolver = Resolver::with_prepopulated_graph(npmrc.clone(), workspace.clone(), prepopulated);
                let mut resolve_handles = Vec::new();

                for (name, range) in &all_direct_deps {
                    let resolver_clone = resolver.clone();
                    let name = name.clone();
                    let range = range.clone();
                    resolve_handles.push(tokio::spawn(async move {
                        resolver_clone.resolve(name, range, false).await
                    }));
                }

                for (ws_name, ws_pkg) in &workspace.members {
                    let resolver_clone = resolver.clone();
                    let name = ws_name.clone();
                    let range = format!("workspace:{}", ws_pkg.version);
                    resolve_handles.push(tokio::spawn(async move {
                        resolver_clone.resolve(name, range, false).await
                    }));
                }

                for handle in resolve_handles {
                    handle.await.map_err(|e| format!("Resolver thread crashed: {}", e))??;
                }

                resolved_packages = resolver.resolved_graph.read().map_err(|e| format!("Lock poisoned: {}", e))?.clone();
                let lockfile = Lockfile::new(all_direct_deps.clone(), resolved_packages.clone());
                lockfile.write_to_file(&lock_path)?;
            } else {
                println!("{}", style("No lockfile found. Resolving all dependencies...").yellow());
                resolved_packages = run_resolver(&all_direct_deps, npmrc.clone(), workspace.clone()).await?;
                let lockfile = Lockfile::new(all_direct_deps.clone(), resolved_packages.clone());
                lockfile.write_to_file(&lock_path)?;
            }
        }
    }

    let external_packages: Vec<&resolver::ResolvedPackage> = resolved_packages.values()
        .filter(|pkg| !pkg.tarball_url.starts_with("workspace:"))
        .collect();

    let pb = ProgressBar::new(external_packages.len() as u64);
    pb.set_style(
        ProgressStyle::with_template("{spinner:.green} Downloading [{bar:30.cyan/blue}] {pos}/{len} {msg}")
            .unwrap()
            .progress_chars("██░")
    );

    let cas = Arc::new(cas::Cas::new());
    let client = Arc::new(reqwest::Client::new());
    let mut download_handles = Vec::new();

    for pkg in external_packages {
        let cas_clone = cas.clone();
        let client_clone = client.clone();
        let npmrc_clone = npmrc.clone();
        let name = pkg.name.clone();
        let version = pkg.version.clone();
        let tarball_url = pkg.tarball_url.clone();
        let shasum = pkg.shasum.clone();
        let pb_clone = pb.clone();
        let is_optional = pkg.is_optional;

        download_handles.push(tokio::spawn(async move {
            pb_clone.set_message(format!("{}@{}", name, version));
            let res = cas_clone.download_and_extract(&client_clone, &npmrc_clone, &name, &version, &tarball_url, &shasum).await;
            pb_clone.inc(1);
            (name, version, res, is_optional)
        }));
    }

    let mut failed_optional_packages = std::collections::HashSet::new();

    for handle in download_handles {
        let (name, version, res, is_optional) = handle.await.map_err(|e| format!("Download thread crashed: {}", e))?;
        if let Err(e) = res {
            if is_optional {
                eprintln!("Warning: Failed to download optional dependency {}@{}: {}. Skipping.", name, version, e);
                failed_optional_packages.insert(format!("{}@{}", name, version));
            } else {
                return Err(e);
            }
        }
    }
    pb.finish_and_clear();

    if !failed_optional_packages.is_empty() {
        resolved_packages.retain(|key, _| !failed_optional_packages.contains(key));
        for pkg in resolved_packages.values_mut() {
            pkg.dependencies.retain(|dep_name, dep_version| {
                let dep_key = format!("{}@{}", dep_name, dep_version);
                !failed_optional_packages.contains(&dep_key)
            });
        }
    }

    println!("{}", style("Linking dependencies...").cyan().bold());
    let linker = Linker::new(project_dir, workspace.clone(), None);
    linker.prepare()?;

    let mut direct_resolved = Vec::new();
    for (name, _) in &direct_deps {
        let mut resolved_ver = None;
        for (key, resolved) in &resolved_packages {
            if key.starts_with(&format!("{}@", name)) {
                resolved_ver = Some(resolved.version.clone());
                break;
            }
        }
        if let Some(ver) = resolved_ver {
            direct_resolved.push((name.clone(), ver));
        } else {
            return Err(format!("Could not find resolved version for direct dependency {}", name));
        }
    }

    linker.link(&resolved_packages, &direct_resolved)?;
    linker.run_lifecycle_scripts(&resolved_packages, &direct_resolved)?;
    println!("{}", style("Successfully updated dependencies.").green().bold());
    Ok(())
}

async fn handle_outdated(project_dir: &Path) -> Result<(), String> {
    let pkg = PackageJson::read_from_dir(project_dir)?;
    let lock_path = project_dir.join("amae-lock.bin");
    if !lock_path.exists() {
        return Err("No lockfile found. Please run 'amae install' first.".to_string());
    }

    let lockfile = Lockfile::read_from_file(&lock_path)?;
    let resolved_packages: HashMap<String, resolver::ResolvedPackage> = lockfile.packages.into_iter().collect();

    let npmrc = Arc::new(npmrc::Npmrc::load());
    let workspace = Arc::new(workspace::Workspace::load(project_dir));

    let is_skipped_specifier = |v: &str| {
        v.starts_with("file:")
            || v.starts_with("link:")
            || v.starts_with("git+")
            || v.starts_with("git:")
            || v.starts_with("https:")
            || v.starts_with("http:")
            || v.starts_with('/')
            || v.starts_with('.')
    };

    let mut targets = Vec::new();
    for (name, range) in pkg.dependencies.iter().chain(pkg.dev_dependencies.iter()) {
        if is_skipped_specifier(range) {
            continue;
        }
        targets.push((name.clone(), range.clone()));
    }

    let mut packages_to_check = Vec::new();
    for (name, range) in targets {
        let (real_name, real_range) = Resolver::parse_alias(&name, &range);
        if workspace.members.contains_key(&real_name) {
            continue;
        }

        let mut current_version = None;
        for key in resolved_packages.keys() {
            if key.starts_with(&format!("{}@", name)) {
                if let Some(resolved) = resolved_packages.get(key) {
                    current_version = Some(resolved.version.clone());
                    break;
                }
            }
        }

        packages_to_check.push((name, real_name, real_range, current_version));
    }

    if packages_to_check.is_empty() {
        println!("No external dependencies to check.");
        return Ok(());
    }

    println!("{}", style("Checking for outdated dependencies...").cyan());

    let client = Arc::new(reqwest::Client::new());
    let mut handles = Vec::new();

    for (dep_name, real_name, real_range, current_version) in packages_to_check {
        let client_clone = client.clone();
        let npmrc_clone = npmrc.clone();

        handles.push(tokio::spawn(async move {
            let url_encoded_name = real_name.replace('/', "%2f");
            let registry = &npmrc_clone.registry;
            let url = format!("{}/{}", registry.trim_end_matches('/'), url_encoded_name);

            let mut response = None;
            for attempt in 0..3 {
                if attempt > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(500 * 2u64.pow(attempt - 1))).await;
                }

                let mut req = client_clone.get(&url)
                    .header("Accept", "application/vnd.npm.install-v1+json; q=1.0, application/json; q=0.8");
                if let Some(token) = npmrc_clone.get_token(&url) {
                    req = req.header("Authorization", format!("Bearer {}", token));
                }

                match req.send().await {
                    Ok(res) => {
                        response = Some(res);
                        break;
                    }
                    Err(_) => {}
                }
            }

            let res = match response {
                Some(r) => r,
                None => return Err(format!("Network error fetching metadata for {}", real_name)),
            };

            if res.status() == 404 {
                return Err(format!("Package not found: {}", real_name));
            }

            let metadata: resolver::RegistryPackage = res.json()
                .await
                .map_err(|e| format!("Failed to parse metadata for {}: {}", real_name, e))?;

            let latest = metadata.dist_tags.get("latest").cloned().unwrap_or_else(|| "unknown".to_string());

            let mut wanted = "unknown".to_string();
            for (ver_str, _) in metadata.versions.iter().rev() {
                if let Ok(ver) = semver::Version::parse(ver_str) {
                    if Resolver::matches_range(&ver, &real_range, &metadata.dist_tags) {
                        wanted = ver_str.clone();
                        break;
                    }
                }
            }

            Ok((dep_name, current_version.unwrap_or_else(|| "missing".to_string()), wanted, latest))
        }));
    }

    let mut outdated_packages = Vec::new();
    for handle in handles {
        match handle.await {
            Ok(Ok((name, current, wanted, latest))) => {
                if current != latest || current != wanted {
                    outdated_packages.push((name, current, wanted, latest));
                }
            }
            Ok(Err(e)) => {
                eprintln!("{}: {}", style("Warning").yellow().bold(), e);
            }
            Err(_) => {
                eprintln!("{}: task join error", style("Warning").yellow().bold());
            }
        }
    }

    if outdated_packages.is_empty() {
        println!("{}", style("All dependencies are up to date.").green().bold());
        return Ok(());
    }

    let mut name_width = 7;
    let mut current_width = 7;
    let mut wanted_width = 6;
    let mut latest_width = 6;

    for (name, current, wanted, latest) in &outdated_packages {
        name_width = name_width.max(name.len());
        current_width = current_width.max(current.len());
        wanted_width = wanted_width.max(wanted.len());
        latest_width = latest_width.max(latest.len());
    }

    let pkg_header = format!("{:<nw$}", "Package", nw = name_width);
    let current_header = format!("{:<cw$}", "Current", cw = current_width);
    let wanted_header = format!("{:<ww$}", "Wanted", ww = wanted_width);
    let latest_header = format!("{:<lw$}", "Latest", lw = latest_width);

    println!(
        "{}  {}  {}  {}",
        style(pkg_header).bold().underlined(),
        style(current_header).bold().underlined(),
        style(wanted_header).bold().underlined(),
        style(latest_header).bold().underlined()
    );

    for (name, current, wanted, latest) in outdated_packages {
        let is_red = if let (Ok(c), Ok(w)) = (semver::Version::parse(&current), semver::Version::parse(&wanted)) {
            c < w
        } else {
            current != wanted
        };

        let pkg_str = format!("{:<nw$}", name, nw = name_width);
        let current_str = format!("{:<cw$}", current, cw = current_width);
        let wanted_str = format!("{:<ww$}", wanted, ww = wanted_width);
        let latest_str = format!("{:<lw$}", latest, lw = latest_width);

        if is_red {
            println!(
                "{}  {}  {}  {}",
                style(pkg_str).red(),
                style(current_str).red(),
                style(wanted_str).green(),
                style(latest_str).magenta()
            );
        } else {
            println!(
                "{}  {}  {}  {}",
                style(pkg_str).yellow(),
                style(current_str).yellow(),
                style(wanted_str).yellow(),
                style(latest_str).magenta()
            );
        }
    }

    Ok(())
}

async fn run_resolver(
    direct_deps: &BTreeMap<String, String>,
    npmrc: Arc<npmrc::Npmrc>,
    workspace: Arc<workspace::Workspace>,
) -> Result<HashMap<String, resolver::ResolvedPackage>, String> {
    let resolver = Resolver::new(npmrc, workspace.clone());
    let mut resolve_handles = Vec::new();

    for (name, range) in direct_deps {
        let resolver_clone = resolver.clone();
        let name = name.clone();
        let range = range.clone();
        resolve_handles.push(tokio::spawn(async move {
            resolver_clone.resolve(name, range, false).await
        }));
    }

    for (ws_name, ws_pkg) in &workspace.members {
        let resolver_clone = resolver.clone();
        let name = ws_name.clone();
        let range = format!("workspace:{}", ws_pkg.version);
        resolve_handles.push(tokio::spawn(async move {
            resolver_clone.resolve(name, range, false).await
        }));
    }

    for handle in resolve_handles {
        handle.await.map_err(|e| format!("Resolver thread crashed: {}", e))??;
    }

    let graph = resolver.resolved_graph.read().map_err(|e| format!("Lock poisoned: {}", e))?.clone();
    Ok(graph)
}

async fn handle_add(project_dir: &Path, package_name: &str, dev: bool) -> Result<(), String> {
    let mut pkg = if PackageJson::read_from_dir(project_dir).is_err() {
        let default_pkg = PackageJson {
            name: Some(project_dir.file_name().unwrap().to_string_lossy().to_string()),
            version: Some("1.0.0".to_string()),
            dependencies: BTreeMap::new(),
            dev_dependencies: BTreeMap::new(),
            scripts: BTreeMap::new(),
            bin: None,
        };
        default_pkg.write_to_dir(project_dir)?;
        default_pkg
    } else {
        PackageJson::read_from_dir(project_dir)?
    };

    println!("{}", style(format!("Fetching package metadata for {}...", package_name)).cyan());
    let (name, range) = if package_name.contains('@') && !package_name.starts_with('@') {
        let parts: Vec<&str> = package_name.split('@').collect();
        (parts[0].to_string(), parts[1].to_string())
    } else if package_name.starts_with('@') && package_name.matches('@').count() > 1 {
        let parts: Vec<&str> = package_name.split('@').collect();
        (format!("@{}", parts[1]), parts[2].to_string())
    } else {
        let url_encoded_name = package_name.replace('/', "%2f");
        let npmrc = npmrc::Npmrc::load();
        let registry = &npmrc.registry;
        let url = format!("{}/{}", registry.trim_end_matches('/'), url_encoded_name);
        let client = reqwest::Client::new();
        let mut req = client.get(&url)
            .header("Accept", "application/vnd.npm.install-v1+json; q=1.0, application/json; q=0.8");
        if let Some(token) = npmrc.get_token(&url) {
            req = req.header("Authorization", format!("Bearer {}", token));
        }
        let response = req.send()
            .await
            .map_err(|e| format!("Failed to connect to registry: {}", e))?;

        if response.status() == 404 {
            return Err(format!("Package not found: {}", package_name));
        }

        let pkg_meta: resolver::RegistryPackage = response.json()
            .await
            .map_err(|e| format!("Failed to parse metadata for {}: {}", package_name, e))?;

        let latest_version = pkg_meta.dist_tags.get("latest")
            .ok_or_else(|| format!("Could not determine latest version for {}", package_name))?;

        (package_name.to_string(), format!("^{}", latest_version))
    };

    println!("{}", style(format!("Adding {}@{} to package.json", name, range)).cyan());
    if dev {
        pkg.dev_dependencies.insert(name, range);
    } else {
        pkg.dependencies.insert(name, range);
    }

    pkg.write_to_dir(project_dir)?;
    handle_install(project_dir, false, false, false, None).await
}

async fn handle_remove(project_dir: &Path, package_name: &str) -> Result<(), String> {
    let mut pkg = PackageJson::read_from_dir(project_dir)?;
    
    let mut removed = false;
    if pkg.dependencies.remove(package_name).is_some() {
        removed = true;
    }
    if pkg.dev_dependencies.remove(package_name).is_some() {
        removed = true;
    }

    if !removed {
        return Err(format!("Package {} is not a dependency of this project", package_name));
    }

    pkg.write_to_dir(project_dir)?;
    println!("{}", style(format!("Removed {} from package.json", package_name)).green().bold());

    let lock_path = project_dir.join("amae-lock.bin");
    if lock_path.exists() {
        let _ = std::fs::remove_file(lock_path);
    }

    let node_modules_dir = project_dir.join("node_modules");
    if node_modules_dir.exists() {
        let _ = std::fs::remove_dir_all(node_modules_dir);
    }

    handle_install(project_dir, false, false, false, None).await
}

async fn handle_run(project_dir: &Path, script_name: &str) -> Result<(), String> {
    let pkg = PackageJson::read_from_dir(project_dir)?;
    let cmd_str = pkg.scripts.get(script_name)
        .ok_or_else(|| format!("Script '{}' not found in package.json", script_name))?;

    println!("> {}", style(cmd_str).dim());

    let local_bin = project_dir.join("node_modules").join(".bin");
    let mut path_val = std::env::var_os("PATH").unwrap_or_default();
    
    #[cfg(unix)]
    {
        let mut new_path = local_bin.into_os_string();
        if !path_val.is_empty() {
            new_path.push(":");
            new_path.push(path_val);
        }
        path_val = new_path;
    }
    #[cfg(windows)]
    {
        let mut new_path = local_bin.into_os_string();
        if !path_val.is_empty() {
            new_path.push(";");
            new_path.push(path_val);
        }
        path_val = new_path;
    }

    #[cfg(unix)]
    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd_str)
        .env("PATH", path_val)
        .current_dir(project_dir)
        .spawn()
        .map_err(|e| format!("Failed to start shell process: {}", e))?;

    #[cfg(windows)]
    let mut child = std::process::Command::new("cmd")
        .arg("/C")
        .arg(cmd_str)
        .env("PATH", path_val)
        .current_dir(project_dir)
        .spawn()
        .map_err(|e| format!("Failed to start shell process: {}", e))?;

    let status = child.wait().map_err(|e| format!("Failed to wait for process: {}", e))?;

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }

    Ok(())
}

fn handle_clean(project_dir: &Path) -> Result<(), String> {
    let node_modules = project_dir.join("node_modules");
    if node_modules.exists() {
        println!("{}", style("Cleaning node_modules...").cyan());
        std::fs::remove_dir_all(&node_modules).map_err(|e| format!("Failed to remove node_modules: {}", e))?;
    }
    let lock_path = project_dir.join("amae-lock.bin");
    if lock_path.exists() {
        println!("{}", style("Cleaning amae-lock.bin...").cyan());
        std::fs::remove_file(&lock_path).map_err(|e| format!("Failed to remove lockfile: {}", e))?;
    }
    println!("{}", style("Cleaned project directories successfully.").green().bold());
    Ok(())
}

fn handle_list(project_dir: &Path) -> Result<(), String> {
    let pkg = PackageJson::read_from_dir(project_dir)?;
    let name = pkg.name.unwrap_or_else(|| "unnamed".to_string());
    let version = pkg.version.unwrap_or_else(|| "0.0.0".to_string());
    println!("{}@{} {}", style(name).bold(), style(version).bold(), style(project_dir.display()).dim());

    let lock_path = project_dir.join("amae-lock.bin");
    let resolved_map = if lock_path.exists() {
        match Lockfile::read_from_file(&lock_path) {
            Ok(lock) => Some(lock.packages),
            Err(_) => None,
        }
    } else {
        None
    };

    let list_deps = |deps: &BTreeMap<String, String>, is_dev: bool| {
        for (dep_name, dep_range) in deps {
            let actual_ver = if let Some(ref map) = resolved_map {
                let key_prefix = format!("{}@", dep_name);
                let found = map.keys().find(|k| k.starts_with(&key_prefix));
                if let Some(key) = found {
                    map.get(key).map(|p| p.version.clone())
                } else {
                    None
                }
            } else {
                None
            };

            let dev_suffix = if is_dev {
                format!(" {}", style("[dev]").magenta())
            } else {
                "".to_string()
            };

            if let Some(ver) = actual_ver {
                println!(
                    "├── {}@{} (resolved to {}){}",
                    style(dep_name).cyan(),
                    dep_range,
                    style(ver).green(),
                    dev_suffix
                );
            } else {
                println!(
                    "├── {}@{}{}",
                    style(dep_name).cyan(),
                    dep_range,
                    dev_suffix
                );
            }
        }
    };

    list_deps(&pkg.dependencies, false);
    list_deps(&pkg.dev_dependencies, true);

    Ok(())
}

fn handle_prune() -> Result<(), String> {
    let cas = cas::Cas::new();
    println!("{}", style(format!("Pruning global store at {}...", cas.store_dir.display())).cyan());

    #[cfg(unix)]
    {
        let mut child = std::process::Command::new("chmod")
            .arg("-R")
            .arg("u+w")
            .arg(&cas.store_dir)
            .spawn()
            .map_err(|e| format!("Failed to chmod store: {}", e))?;
        let _ = child.wait();
    }

    if cas.store_dir.exists() {
        std::fs::remove_dir_all(&cas.store_dir)
            .map_err(|e| format!("Failed to delete global store: {}", e))?;
    }
    std::fs::create_dir_all(&cas.store_dir)
        .map_err(|e| format!("Failed to recreate global store: {}", e))?;

    println!("{}", style("Successfully pruned global CAS store.").green().bold());
    Ok(())
}

fn handle_completions(shell: clap_complete::Shell) -> Result<(), String> {
    let mut cmd = Cli::command();
    clap_complete::generate(shell, &mut cmd, "amae", &mut std::io::stdout());
    Ok(())
}

fn handle_why(project_dir: &Path, target_name: &str) -> Result<(), String> {
    let pkg = PackageJson::read_from_dir(project_dir)?;
    let workspace = workspace::Workspace::load(project_dir);
    let lock_path = project_dir.join("amae-lock.bin");
    if !lock_path.exists() {
        return Err("No lockfile found. Run 'amae install' first.".to_string());
    }
    let lockfile = Lockfile::read_from_file(&lock_path)?;

    let mut target_keys = Vec::new();
    for key in lockfile.packages.keys() {
        if key == target_name || key.starts_with(&format!("{}@", target_name)) {
            target_keys.push(key.clone());
        }
    }

    if target_keys.is_empty() {
        println!("Package '{}' is not installed in the project.", target_name);
        return Ok(());
    }

    let mut parent_map: HashMap<String, Vec<String>> = HashMap::new();

    for (pkg_key, pkg_info) in &lockfile.packages {
        for (dep_name, dep_version) in &pkg_info.dependencies {
            let dep_key = format!("{}@{}", dep_name, dep_version);
            let parents = parent_map.entry(dep_key).or_default();
            if !parents.contains(pkg_key) {
                parents.push(pkg_key.clone());
            }
        }
    }

    fn add_direct(
        deps: &BTreeMap<String, String>,
        parent_key: &str,
        lockfile: &Lockfile,
        parent_map: &mut HashMap<String, Vec<String>>,
    ) {
        for (dep_name, dep_range) in deps {
            for (pkg_key, pkg_info) in &lockfile.packages {
                if pkg_info.name == *dep_name {
                    let mut matches = false;
                    if dep_range.starts_with("workspace:") {
                        matches = true;
                    } else if let Ok(req) = semver::VersionReq::parse(dep_range) {
                        if let Ok(ver) = semver::Version::parse(&pkg_info.version) {
                            matches = req.matches(&ver);
                        }
                    } else {
                        matches = pkg_info.version == *dep_range;
                    }

                    if matches {
                        let parents = parent_map.entry(pkg_key.clone()).or_default();
                        let p_str = parent_key.to_string();
                        if !parents.contains(&p_str) {
                            parents.push(p_str);
                        }
                    }
                }
            }
        }
    }

    add_direct(&pkg.dependencies, "root", &lockfile, &mut parent_map);
    add_direct(&pkg.dev_dependencies, "root", &lockfile, &mut parent_map);

    for (ws_name, ws_pkg) in &workspace.members {
        let ws_key = format!("{}@{}", ws_name, ws_pkg.version);
        let parents = parent_map.entry(ws_key.clone()).or_default();
        let root_str = "root".to_string();
        if !parents.contains(&root_str) {
            parents.push(root_str);
        }
        add_direct(&ws_pkg.dependencies, &ws_key, &lockfile, &mut parent_map);
        add_direct(&ws_pkg.dev_dependencies, &ws_key, &lockfile, &mut parent_map);
    }

    let mut paths_found = 0;
    for target_key in &target_keys {
        let mut current_path = vec![target_key.clone()];
        let mut visited = std::collections::HashSet::new();
        visited.insert(target_key.clone());
        
        let mut paths = Vec::new();
        find_paths_backwards(target_key, &parent_map, &mut visited, &mut current_path, &mut paths);

        if !paths.is_empty() {
            println!("Paths to {}:", style(target_key).green().bold());
            for path in paths {
                let mut forward_path = path.clone();
                forward_path.reverse();
                
                for (i, node) in forward_path.iter().enumerate() {
                    if i > 0 {
                        print!(" {} ", style("➔").dim());
                    }
                    if node == "root" {
                        print!("{}", style("root").bold());
                    } else if node == target_key {
                        print!("{}", style(node).green().bold());
                    } else {
                        print!("{}", style(node).cyan());
                    }
                }
                println!();
                paths_found += 1;
            }
        }
    }

    if paths_found == 0 {
        println!("No dependency paths found to '{}'.", target_name);
    }

    Ok(())
}

fn find_paths_backwards(
    current: &str,
    parent_map: &HashMap<String, Vec<String>>,
    visited: &mut std::collections::HashSet<String>,
    current_path: &mut Vec<String>,
    all_paths: &mut Vec<Vec<String>>,
) {
    if current == "root" {
        all_paths.push(current_path.clone());
        return;
    }

    if let Some(parents) = parent_map.get(current) {
        for parent in parents {
            if parent == "root" {
                let mut path = current_path.clone();
                path.push("root".to_string());
                all_paths.push(path);
            } else if !visited.contains(parent) {
                visited.insert(parent.clone());
                current_path.push(parent.clone());
                
                find_paths_backwards(parent, parent_map, visited, current_path, all_paths);
                
                current_path.pop();
                visited.remove(parent);
            }
        }
    }
}

