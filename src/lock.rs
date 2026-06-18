use serde::{Serialize, Deserialize};
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use crate::resolver::ResolvedPackage;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Lockfile {
    pub version: u32,
    pub direct_dependencies: BTreeMap<String, String>,
    pub packages: BTreeMap<String, ResolvedPackage>,
}

impl Lockfile {
    pub fn new(direct_dependencies: BTreeMap<String, String>, packages: HashMap<String, ResolvedPackage>) -> Self {
        let sorted_packages = packages.into_iter().collect::<BTreeMap<_, _>>();
        Self {
            version: 1,
            direct_dependencies,
            packages: sorted_packages,
        }
    }

    pub fn read_from_file<P: AsRef<Path>>(path: P) -> Result<Self, String> {
        let file = File::open(path).map_err(|e| format!("Failed to open lockfile: {}", e))?;
        let mmap = unsafe { memmap2::Mmap::map(&file).map_err(|e| format!("Failed to mmap lockfile: {}", e))? };
        use bincode::Options;
        bincode::options()
            .with_limit(50 * 1024 * 1024)
            .deserialize(&mmap)
            .map_err(|e| format!("Failed to deserialize lockfile: {}", e))
    }

    pub fn write_to_file<P: AsRef<Path>>(&self, path: P) -> Result<(), String> {
        let file = File::create(path).map_err(|e| format!("Failed to create lockfile: {}", e))?;
        let mut writer = BufWriter::new(file);
        use bincode::Options;
        bincode::options()
            .with_limit(50 * 1024 * 1024)
            .serialize_into(&mut writer, self)
            .map_err(|e| format!("Failed to serialize lockfile: {}", e))?;
        use std::io::Write;
        writer.flush().map_err(|e| format!("Failed to flush lockfile: {}", e))
    }

    pub fn read_from_json<P: AsRef<Path>>(path: P) -> Result<Self, String> {
        let metadata = std::fs::metadata(&path).map_err(|e| format!("Failed to get JSON lockfile metadata: {}", e))?;
        if metadata.len() > 100 * 1024 * 1024 { // 100MB limit
            return Err("JSON lockfile exceeds size limit of 100MB".to_string());
        }
        let file = File::open(path).map_err(|e| format!("Failed to open JSON lockfile: {}", e))?;
        serde_json::from_reader(file).map_err(|e| format!("Failed to parse JSON lockfile: {}", e))
    }

    pub fn write_to_json<P: AsRef<Path>>(&self, path: P) -> Result<(), String> {
        let file = File::create(path).map_err(|e| format!("Failed to create JSON lockfile: {}", e))?;
        let mut writer = BufWriter::new(file);
        serde_json::to_writer_pretty(&mut writer, self).map_err(|e| format!("Failed to serialize JSON lockfile: {}", e))?;
        use std::io::Write;
        writer.flush().map_err(|e| format!("Failed to flush JSON lockfile: {}", e))
    }
}

#[cfg(test)]
mod test_hybrid {
    use super::*;

    #[test]
    fn test_sync_loop() {
        let mut direct_dependencies = BTreeMap::new();
        direct_dependencies.insert("is-even".to_string(), "^1.0.0".to_string());
        direct_dependencies.insert("is-odd".to_string(), "^1.0.0".to_string());

        let mut packages = HashMap::new();
        packages.insert(
            "is-even@1.0.0".to_string(),
            ResolvedPackage {
                name: "is-even".to_string(),
                version: "1.0.0".to_string(),
                tarball_url: "http://example.com/is-even.tgz".to_string(),
                shasum: "12345678".to_string(),
                integrity: None,
                dependencies: BTreeMap::new(),
                is_optional: false,
            },
        );
        packages.insert(
            "is-odd@1.0.0".to_string(),
            ResolvedPackage {
                name: "is-odd".to_string(),
                version: "1.0.0".to_string(),
                tarball_url: "http://example.com/is-odd.tgz".to_string(),
                shasum: "abcdef".to_string(),
                integrity: Some("sha512-abc".to_string()),
                dependencies: BTreeMap::new(),
                is_optional: true,
            },
        );

        let lockfile = Lockfile::new(direct_dependencies, packages);
        
        let temp_bin = "scratch/test-workspace/amae-lock-temp.bin";
        let temp_json = "scratch/test-workspace/amae-lock-temp.json";

        lockfile.write_to_file(temp_bin).unwrap();
        let loaded_bin = Lockfile::read_from_file(temp_bin).unwrap();
        assert_eq!(loaded_bin.version, lockfile.version);
        assert_eq!(loaded_bin.direct_dependencies, lockfile.direct_dependencies);
        assert_eq!(loaded_bin.packages.get("is-even@1.0.0").unwrap().integrity, None);
        assert_eq!(loaded_bin.packages.get("is-odd@1.0.0").unwrap().integrity, Some("sha512-abc".to_string()));

        lockfile.write_to_json(temp_json).unwrap();
        let loaded_json = Lockfile::read_from_json(temp_json).unwrap();
        assert_eq!(loaded_json.version, lockfile.version);
        assert_eq!(loaded_json.direct_dependencies, lockfile.direct_dependencies);
        assert_eq!(loaded_json.packages.get("is-even@1.0.0").unwrap().integrity, None);
        assert_eq!(loaded_json.packages.get("is-odd@1.0.0").unwrap().integrity, Some("sha512-abc".to_string()));

        let _ = std::fs::remove_file(temp_bin);
        let _ = std::fs::remove_file(temp_json);
    }
}
