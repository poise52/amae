use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, RwLock};
use semver::{Version, VersionReq};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT};
use futures_util::future::BoxFuture;

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
    metadata_cache: Arc<RwLock<HashMap<String, Arc<RegistryPackage>>>>,
    pub resolved_graph: Arc<RwLock<HashMap<String, ResolvedPackage>>>,
}

impl Clone for Resolver {
    fn clone(&self) -> Self {
        Self {
            client: self.client.clone(),
            npmrc: self.npmrc.clone(),
            metadata_cache: self.metadata_cache.clone(),
            resolved_graph: self.resolved_graph.clone(),
        }
    }
}

impl Resolver {
    pub fn new(npmrc: Arc<crate::npmrc::Npmrc>) -> Self {
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
            metadata_cache: Arc::new(RwLock::new(HashMap::new())),
            resolved_graph: Arc::new(RwLock::new(HashMap::new())),
        }
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

    fn matches_range(version: &Version, range_str: &str, dist_tags: &BTreeMap<String, String>) -> bool {
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

        let mut req = client.get(&url);
        if let Some(token) = npmrc.get_token(&url) {
            req = req.header("Authorization", format!("Bearer {}", token));
        }

        let response = req.send()
            .await
            .map_err(|e| format!("Network error fetching {}: {}", name, e))?;

        if response.status() == 404 {
            return Err(format!("Package not found: {}", name));
        }

        let pkg: RegistryPackage = response.json()
            .await
            .map_err(|e| format!("Failed to parse metadata for {}: {}", name, e))?;

        let pkg_arc = Arc::new(pkg);
        
        if let Ok(mut cache) = metadata_cache.write() {
            cache.insert(name, pkg_arc.clone());
        }

        Ok(pkg_arc)
    }

    pub fn resolve(self, name: String, range_str: String) -> BoxFuture<'static, Result<String, String>> {
        Box::pin(async move {
            let metadata = Self::fetch_package_metadata(
                self.client.clone(),
                self.npmrc.clone(),
                self.metadata_cache.clone(),
                name.clone(),
            ).await?;

            let (version, deps, tarball_url, shasum) = {
                let mut matched_version: Option<(&String, &RegistryVersion)> = None;
                for (ver_str, ver_info) in metadata.versions.iter().rev() {
                    if let Ok(ver) = Version::parse(ver_str) {
                        if Self::matches_range(&ver, &range_str, &metadata.dist_tags) {
                            matched_version = Some((ver_str, ver_info));
                            break;
                        }
                    }
                }

                let (version_str, ver_info) = match matched_version {
                    Some(v) => v,
                    None => return Err(format!("No matching version found for {}@{}", name, range_str)),
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

            let mut resolved_deps = BTreeMap::new();

            let mut futures = Vec::new();
            for (dep_name, dep_range) in deps.iter() {
                let dep_name = dep_name.clone();
                let dep_range = dep_range.clone();
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

            self.insert_resolved(
                key,
                ResolvedPackage {
                    name: name.clone(),
                    version: version.clone(),
                    tarball_url,
                    shasum,
                    dependencies: resolved_deps,
                },
            );

            Ok(version)
        })
    }
}
