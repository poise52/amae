mod cli;
mod package;
mod resolver;
mod cas;
mod linker;
mod lock;
mod npmrc;

use clap::Parser;
use cli::{Cli, Commands};
use package::PackageJson;
use resolver::Resolver;
use linker::Linker;
use lock::Lockfile;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let args = Cli::parse();
    let project_dir = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());

    match args.command {
        Commands::Init => {
            if let Err(e) = handle_init(&project_dir) {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Commands::Install => {
            if let Err(e) = handle_install(&project_dir).await {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Commands::Add { package, dev } => {
            if let Err(e) = handle_add(&project_dir, &package, dev).await {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Commands::Remove { package } => {
            if let Err(e) = handle_remove(&project_dir, &package).await {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Commands::Run { script } => {
            if let Err(e) = handle_run(&project_dir, &script).await {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Commands::Test => {
            if let Err(e) = handle_run(&project_dir, "test").await {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Commands::Start => {
            if let Err(e) = handle_run(&project_dir, "start").await {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Commands::Clean => {
            if let Err(e) = handle_clean(&project_dir) {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Commands::List => {
            if let Err(e) = handle_list(&project_dir) {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Commands::Prune => {
            if let Err(e) = handle_prune() {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
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
    println!("Initialized package.json");
    Ok(())
}

async fn handle_install(project_dir: &Path) -> Result<(), String> {
    let pkg = PackageJson::read_from_dir(project_dir)?;
    let lock_path = project_dir.join("amae-lock.bin");
    let npmrc = Arc::new(npmrc::Npmrc::load());

    let mut direct_deps = BTreeMap::new();
    for (k, v) in pkg.dependencies.iter() {
        direct_deps.insert(k.clone(), v.clone());
    }
    for (k, v) in pkg.dev_dependencies.iter() {
        direct_deps.insert(k.clone(), v.clone());
    }

    if direct_deps.is_empty() {
        println!("No dependencies found in package.json");
        return Ok(());
    }

    let resolved_packages: HashMap<String, resolver::ResolvedPackage>;

    if lock_path.exists() {
        println!("Found lockfile. Reading dependencies...");
        let lockfile = Lockfile::read_from_file(&lock_path)?;
        
        let mut match_ok = true;
        for (k, v) in &direct_deps {
            if lockfile.direct_dependencies.get(k) != Some(v) {
                match_ok = false;
                break;
            }
        }

        if match_ok {
            resolved_packages = lockfile.packages.into_iter().collect();
        } else {
            println!("Lockfile out of date. Resolving dependencies...");
            resolved_packages = run_resolver(&direct_deps, npmrc.clone()).await?;
            let lockfile = Lockfile::new(direct_deps.clone(), resolved_packages.clone());
            lockfile.write_to_file(&lock_path)?;
        }
    } else {
        println!("Resolving dependencies...");
        resolved_packages = run_resolver(&direct_deps, npmrc.clone()).await?;
        let lockfile = Lockfile::new(direct_deps.clone(), resolved_packages.clone());
        lockfile.write_to_file(&lock_path)?;
    }

    println!("Downloading {} packages...", resolved_packages.len());
    let cas = Arc::new(cas::Cas::new());
    let client = Arc::new(reqwest::Client::new());
    let mut download_handles = Vec::new();

    for pkg in resolved_packages.values() {
        let cas_clone = cas.clone();
        let client_clone = client.clone();
        let npmrc_clone = npmrc.clone();
        let name = pkg.name.clone();
        let version = pkg.version.clone();
        let tarball_url = pkg.tarball_url.clone();
        let shasum = pkg.shasum.clone();

        download_handles.push(tokio::spawn(async move {
            cas_clone.download_and_extract(&client_clone, &npmrc_clone, &name, &version, &tarball_url, &shasum).await
        }));
    }

    for handle in download_handles {
        handle.await.map_err(|e| format!("Download thread crashed: {}", e))??;
    }

    println!("Linking dependencies...");
    let linker = Linker::new(project_dir);
    linker.prepare()?;

    let mut direct_resolved = Vec::new();
    for (name, _) in &direct_deps {
        let mut resolved_ver = None;
        for (_, resolved) in &resolved_packages {
            if &resolved.name == name {
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
    println!("Successfully installed dependencies.");
    Ok(())
}

async fn run_resolver(
    direct_deps: &BTreeMap<String, String>,
    npmrc: Arc<npmrc::Npmrc>,
) -> Result<HashMap<String, resolver::ResolvedPackage>, String> {
    let resolver = Resolver::new(npmrc);
    let mut resolve_handles = Vec::new();

    for (name, range) in direct_deps {
        let resolver_clone = resolver.clone();
        let name = name.clone();
        let range = range.clone();
        resolve_handles.push(tokio::spawn(async move {
            resolver_clone.resolve(name, range).await
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

    println!("Fetching package metadata for {}...", package_name);
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

    println!("Adding {}@{} to package.json", name, range);
    if dev {
        pkg.dev_dependencies.insert(name, range);
    } else {
        pkg.dependencies.insert(name, range);
    }

    pkg.write_to_dir(project_dir)?;
    handle_install(project_dir).await
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
    println!("Removed {} from package.json", package_name);

    let lock_path = project_dir.join("amae-lock.bin");
    if lock_path.exists() {
        let _ = std::fs::remove_file(lock_path);
    }

    let node_modules_dir = project_dir.join("node_modules");
    if node_modules_dir.exists() {
        let _ = std::fs::remove_dir_all(node_modules_dir);
    }

    handle_install(project_dir).await
}

async fn handle_run(project_dir: &Path, script_name: &str) -> Result<(), String> {
    let pkg = PackageJson::read_from_dir(project_dir)?;
    let cmd_str = pkg.scripts.get(script_name)
        .ok_or_else(|| format!("Script '{}' not found in package.json", script_name))?;

    println!("> {}", cmd_str);

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
        println!("Cleaning node_modules...");
        std::fs::remove_dir_all(&node_modules).map_err(|e| format!("Failed to remove node_modules: {}", e))?;
    }
    let lock_path = project_dir.join("amae-lock.bin");
    if lock_path.exists() {
        println!("Cleaning amae-lock.bin...");
        std::fs::remove_file(&lock_path).map_err(|e| format!("Failed to remove lockfile: {}", e))?;
    }
    println!("Cleaned project directories successfully.");
    Ok(())
}

fn handle_list(project_dir: &Path) -> Result<(), String> {
    let pkg = PackageJson::read_from_dir(project_dir)?;
    let name = pkg.name.unwrap_or_else(|| "unnamed".to_string());
    let version = pkg.version.unwrap_or_else(|| "0.0.0".to_string());
    println!("{}@{} {}", name, version, project_dir.display());

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

            if let Some(ver) = actual_ver {
                println!("├── {}@{} (resolved to {}){}", dep_name, dep_range, ver, if is_dev { " [dev]" } else { "" });
            } else {
                println!("├── {}@{}{}", dep_name, dep_range, if is_dev { " [dev]" } else { "" });
            }
        }
    };

    list_deps(&pkg.dependencies, false);
    list_deps(&pkg.dev_dependencies, true);

    Ok(())
}

fn handle_prune() -> Result<(), String> {
    let cas = cas::Cas::new();
    println!("Pruning global store at {}...", cas.store_dir.display());

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

    println!("Successfully pruned global CAS store.");
    Ok(())
}

