use serde::{Serialize, Deserialize};
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{BufReader, BufWriter};
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
        let reader = BufReader::new(file);
        bincode::deserialize_from(reader).map_err(|e| format!("Failed to deserialize lockfile: {}", e))
    }

    pub fn write_to_file<P: AsRef<Path>>(&self, path: P) -> Result<(), String> {
        let file = File::create(path).map_err(|e| format!("Failed to create lockfile: {}", e))?;
        let writer = BufWriter::new(file);
        bincode::serialize_into(writer, self).map_err(|e| format!("Failed to serialize lockfile: {}", e))
    }
}
