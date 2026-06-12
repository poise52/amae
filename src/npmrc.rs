use std::collections::HashMap;
use std::fs;
use std::path::Path;
use directories::UserDirs;

#[derive(Debug, Clone, Default)]
pub struct Npmrc {
    pub registry: String,
    pub auth_tokens: HashMap<String, String>,
}

impl Npmrc {
    pub fn load() -> Self {
        let mut npmrc = Npmrc {
            registry: "https://registry.npmjs.org/".to_string(),
            auth_tokens: HashMap::new(),
        };

        if let Some(user_dirs) = UserDirs::new() {
            let home_dir = user_dirs.home_dir();
            let global_path = home_dir.join(".npmrc");
            if global_path.exists() {
                let _ = npmrc.parse_file(&global_path);
            }
        }

        let local_path = Path::new(".npmrc");
        if local_path.exists() {
            let _ = npmrc.parse_file(local_path);
        }

        npmrc
    }

    fn parse_file(&mut self, path: &Path) -> Result<(), String> {
        let content = fs::read_to_string(path).map_err(|e| e.to_string())?;
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
                continue;
            }

            if let Some(idx) = line.find('=') {
                let key = line[..idx].trim().to_string();
                let val = line[idx + 1..].trim().to_string();

                if key == "registry" {
                    self.registry = val;
                } else if key.ends_with(":_authToken") {
                    let domain = key.trim_end_matches(":_authToken").to_string();
                    self.auth_tokens.insert(domain, val);
                } else if key == "_authToken" {
                    self.auth_tokens.insert("default".to_string(), val);
                }
            }
        }
        Ok(())
    }

    pub fn get_token(&self, registry_url: &str) -> Option<&String> {
        for (k, v) in &self.auth_tokens {
            if k == "default" {
                continue;
            }
            let stripped_url = registry_url.trim_start_matches("https:").trim_start_matches("http:");
            if stripped_url.contains(k) || k.contains(stripped_url) {
                return Some(v);
            }
        }
        self.auth_tokens.get("default")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_npmrc() {
        let content = "
            registry=https://custom-registry.com/
            //custom-registry.com/:_authToken=secret-token-123
            _authToken=global-token-456
            ; comment line
            # another comment line
        ";

        let mut npmrc = Npmrc::default();
        let temp_file = tempfile::NamedTempFile::new().unwrap();
        let mut file = fs::File::create(temp_file.path()).unwrap();
        use std::io::Write;
        write!(file, "{}", content).unwrap();

        npmrc.parse_file(temp_file.path()).unwrap();

        assert_eq!(npmrc.registry, "https://custom-registry.com/");
        assert_eq!(npmrc.get_token("https://custom-registry.com/npm/"), Some(&"secret-token-123".to_string()));
        assert_eq!(npmrc.get_token("https://other-registry.com/"), Some(&"global-token-456".to_string()));
    }
}
