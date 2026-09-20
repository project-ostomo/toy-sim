use anyhow::{Context, Result, ensure};
use ed25519_dalek::SigningKey;
use osg_model::Id;
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub account: Id,
    account_secret: [u8; 32],
    server_secret: [u8; 32],
    pub ship: Option<PathBuf>,
}

impl Identity {
    pub fn account_key(&self) -> SigningKey {
        SigningKey::from_bytes(&self.account_secret)
    }

    pub fn server_key(&self) -> SigningKey {
        SigningKey::from_bytes(&self.server_secret)
    }
}

pub struct StateDirectory {
    pub path: PathBuf,
    pub identity: Identity,
    ephemeral: bool,
    _lock: File,
}

impl StateDirectory {
    pub fn open(path: PathBuf, ephemeral: bool, ship: Option<PathBuf>) -> Result<Self> {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&path)?;
        let path = std::fs::canonicalize(path)?;
        let lock = private_options()
            .create(true)
            .truncate(false)
            .open(path.join("debug.lock"))?;
        lock.try_lock()
            .context("this debug world is already open in another process")?;

        let identity_path = path.join("identity.toml");
        let identity = if identity_path.exists() {
            private_permissions(&identity_path)?;
            let saved: Identity = toml::from_str(&std::fs::read_to_string(&identity_path)?)
                .context("read saved debug identity")?;
            ensure!(
                ship.is_none() || saved.ship == ship,
                "this saved world uses a different ship; use a new --state-dir or --ephemeral"
            );
            saved
        } else {
            ensure!(
                !path.join("world.sqlite").exists() && !path.join("server.toml").exists(),
                "saved world has no identity.toml; restore its identity from backup or choose a new --state-dir"
            );
            let identity = Identity {
                account: Id::new(),
                account_secret: rand::random(),
                server_secret: rand::random(),
                ship,
            };
            write_private(&identity_path, &toml::to_string(&identity)?)?;
            identity
        };
        Ok(Self {
            path,
            identity,
            ephemeral,
            _lock: lock,
        })
    }
}

impl Drop for StateDirectory {
    fn drop(&mut self) {
        if self.ephemeral {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

pub fn default_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("XDG_STATE_HOME").filter(|path| !path.is_empty()) {
        return Ok(PathBuf::from(path).join("openspacegame/debug"));
    }
    let home = std::env::var_os("HOME").context("HOME unavailable; specify --state-dir")?;
    Ok(PathBuf::from(home).join(".local/state/openspacegame/debug"))
}

fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

fn private_permissions(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

pub fn write_private(path: &Path, text: &str) -> Result<()> {
    let temporary = path.with_extension("tmp");
    let mut file = private_options()
        .create(true)
        .truncate(true)
        .open(&temporary)?;
    private_permissions(&temporary)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    std::fs::rename(temporary, path)?;
    #[cfg(unix)]
    File::open(
        path.parent()
            .context("state file has no parent directory")?,
    )?
    .sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_identity_survives_reopen_and_rejects_concurrent_launch() {
        let path = std::env::temp_dir().join(format!("osg-debug-state-{}", Id::new()));
        let first = StateDirectory::open(path.clone(), false, None).unwrap();
        let account = first.identity.account;
        let account_key = first.identity.account_key().to_bytes();
        let server_key = first.identity.server_key().to_bytes();
        assert!(StateDirectory::open(path.clone(), false, None).is_err());
        drop(first);
        let second = StateDirectory::open(path.clone(), false, None).unwrap();
        assert_eq!(second.identity.account, account);
        assert_eq!(second.identity.account_key().to_bytes(), account_key);
        assert_eq!(second.identity.server_key().to_bytes(), server_key);
        drop(second);
        assert!(StateDirectory::open(path.clone(), false, Some("other.toml".into())).is_err());
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn missing_identity_cannot_silently_orphan_an_existing_world() {
        let path = std::env::temp_dir().join(format!("osg-debug-state-{}", Id::new()));
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("world.sqlite"), []).unwrap();
        assert!(StateDirectory::open(path.clone(), false, None).is_err());
        assert!(!path.join("identity.toml").exists());
        std::fs::remove_dir_all(path).unwrap();
    }
}
