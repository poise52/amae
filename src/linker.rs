use std::fs;
use std::path::{Path, PathBuf};
use std::collections::{HashMap, BTreeMap};
use std::sync::Arc;
use crate::resolver::ResolvedPackage;
use console::style;

pub struct Linker {
    node_modules_dir: PathBuf,
    store_dir: PathBuf,
    workspace: Arc<crate::workspace::Workspace>,
    cas_store_dir: Option<PathBuf>,
}

impl Linker {
    pub fn new<P: AsRef<Path>>(
        project_root: P,
        workspace: Arc<crate::workspace::Workspace>,
        cas_store_dir: Option<PathBuf>,
    ) -> Self {
        let node_modules_dir = project_root.as_ref().join("node_modules");
        let store_dir = node_modules_dir.join(".store");
        Self {
            node_modules_dir,
            store_dir,
            workspace,
            cas_store_dir,
        }
    }

    pub fn prepare(&self) -> Result<(), String> {
        fs::create_dir_all(&self.node_modules_dir)
            .map_err(|e| format!("Failed to create node_modules: {}", e))?;
        fs::create_dir_all(&self.store_dir)
            .map_err(|e| format!("Failed to create node_modules/.store: {}", e))?;
        Ok(())
    }

    pub fn link(
        &self,
        resolved_graph: &HashMap<String, ResolvedPackage>,
        direct_deps: &[(String, String)],
    ) -> Result<(), String> {
        let cas = match &self.cas_store_dir {
            Some(dir) => crate::cas::Cas::with_store_dir(dir.clone()),
            None => crate::cas::Cas::new(),
        };
        
        use rayon::prelude::*;
        
        resolved_graph.values().collect::<Vec<_>>().par_iter().map(|pkg| {
            if pkg.tarball_url.starts_with("workspace:") {
                return Ok(());
            }
            let global_pkg_dir = cas.package_dir(&pkg.name, &pkg.version);
            let local_pkg_store_dir = self.local_package_store_dir(&pkg.name, &pkg.version);
            
            if !local_pkg_store_dir.exists() {
                fs::create_dir_all(&local_pkg_store_dir)
                    .map_err(|e| format!("Failed to create local package store dir: {}", e))?;
                
                self.link_dir_recursive(&global_pkg_dir, &local_pkg_store_dir)?;
            }
            Ok(())
        }).collect::<Result<Vec<()>, String>>()?;

        for (_, pkg) in resolved_graph.iter() {
            let local_pkg_node_modules = if pkg.tarball_url.starts_with("workspace:") {
                let ws_path = Path::new(&pkg.tarball_url["workspace:".len()..]);
                ws_path.join("node_modules")
            } else {
                self.local_pkg_node_modules_dir(&pkg.name, &pkg.version)
            };

            for (dep_name, dep_version) in pkg.dependencies.iter() {
                let dep_symlink_path = local_pkg_node_modules.join(dep_name);
                
                if let Some(parent) = dep_symlink_path.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|e| format!("Failed to create parent directory for symlink: {}", e))?;
                }

                if dep_symlink_path.exists() || dep_symlink_path.is_symlink() {
                    let _ = fs::remove_file(&dep_symlink_path);
                    let _ = fs::remove_dir_all(&dep_symlink_path);
                }

                let target_path = if let Some(dep_pkg) = resolved_graph.get(&format!("{}@{}", dep_name, dep_version)) {
                    if dep_pkg.tarball_url.starts_with("workspace:") {
                        PathBuf::from(&dep_pkg.tarball_url["workspace:".len()..])
                    } else {
                        self.local_package_store_dir(&dep_pkg.name, &dep_pkg.version)
                    }
                } else {
                    self.local_package_store_dir(dep_name, dep_version)
                };

                let relative_target = get_relative_path(dep_symlink_path.parent().unwrap(), &target_path)
                    .ok_or_else(|| format!("Could not compute relative path from {} to {}", dep_symlink_path.parent().unwrap().display(), target_path.display()))?;

                create_symlink(&relative_target, &dep_symlink_path)
                    .map_err(|e| format!("Failed to create symlink for dependency {} -> {}: {}", dep_name, relative_target.display(), e))?;
            }
        }

        for (name, version) in direct_deps {
            let symlink_path = self.node_modules_dir.join(name);

            if let Some(parent) = symlink_path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create parent for direct symlink: {}", e))?;
            }

            if symlink_path.exists() || symlink_path.is_symlink() {
                let _ = fs::remove_file(&symlink_path);
                let _ = fs::remove_dir_all(&symlink_path);
            }

            let target_path = if let Some(dep_pkg) = resolved_graph.get(&format!("{}@{}", name, version)) {
                if dep_pkg.tarball_url.starts_with("workspace:") {
                    PathBuf::from(&dep_pkg.tarball_url["workspace:".len()..])
                } else {
                    self.local_package_store_dir(&dep_pkg.name, &dep_pkg.version)
                }
            } else {
                self.local_package_store_dir(name, version)
            };

            let relative_target = get_relative_path(symlink_path.parent().unwrap(), &target_path)
                .ok_or_else(|| format!("Could not compute relative path from {} to {}", symlink_path.parent().unwrap().display(), target_path.display()))?;

            create_symlink(&relative_target, &symlink_path)
                .map_err(|e| format!("Failed to create direct symlink for {}: {}", name, e))?;
        }

        for (_, pkg) in resolved_graph.iter() {
            let local_pkg_node_modules = if pkg.tarball_url.starts_with("workspace:") {
                let ws_path = Path::new(&pkg.tarball_url["workspace:".len()..]);
                ws_path.join("node_modules")
            } else {
                self.local_pkg_node_modules_dir(&pkg.name, &pkg.version)
            };
            let deps_list: Vec<(String, String)> = pkg.dependencies.iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            self.link_binaries(&local_pkg_node_modules, &deps_list, resolved_graph)?;
        }

        self.link_binaries(&self.node_modules_dir, direct_deps, resolved_graph)?;

        Ok(())
    }

    fn link_binaries(
        &self,
        base_node_modules: &Path,
        dependencies: &[(String, String)],
        resolved_graph: &HashMap<String, ResolvedPackage>,
    ) -> Result<(), String> {
        let bin_dir = base_node_modules.join(".bin");
        
        for (dep_name, dep_version) in dependencies {
            let dep_store_dir = if let Some(dep_pkg) = resolved_graph.get(&format!("{}@{}", dep_name, dep_version)) {
                if dep_pkg.tarball_url.starts_with("workspace:") {
                    PathBuf::from(&dep_pkg.tarball_url["workspace:".len()..])
                } else {
                    self.local_package_store_dir(dep_name, dep_version)
                }
            } else {
                self.local_package_store_dir(dep_name, dep_version)
            };
            let pkg_json_path = dep_store_dir.join("package.json");
            
            if !pkg_json_path.exists() {
                continue;
            }

            let pkg_json = match crate::package::PackageJson::read_from_dir(&dep_store_dir) {
                Ok(json) => json,
                Err(_) => continue,
            };

            if let Some(bin_config) = pkg_json.bin {
                fs::create_dir_all(&bin_dir)
                    .map_err(|e| format!("Failed to create bin dir {}: {}", bin_dir.display(), e))?;

                let bins = match bin_config {
                    crate::package::BinConfig::Single(path) => {
                        let name_without_scope = dep_name.split('/').last().unwrap().to_string();
                        let mut map = BTreeMap::new();
                        map.insert(name_without_scope, path);
                        map
                    }
                    crate::package::BinConfig::Multiple(map) => map,
                };

                for (cmd_name, bin_rel_path) in bins {
                    let symlink_path = bin_dir.join(&cmd_name);
                    
                    if symlink_path.exists() || symlink_path.is_symlink() {
                        let _ = fs::remove_file(&symlink_path);
                    }

                    let target_path = dep_store_dir.join(&bin_rel_path);
                    let relative_target = get_relative_path(&bin_dir, &target_path)
                        .ok_or_else(|| format!("Could not compute relative path from {} to {}", bin_dir.display(), target_path.display()))?;

                    create_symlink(&relative_target, &symlink_path)
                        .map_err(|e| format!("Failed to create bin symlink for {}: {}", cmd_name, e))?;

                    let real_bin_path = dep_store_dir.join(&bin_rel_path);
                    if let Err(e) = make_executable(&real_bin_path) {
                        return Err(format!("Failed to make binary {} executable: {}", real_bin_path.display(), e));
                    }
                }
            }
        }

        Ok(())
    }

    fn local_pkg_node_modules_dir(&self, name: &str, version: &str) -> PathBuf {
        let escaped_name = name.replace('/', "+");
        self.store_dir
            .join(format!("{}@{}", escaped_name, version))
            .join("node_modules")
    }

    fn local_package_store_dir(&self, name: &str, version: &str) -> PathBuf {
        self.local_pkg_node_modules_dir(name, version).join(name)
    }

    fn link_dir_recursive(&self, src: &Path, dest: &Path) -> Result<(), String> {
        if !dest.exists() {
            fs::create_dir_all(dest)
                .map_err(|e| format!("Failed to create destination dir {}: {}", dest.display(), e))?;
        }

        for entry in fs::read_dir(src).map_err(|e| format!("Failed to read source dir: {}", e))? {
            let entry = entry.map_err(|e| format!("Failed to read entry: {}", e))?;
            let file_type = entry.file_type().map_err(|e| format!("Failed to get file type: {}", e))?;
            let src_path = entry.path();
            let dest_path = dest.join(entry.file_name());

            if file_type.is_dir() {
                self.link_dir_recursive(&src_path, &dest_path)?;
            } else {
                if let Err(_) = fs::hard_link(&src_path, &dest_path) {
                    fs::copy(&src_path, &dest_path)
                        .map_err(|e| format!("Failed to copy file from {} to {}: {}", src_path.display(), dest_path.display(), e))?;
                }
            }
        }
        Ok(())
    }

    fn run_package_lifecycle_scripts(&self, pkg: &ResolvedPackage) -> Result<(), String> {
        let pkg_store_dir = if pkg.tarball_url.starts_with("workspace:") {
            PathBuf::from(&pkg.tarball_url["workspace:".len()..])
        } else {
            self.local_package_store_dir(&pkg.name, &pkg.version)
        };
        let pkg_json = match crate::package::PackageJson::read_from_dir(&pkg_store_dir) {
            Ok(json) => json,
            Err(_) => return Ok(()),
        };

        let mut scripts_to_run = Vec::new();
        if let Some(pre) = pkg_json.scripts.get("preinstall") {
            scripts_to_run.push(("preinstall", pre.clone()));
        }
        if let Some(ins) = pkg_json.scripts.get("install") {
            scripts_to_run.push(("install", ins.clone()));
        } else if pkg_store_dir.join("binding.gyp").exists() {
            scripts_to_run.push(("install", "node-gyp rebuild".to_string()));
        }
        if let Some(post) = pkg_json.scripts.get("postinstall") {
            scripts_to_run.push(("postinstall", post.clone()));
        }

        if scripts_to_run.is_empty() {
            return Ok(());
        }

        println!("{}", style(format!("Running lifecycle scripts for {}@{}...", pkg.name, pkg.version)).cyan().bold());

        let pkg_bin_dir = if pkg.tarball_url.starts_with("workspace:") {
            pkg_store_dir.join("node_modules").join(".bin")
        } else {
            self.local_pkg_node_modules_dir(&pkg.name, &pkg.version).join(".bin")
        };
        let root_bin_dir = self.node_modules_dir.join(".bin");
        let path_val = std::env::var_os("PATH").unwrap_or_default();

        let mut path_list = Vec::new();
        if pkg_bin_dir.exists() {
            path_list.push(pkg_bin_dir);
        }
        if root_bin_dir.exists() {
            path_list.push(root_bin_dir);
        }
        if !path_val.is_empty() {
            path_list.extend(std::env::split_paths(&path_val));
        }
        let new_path = std::env::join_paths(path_list)
            .map_err(|e| format!("Failed to join PATH: {}", e))?;

        for (name, script) in scripts_to_run {
            println!("  > {} ({}): {}", style(&pkg.name).dim(), style(name).dim(), style(&script).dim());

            #[cfg(unix)]
            let mut child = std::process::Command::new("sh")
                .arg("-c")
                .arg(&script)
                .env("PATH", &new_path)
                .current_dir(&pkg_store_dir)
                .spawn()
                .map_err(|e| format!("Failed to run script '{}' for {}: {}", script, pkg.name, e))?;

            #[cfg(windows)]
            let mut child = std::process::Command::new("cmd")
                .arg("/C")
                .arg(&script)
                .env("PATH", &new_path)
                .current_dir(&pkg_store_dir)
                .spawn()
                .map_err(|e| format!("Failed to run script '{}' for {}: {}", script, pkg.name, e))?;

            let status = child.wait().map_err(|e| format!("Failed to wait for script '{}': {}", script, e))?;
            if !status.success() {
                if pkg.is_optional {
                    eprintln!("Warning: Optional dependency {}@{} lifecycle script failed: {}. Continuing.", pkg.name, pkg.version, script);
                } else {
                    return Err(format!("Lifecycle script '{}' failed for {}@{} with exit code {:?}", script, pkg.name, pkg.version, status.code()));
                }
            }
        }
        Ok(())
    }

    pub fn run_lifecycle_scripts(
        &self,
        resolved_graph: &HashMap<String, ResolvedPackage>,
        direct_deps: &[(String, String)],
    ) -> Result<(), String> {
        let order = self.get_build_order(resolved_graph, direct_deps);
        for key in order {
            if let Some(pkg) = resolved_graph.get(&key) {
                self.run_package_lifecycle_scripts(pkg)?;
            }
        }

        let root_dir = self.node_modules_dir.parent().unwrap_or(Path::new(".")).to_path_buf();
        let root_pkg_json = match crate::package::PackageJson::read_from_dir(&root_dir) {
            Ok(json) => json,
            Err(_) => return Ok(()),
        };

        let mut root_scripts = Vec::new();
        if let Some(pre) = root_pkg_json.scripts.get("preinstall") {
            root_scripts.push(("preinstall", pre.clone()));
        }
        if let Some(ins) = root_pkg_json.scripts.get("install") {
            root_scripts.push(("install", ins.clone()));
        }
        if let Some(post) = root_pkg_json.scripts.get("postinstall") {
            root_scripts.push(("postinstall", post.clone()));
        }

        if !root_scripts.is_empty() {
            println!("{}", style("Running root lifecycle scripts...").cyan().bold());
            let root_bin_dir = self.node_modules_dir.join(".bin");
            let path_val = std::env::var_os("PATH").unwrap_or_default();
            let mut path_list = Vec::new();
            if root_bin_dir.exists() {
                path_list.push(root_bin_dir);
            }
            if !path_val.is_empty() {
                path_list.extend(std::env::split_paths(&path_val));
            }
            let new_path = std::env::join_paths(path_list)
                .map_err(|e| format!("Failed to join PATH: {}", e))?;

            for (name, script) in root_scripts {
                println!("  > root ({}): {}", style(name).dim(), style(&script).dim());

                #[cfg(unix)]
                let mut child = std::process::Command::new("sh")
                    .arg("-c")
                    .arg(&script)
                    .env("PATH", &new_path)
                    .current_dir(&root_dir)
                    .spawn()
                    .map_err(|e| format!("Failed to run root script '{}': {}", script, e))?;

                #[cfg(windows)]
                let mut child = std::process::Command::new("cmd")
                    .arg("/C")
                    .arg(&script)
                    .env("PATH", &new_path)
                    .current_dir(&root_dir)
                    .spawn()
                    .map_err(|e| format!("Failed to run root script '{}': {}", script, e))?;

                let status = child.wait().map_err(|e| format!("Failed to wait for root script '{}': {}", script, e))?;
                if !status.success() {
                    return Err(format!("Root lifecycle script '{}' failed with exit status {:?}", script, status));
                }
            }
        }

        Ok(())
    }

    fn get_build_order(
        &self,
        resolved_graph: &HashMap<String, ResolvedPackage>,
        direct_deps: &[(String, String)],
    ) -> Vec<String> {
        let mut order = Vec::new();
        let mut visited = std::collections::HashSet::new();

        fn visit(
            key: &str,
            resolved_graph: &HashMap<String, ResolvedPackage>,
            visited: &mut std::collections::HashSet<String>,
            order: &mut Vec<String>,
        ) {
            if visited.contains(key) {
                return;
            }
            visited.insert(key.to_string());

            if let Some(pkg) = resolved_graph.get(key) {
                for (dep_name, dep_version) in &pkg.dependencies {
                    let dep_key = format!("{}@{}", dep_name, dep_version);
                    visit(&dep_key, resolved_graph, visited, order);
                }
            }
            order.push(key.to_string());
        }

        for (ws_name, ws_pkg) in self.workspace.members.iter() {
            let key = format!("{}@{}", ws_name, ws_pkg.version);
            visit(&key, resolved_graph, &mut visited, &mut order);
        }

        for (name, version) in direct_deps {
            let key = format!("{}@{}", name, version);
            visit(&key, resolved_graph, &mut visited, &mut order);
        }

        order
    }
}

fn get_relative_path(from: &Path, to: &Path) -> Option<PathBuf> {
    let from_components: Vec<_> = from.components().collect();
    let to_components: Vec<_> = to.components().collect();
    
    let mut common_prefix_len = 0;
    for (f, t) in from_components.iter().zip(to_components.iter()) {
        if f == t {
            common_prefix_len += 1;
        } else {
            break;
        }
    }
    
    let mut rel_path = PathBuf::new();
    for _ in common_prefix_len..from_components.len() {
        rel_path.push("..");
    }
    for comp in &to_components[common_prefix_len..] {
        rel_path.push(comp.as_os_str());
    }
    
    Some(rel_path)
}

fn create_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(windows)]
    {
        if std::os::windows::fs::symlink_dir(target, link).is_err() {
            std::os::windows::fs::symlink_file(target, link)
        } else {
            Ok(())
        }
    }
}

#[cfg(unix)]
fn make_executable<P: AsRef<Path>>(path: P) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = fs::metadata(&path)?;
    let mut perms = metadata.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms)
}

#[cfg(not(unix))]
fn make_executable<P: AsRef<Path>>(_path: P) -> std::io::Result<()> {
    Ok(())
}
