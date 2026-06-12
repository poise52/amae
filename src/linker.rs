use std::fs;
use std::path::{Path, PathBuf};
use std::collections::HashMap;
use crate::resolver::ResolvedPackage;

pub struct Linker {
    node_modules_dir: PathBuf,
    store_dir: PathBuf,
}

impl Linker {
    pub fn new<P: AsRef<Path>>(project_root: P) -> Self {
        let node_modules_dir = project_root.as_ref().join("node_modules");
        let store_dir = node_modules_dir.join(".store");
        Self {
            node_modules_dir,
            store_dir,
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
        let cas = crate::cas::Cas::new();
        for (_, pkg) in resolved_graph.iter() {
            let global_pkg_dir = cas.package_dir(&pkg.name, &pkg.version);
            let local_pkg_store_dir = self.local_package_store_dir(&pkg.name, &pkg.version);
            
            if !local_pkg_store_dir.exists() {
                fs::create_dir_all(&local_pkg_store_dir)
                    .map_err(|e| format!("Failed to create local package store dir: {}", e))?;
                
                self.link_dir_recursive(&global_pkg_dir, &local_pkg_store_dir)?;
            }
        }

        for (_, pkg) in resolved_graph.iter() {
            let local_pkg_store_dir = self.local_package_store_dir(&pkg.name, &pkg.version);
            let local_pkg_node_modules = local_pkg_store_dir.parent().unwrap();

            for (dep_name, dep_version) in pkg.dependencies.iter() {
                let escaped_dep = dep_name.replace('/', "+");
                let dep_symlink_path = local_pkg_node_modules.join(dep_name);
                
                if let Some(parent) = dep_symlink_path.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|e| format!("Failed to create parent directory for symlink: {}", e))?;
                }

                if dep_symlink_path.exists() || dep_symlink_path.is_symlink() {
                    let _ = fs::remove_file(&dep_symlink_path);
                    let _ = fs::remove_dir_all(&dep_symlink_path);
                }

                let relative_target = if dep_name.contains('/') {
                    PathBuf::from(format!(
                        "../../../{}@{}/node_modules/{}",
                        escaped_dep, dep_version, dep_name
                    ))
                } else {
                    PathBuf::from(format!(
                        "../../{}@{}/node_modules/{}",
                        escaped_dep, dep_version, dep_name
                    ))
                };

                create_symlink(&relative_target, &dep_symlink_path)
                    .map_err(|e| format!("Failed to create symlink for dependency {} -> {}: {}", dep_name, relative_target.display(), e))?;
            }
        }

        for (name, version) in direct_deps {
            let escaped_name = name.replace('/', "+");
            let symlink_path = self.node_modules_dir.join(name);

            if let Some(parent) = symlink_path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create parent for direct symlink: {}", e))?;
            }

            if symlink_path.exists() || symlink_path.is_symlink() {
                let _ = fs::remove_file(&symlink_path);
                let _ = fs::remove_dir_all(&symlink_path);
            }

            let relative_target = if name.contains('/') {
                PathBuf::from(format!(
                    "../.store/{}@{}/node_modules/{}",
                    escaped_name, version, name
                ))
            } else {
                PathBuf::from(format!(
                    ".store/{}@{}/node_modules/{}",
                    escaped_name, version, name
                ))
            };

            create_symlink(&relative_target, &symlink_path)
                .map_err(|e| format!("Failed to create direct symlink for {}: {}", name, e))?;
        }

        Ok(())
    }

    fn local_package_store_dir(&self, name: &str, version: &str) -> PathBuf {
        let escaped_name = name.replace('/', "+");
        self.store_dir
            .join(format!("{}@{}", escaped_name, version))
            .join("node_modules")
            .join(name)
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
