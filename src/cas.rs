use std::fs;
use std::path::PathBuf;
use directories::UserDirs;
use sha1::{Sha1, Digest};
use tar::Archive;
use flate2::read::GzDecoder;

pub struct Cas {
    pub store_dir: PathBuf,
    pub tmp_dir: PathBuf,
}

impl Cas {
    pub fn new() -> Self {
        let home = UserDirs::new()
            .expect("Could not determine home directory")
            .home_dir()
            .to_path_buf();
        
        let amae_dir = home.join(".amae");
        let store_dir = amae_dir.join("store");
        let tmp_dir = amae_dir.join("tmp");

        fs::create_dir_all(&store_dir).expect("Failed to create global store directory");
        fs::create_dir_all(&tmp_dir).expect("Failed to create temporary directory");

        Self { store_dir, tmp_dir }
    }

    pub fn package_dir(&self, name: &str, version: &str) -> PathBuf {
        let escaped_name = name.replace('/', "+");
        self.store_dir.join(format!("{}@{}", escaped_name, version))
    }

    pub async fn download_and_extract(
        &self,
        client: &reqwest::Client,
        npmrc: &crate::npmrc::Npmrc,
        name: &str,
        version: &str,
        tarball_url: &str,
        expected_shasum: &str,
    ) -> Result<PathBuf, String> {
        let dest_dir = self.package_dir(name, version);
        
        if dest_dir.exists() {
            return Ok(dest_dir);
        }

        let mut last_err = String::new();
        let mut bytes = None;

        for attempt in 0..3u32 {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(500 * 2u64.pow(attempt - 1))).await;
            }

            let mut req = client.get(tarball_url);
            if let Some(token) = npmrc.get_token(tarball_url) {
                req = req.header("Authorization", format!("Bearer {}", token));
            }

            let response = match req.send().await {
                Ok(r) => r,
                Err(e) => {
                    last_err = format!("Failed to download tarball: {}", e);
                    continue;
                }
            };

            if !response.status().is_success() {
                last_err = format!("Failed to download package: HTTP status {}", response.status());
                continue;
            }

            match response.bytes().await {
                Ok(b) => {
                    bytes = Some(b);
                    break;
                }
                Err(e) => {
                    last_err = format!("Failed to read response bytes: {}", e);
                    continue;
                }
            }
        }

        let bytes = match bytes {
            Some(b) => b,
            None => return Err(last_err),
        };

        let mut hasher = Sha1::new();
        hasher.update(&bytes);
        let shasum = format!("{:x}", hasher.finalize());
        if shasum != expected_shasum {
            return Err(format!(
                "Integrity check failed for {}. Expected shasum {}, got {}",
                name, expected_shasum, shasum
            ));
        }

        let temp_extract_dir = tempfile::Builder::new()
            .prefix("amae-extract-")
            .tempdir_in(&self.tmp_dir)
            .map_err(|e| format!("Failed to create temp extract directory: {}", e))?;

        let tar = GzDecoder::new(&bytes[..]);
        let mut archive = Archive::new(tar);
        
        archive.unpack(temp_extract_dir.path())
            .map_err(|e| format!("Failed to unpack tarball: {}", e))?;

        let entries = fs::read_dir(temp_extract_dir.path())
            .map_err(|e| format!("Failed to read temp extract directory: {}", e))?;

        let mut npm_package_dir = None;
        for entry in entries {
            let entry = entry.map_err(|e| format!("Failed to read entry: {}", e))?;
            let path = entry.path();
            if path.is_dir() {
                npm_package_dir = Some(path);
                break;
            }
        }

        let npm_package_dir = match npm_package_dir {
            Some(dir) => dir,
            None => {
                return Err(format!(
                    "Invalid package tarball format for {}: no directory found in archive", name
                ));
            }
        };

        if !dest_dir.exists() {
            fs::create_dir_all(dest_dir.parent().unwrap())
                .map_err(|e| format!("Failed to create parent dir: {}", e))?;
            
            if let Err(e) = fs::rename(&npm_package_dir, &dest_dir) {
                if !dest_dir.exists() {
                    return Err(format!("Failed to move extracted package to store: {}", e));
                }
            }

            if let Err(e) = make_dir_read_only(&dest_dir) {
                return Err(format!("Failed to make package store directory read-only: {}", e));
            }
        }

        Ok(dest_dir)
    }
}

fn make_dir_read_only(dir: &std::path::Path) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = entry.metadata()?;
        let mut perms = metadata.permissions();

        if metadata.is_dir() {
            make_dir_read_only(&path)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = perms.mode();
                perms.set_mode((mode & !0o222) | 0o111);
            }
            #[cfg(not(unix))]
            perms.set_readonly(true);
            fs::set_permissions(&path, perms)?;
        } else {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = perms.mode();
                perms.set_mode(mode & !0o222);
            }
            #[cfg(not(unix))]
            perms.set_readonly(true);
            fs::set_permissions(&path, perms)?;
        }
    }

    let metadata = fs::metadata(dir)?;
    let mut perms = metadata.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = perms.mode();
        perms.set_mode((mode & !0o222) | 0o111);
    }
    #[cfg(not(unix))]
    perms.set_readonly(true);
    fs::set_permissions(dir, perms)?;

    Ok(())
}
