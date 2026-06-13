use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum BinConfig {
    Single(String),
    Multiple(BTreeMap<String, String>),
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct PackageJson {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dependencies: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dev_dependencies: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub scripts: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bin: Option<BinConfig>,
}


impl PackageJson {
    pub fn read_from_dir<P: AsRef<Path>>(dir: P) -> Result<Self, String> {
        let path = dir.as_ref().join("package.json");
        if !path.exists() {
            return Err("package.json not found".to_string());
        }
        let mut file = File::open(&path).map_err(|e| format!("Failed to open package.json: {}", e))?;
        let mut content = String::new();
        file.read_to_string(&mut content).map_err(|e| format!("Failed to read package.json: {}", e))?;
        serde_json::from_str(&content).map_err(|e| format!("Failed to parse package.json: {}", e))
    }

    pub fn write_to_dir<P: AsRef<Path>>(&self, dir: P) -> Result<(), String> {
        let path = dir.as_ref().join("package.json");
        let content = serde_json::to_string_pretty(self).map_err(|e| format!("Failed to serialize package.json: {}", e))?;
        let mut file = File::create(&path).map_err(|e| format!("Failed to create package.json: {}", e))?;
        file.write_all(content.as_bytes()).map_err(|e| format!("Failed to write package.json: {}", e))?;
        Ok(())
    }
}

pub fn is_skipped_specifier(v: &str) -> bool {
    v.starts_with("file:")
        || v.starts_with("link:")
        || v.starts_with("git+")
        || v.starts_with("git:")
        || v.starts_with("https:")
        || v.starts_with("http:")
        || v.starts_with('/')
        || v.starts_with('.')
}
