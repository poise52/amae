use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use crate::package::PackageJson;

#[derive(Debug, Clone, Default)]
pub struct WorkspacePackage {
    pub version: String,
    pub path: PathBuf,
    pub dependencies: BTreeMap<String, String>,
    pub dev_dependencies: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default)]
pub struct Workspace {
    pub members: HashMap<String, WorkspacePackage>,
}

impl Workspace {
    pub fn load(root: &Path) -> Self {
        let mut workspace = Workspace {
            members: HashMap::new(),
        };

        let mut patterns = Vec::new();

        let pnpm_workspace_path = root.join("pnpm-workspace.yaml");
        if pnpm_workspace_path.exists() {
            if let Ok(content) = fs::read_to_string(&pnpm_workspace_path) {
                patterns = parse_pnpm_workspace(&content);
            }
        }

        if patterns.is_empty() {
            patterns = parse_package_json_workspaces(root);
        }

        if !patterns.is_empty() {
            workspace.members = scan_workspace(root, &patterns);
        }

        workspace
    }
}

fn parse_pnpm_workspace(content: &str) -> Vec<String> {
    let mut patterns = Vec::new();
    let mut in_packages = false;
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with("packages:") {
            in_packages = true;
            continue;
        }
        if in_packages {
            if line.starts_with('-') {
                let pat = line[1..].trim().trim_matches('\'').trim_matches('"').trim();
                patterns.push(pat.to_string());
            } else if line.contains(':') && !line.starts_with('-') {
                in_packages = false;
            }
        }
    }
    patterns
}

fn parse_package_json_workspaces(root: &Path) -> Vec<String> {
    let path = root.join("package.json");
    if !path.exists() {
        return Vec::new();
    }
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let json: serde_json::Value = match serde_json::from_str(&content) {
        Ok(j) => j,
        Err(_) => return Vec::new(),
    };

    let mut patterns = Vec::new();
    if let Some(workspaces) = json.get("workspaces") {
        if let Some(arr) = workspaces.as_array() {
            for val in arr {
                if let Some(s) = val.as_str() {
                    patterns.push(s.to_string());
                }
            }
        } else if let Some(obj) = workspaces.as_object() {
            if let Some(packages) = obj.get("packages") {
                if let Some(arr) = packages.as_array() {
                    for val in arr {
                        if let Some(s) = val.as_str() {
                            patterns.push(s.to_string());
                        }
                    }
                }
            }
        }
    }
    patterns
}

fn scan_workspace(root: &Path, patterns: &[String]) -> HashMap<String, WorkspacePackage> {
    let mut members = HashMap::new();
    for pat in patterns {
        let clean_pat = pat.replace('\\', "/");
        let parts: Vec<&str> = clean_pat.split('/').collect();
        if parts.is_empty() {
            continue;
        }

        let mut base_dir = root.to_path_buf();
        let mut wildcard_idx = None;
        for (idx, part) in parts.iter().enumerate() {
            if part.contains('*') {
                wildcard_idx = Some(idx);
                break;
            } else {
                base_dir = base_dir.join(part);
            }
        }

        if let Some(idx) = wildcard_idx {
            let is_recursive = parts[idx] == "**";
            if base_dir.exists() && base_dir.is_dir() {
                scan_dir_recursive(&base_dir, is_recursive, 0, &mut members);
            }
        } else {
            if base_dir.exists() && base_dir.is_dir() {
                if let Ok(pkg_json) = PackageJson::read_from_dir(&base_dir) {
                    if let (Some(name), Some(version)) = (pkg_json.name, pkg_json.version) {
                        members.insert(name, WorkspacePackage {
                            version,
                            path: base_dir.clone(),
                            dependencies: pkg_json.dependencies,
                            dev_dependencies: pkg_json.dev_dependencies,
                        });
                    }
                }
            }
        }
    }
    members
}

fn scan_dir_recursive(dir: &Path, recursive: bool, depth: usize, members: &mut HashMap<String, WorkspacePackage>) {
    if depth > 4 {
        return;
    }
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries {
            if let Ok(entry) = entry {
                let path = entry.path();
                if path.is_dir() {
                    let pkg_json_path = path.join("package.json");
                    if pkg_json_path.exists() {
                        if let Ok(pkg_json) = PackageJson::read_from_dir(&path) {
                            if let (Some(name), Some(version)) = (pkg_json.name, pkg_json.version) {
                                members.insert(name, WorkspacePackage {
                                    version,
                                    path: path.clone(),
                                    dependencies: pkg_json.dependencies,
                                    dev_dependencies: pkg_json.dev_dependencies,
                                });
                            }
                        }
                    } else if recursive {
                        scan_dir_recursive(&path, recursive, depth + 1, members);
                    }
                }
            }
        }
    }
}
