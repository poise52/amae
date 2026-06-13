use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, RwLock};
use semver::{Version, VersionReq};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT};
use futures_util::future::BoxFuture;
use tokio::sync::Semaphore;

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct RegistryPackage {
    pub name: String,
    #[serde(rename = "dist-tags", default)]
    pub dist_tags: BTreeMap<String, String>,
    pub versions: BTreeMap<String, RegistryVersion>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct RegistryVersion {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    #[serde(default)]
    pub dev_dependencies: BTreeMap<String, String>,
    #[serde(default, rename = "optionalDependencies")]
    pub optional_dependencies: BTreeMap<String, String>,
    pub dist: RegistryDist,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct RegistryDist {
    pub tarball: String,
    pub shasum: String,
    pub integrity: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedPackage {
    pub name: String,
    pub version: String,
    pub tarball_url: String,
    pub shasum: String,
    pub dependencies: BTreeMap<String, String>,
}

pub struct Resolver {
    client: reqwest::Client,
    npmrc: Arc<crate::npmrc::Npmrc>,
    pub workspace: Arc<crate::workspace::Workspace>,
    metadata_cache: Arc<RwLock<HashMap<String, Arc<RegistryPackage>>>>,
    pub resolved_graph: Arc<RwLock<HashMap<String, ResolvedPackage>>>,
    /// Limits concurrent registry HTTP requests to avoid connection overload
    sem: Arc<Semaphore>,
}

impl Clone for Resolver {
    fn clone(&self) -> Self {
        Self {
            client: self.client.clone(),
            npmrc: self.npmrc.clone(),
            workspace: self.workspace.clone(),
            metadata_cache: self.metadata_cache.clone(),
            resolved_graph: self.resolved_graph.clone(),
            sem: self.sem.clone(),
        }
    }
}

impl Resolver {
    pub fn new(npmrc: Arc<crate::npmrc::Npmrc>, workspace: Arc<crate::workspace::Workspace>) -> Self {
        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.npm.install-v1+json; q=1.0, application/json; q=0.8"),
        );

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            client,
            npmrc,
            workspace,
            metadata_cache: Arc::new(RwLock::new(HashMap::new())),
            resolved_graph: Arc::new(RwLock::new(HashMap::new())),
            sem: Arc::new(Semaphore::new(16)),
        }
    }

    pub fn with_prepopulated_graph(
        npmrc: Arc<crate::npmrc::Npmrc>,
        workspace: Arc<crate::workspace::Workspace>,
        prepopulated: HashMap<String, ResolvedPackage>,
    ) -> Self {
        let mut resolver = Self::new(npmrc, workspace);
        resolver.resolved_graph = Arc::new(RwLock::new(prepopulated));
        resolver
    }

    fn check_resolved(&self, key: &str) -> bool {
        if let Ok(graph) = self.resolved_graph.read() {
            graph.contains_key(key)
        } else {
            false
        }
    }

    fn insert_resolved(&self, key: String, pkg: ResolvedPackage) {
        if let Ok(mut graph) = self.resolved_graph.write() {
            graph.insert(key, pkg);
        }
    }

    pub fn parse_alias(name: &str, range_str: &str) -> (String, String) {
        if range_str.starts_with("npm:") {
            let spec = &range_str[4..];
            if let Some(at_pos) = spec.rfind('@').filter(|&p| p > 0) {
                (spec[..at_pos].to_string(), spec[at_pos + 1..].to_string())
            } else {
                (spec.to_string(), "*".to_string())
            }
        } else {
            (name.to_string(), range_str.to_string())
        }
    }

    pub fn matches_range(version: &Version, range_str: &str, dist_tags: &BTreeMap<String, String>) -> bool {
        if let Some(target_ver) = dist_tags.get(range_str) {
            if let Ok(parsed_target) = Version::parse(target_ver) {
                return version == &parsed_target;
            }
        }

        let sub_ranges = range_str.split("||");
        for sub_range in sub_ranges {
            let trimmed = sub_range.trim();
            if trimmed.is_empty() {
                continue;
            }

            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            let mut normalized = String::new();
            
            let mut i = 0;
            while i < parts.len() {
                let part = parts[i];
                if (part == ">=" || part == ">" || part == "<=" || part == "<" || part == "=" || part == "^" || part == "~") && i + 1 < parts.len() {
                    if !normalized.is_empty() && !normalized.ends_with(',') {
                        normalized.push(',');
                    }
                    normalized.push_str(part);
                    normalized.push_str(parts[i+1]);
                    i += 2;
                } else {
                    if !normalized.is_empty() && !normalized.ends_with(',') {
                        normalized.push(',');
                    }
                    normalized.push_str(part);
                    i += 1;
                }
            }

            if let Ok(req) = VersionReq::parse(&normalized).or_else(|_| VersionReq::parse(trimmed)) {
                if req.matches(version) {
                    return true;
                }
            }
        }

        false
    }

    async fn fetch_package_metadata(
        client: reqwest::Client,
        npmrc: Arc<crate::npmrc::Npmrc>,
        metadata_cache: Arc<RwLock<HashMap<String, Arc<RegistryPackage>>>>,
        sem: Arc<Semaphore>,
        name: String,
    ) -> Result<Arc<RegistryPackage>, String> {
        if let Ok(cache) = metadata_cache.read() {
            if let Some(pkg) = cache.get(&name) {
                return Ok(pkg.clone());
            }
        }

        let url_encoded_name = name.replace('/', "%2f");
        let registry = &npmrc.registry;
        let url = format!("{}/{}", registry.trim_end_matches('/'), url_encoded_name);

        let _permit = sem.acquire().await.map_err(|e| format!("Semaphore error: {}", e))?;

        let mut last_err = String::new();
        for attempt in 0..5u32 {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(300 * 2u64.pow(attempt - 1))).await;
            }

            let mut req = client.get(&url);
            if let Some(token) = npmrc.get_token(&url) {
                req = req.header("Authorization", format!("Bearer {}", token));
            }

            let response = match req.send().await {
                Ok(r) => r,
                Err(e) => {
                    last_err = format!("Network error fetching {}: {}", name, e);
                    continue;
                }
            };

            if response.status() == 404 {
                return Err(format!("Package not found: {}", name));
            }

            let pkg: RegistryPackage = match response.json().await {
                Ok(p) => p,
                Err(e) => {
                    last_err = format!("Failed to parse metadata for {}: {}", name, e);
                    continue;
                }
            };

            let pkg_arc = Arc::new(pkg);
            if let Ok(mut cache) = metadata_cache.write() {
                cache.insert(name, pkg_arc.clone());
            }
            return Ok(pkg_arc);
        }

        Err(last_err)
    }

    pub fn resolve(self, name: String, range_str: String) -> BoxFuture<'static, Result<String, String>> {
        Box::pin(async move {
            let (real_name, real_range) = Self::parse_alias(&name, &range_str);

            if let Some(ws_pkg) = self.workspace.members.get(&real_name) {
                let mut matches = false;
                if real_range == "workspace:*" || real_range == "*" {
                    matches = true;
                } else if real_range.starts_with("workspace:") {
                    let actual_range = real_range.trim_start_matches("workspace:").trim();
                    if actual_range == "*" {
                        matches = true;
                    } else if let Ok(req) = VersionReq::parse(actual_range) {
                        if let Ok(ver) = Version::parse(&ws_pkg.version) {
                            matches = req.matches(&ver);
                        }
                    }
                } else if let Ok(req) = VersionReq::parse(&real_range) {
                    if let Ok(ver) = Version::parse(&ws_pkg.version) {
                        matches = req.matches(&ver);
                    }
                }

                if matches {
                    let key = format!("{}@{}", name, ws_pkg.version);
                    if self.check_resolved(&key) {
                        return Ok(ws_pkg.version.clone());
                    }

                    self.insert_resolved(
                        key.clone(),
                        ResolvedPackage {
                            name: real_name.clone(),
                            version: ws_pkg.version.clone(),
                            tarball_url: format!("workspace:{}", ws_pkg.path.display()),
                            shasum: String::new(),
                            dependencies: BTreeMap::new(),
                        },
                    );

                    let mut resolved_deps = BTreeMap::new();
                    let mut combined_deps = ws_pkg.dependencies.clone();
                    for (k, v) in &ws_pkg.dev_dependencies {
                        combined_deps.insert(k.clone(), v.clone());
                    }
                    
                    let mut futures = Vec::new();
                    for (dep_name, dep_range) in combined_deps.iter() {
                        let dep_name = dep_name.clone();
                        let dep_range = dep_range.clone();

                        if dep_range.starts_with("file:")
                            || dep_range.starts_with("link:")
                            || dep_range.starts_with("git+")
                            || dep_range.starts_with("git:")
                            || dep_range.starts_with("https:")
                            || dep_range.starts_with("http:")
                            || dep_range.starts_with('/')
                            || dep_range.starts_with('.')
                        {
                            continue;
                        }

                        let resolver_clone = self.clone();
                        futures.push(tokio::spawn(async move {
                            let resolved_ver = resolver_clone.resolve(dep_name.clone(), dep_range).await?;
                            Ok::<(String, String), String>((dep_name, resolved_ver))
                        }));
                    }

                    for handle in futures {
                        let (dep_name, resolved_ver) = handle.await
                            .map_err(|e| format!("Task join error: {}", e))??;
                        resolved_deps.insert(dep_name, resolved_ver);
                    }

                    if let Ok(mut graph) = self.resolved_graph.write() {
                        if let Some(pkg) = graph.get_mut(&key) {
                            pkg.dependencies = resolved_deps;
                        }
                    }

                    return Ok(ws_pkg.version.clone());
                }
            }

            let metadata = Self::fetch_package_metadata(
                self.client.clone(),
                self.npmrc.clone(),
                self.metadata_cache.clone(),
                self.sem.clone(),
                real_name.clone(),
            ).await?;

            let (version, deps, tarball_url, shasum) = {
                let mut matched_version: Option<(&String, &RegistryVersion)> = None;
                for (ver_str, ver_info) in metadata.versions.iter().rev() {
                    if let Ok(ver) = Version::parse(ver_str) {
                        if Self::matches_range(&ver, &real_range, &metadata.dist_tags) {
                            matched_version = Some((ver_str, ver_info));
                            break;
                        }
                    }
                }

                let (version_str, ver_info) = match matched_version {
                    Some(v) => v,
                    None => return Err(format!("No matching version found for {}@{}", real_name, real_range)),
                };

                let mut combined_deps = ver_info.dependencies.clone();
                for (k, v) in &ver_info.optional_dependencies {
                    combined_deps.insert(k.clone(), v.clone());
                }

                (
                    version_str.clone(),
                    combined_deps,
                    ver_info.dist.tarball.clone(),
                    ver_info.dist.shasum.clone(),
                )
            };

            let key = format!("{}@{}", name, version);

            if self.check_resolved(&key) {
                return Ok(version);
            }

            self.insert_resolved(
                key.clone(),
                ResolvedPackage {
                    name: real_name.clone(),
                    version: version.clone(),
                    tarball_url: tarball_url.clone(),
                    shasum: shasum.clone(),
                    dependencies: BTreeMap::new(),
                },
            );

            let mut resolved_deps = BTreeMap::new();

            let mut futures = Vec::new();
            for (dep_name, dep_range) in deps.iter() {
                let dep_name = dep_name.clone();
                let dep_range = dep_range.clone();

                if dep_range.starts_with("file:")
                    || dep_range.starts_with("link:")
                    || dep_range.starts_with("git+")
                    || dep_range.starts_with("git:")
                    || dep_range.starts_with("https:")
                    || dep_range.starts_with("http:")
                    || dep_range.starts_with('/')
                    || dep_range.starts_with('.')
                {
                    continue;
                }

                let resolver_clone = self.clone();
                futures.push(tokio::spawn(async move {
                    let resolved_ver = resolver_clone.resolve(dep_name.clone(), dep_range).await?;
                    Ok::<(String, String), String>((dep_name, resolved_ver))
                }));
            }

            for handle in futures {
                let (dep_name, resolved_ver) = handle.await
                    .map_err(|e| format!("Task join error: {}", e))??;
                resolved_deps.insert(dep_name, resolved_ver);
            }

            if let Ok(mut graph) = self.resolved_graph.write() {
                if let Some(pkg) = graph.get_mut(&key) {
                    pkg.dependencies = resolved_deps;
                }
            }

            Ok(version)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_alias() {
        assert_eq!(
            Resolver::parse_alias("react-is-18", "npm:react-is@^18.0.0"),
            ("react-is".to_string(), "^18.0.0".to_string())
        );
        assert_eq!(
            Resolver::parse_alias("react-is-18", "npm:react-is"),
            ("react-is".to_string(), "*".to_string())
        );
        assert_eq!(
            Resolver::parse_alias("react-is", "^18.0.0"),
            ("react-is".to_string(), "^18.0.0".to_string())
        );
    }
}
