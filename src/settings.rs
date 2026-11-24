use std::{
    fs,
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use uuid::Uuid;

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub skipped_setup: bool,
    pub translation: Option<TranslationSettings>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct TranslationSettings {
    pub provider: Provider,
    pub target_language: String,
    pub credential_id: Uuid,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Aliyun,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Credentials {
    pub access_key_id: String,
    pub access_key_secret: String,
}

pub struct Storage {
    config: PathBuf,
    pub cache: PathBuf,
}

impl Storage {
    pub fn new() -> Result<Self> {
        let dirs = BaseDirs::new().context("could not find your home directory")?;
        Ok(Self {
            config: dirs.config_dir().join("mangloss/settings.json"),
            cache: dirs.cache_dir().join("mangloss/translations"),
        })
    }

    pub fn load(&self) -> Result<Settings> {
        match fs::read(&self.config) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("invalid settings file {}", self.config.display())),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(Settings::default()),
            Err(error) => Err(error).context("could not read settings"),
        }
    }

    pub fn save(&self, settings: &Settings) -> Result<()> {
        write_atomic(&self.config, &serde_json::to_vec_pretty(settings)?)
            .context("could not save settings")
    }

    pub fn configure(
        &self,
        settings: Settings,
        target: String,
        credentials: &Credentials,
    ) -> Result<()> {
        let new_id = Uuid::new_v4();
        let entry = credential_entry(new_id)?;
        entry
            .set_password(&serde_json::to_string(credentials)?)
            .context("could not save to the system credential store")?;
        let updated = Settings {
            skipped_setup: false,
            translation: Some(TranslationSettings {
                provider: Provider::Aliyun,
                target_language: target,
                credential_id: new_id,
            }),
        };
        if let Err(error) = self.save(&updated) {
            let _ = entry.delete_credential();
            return Err(error);
        }
        if let Some(old) = settings.translation
            && let Ok(old_entry) = credential_entry(old.credential_id)
        {
            let _ = old_entry.delete_credential();
        }
        Ok(())
    }
}

pub fn credentials(settings: &TranslationSettings) -> Result<Credentials> {
    let entry = credential_entry(settings.credential_id)?;
    let password = entry
        .get_password()
        .context("could not load credentials from the system credential store")?;
    serde_json::from_str(&password).context("invalid stored credentials")
}

fn credential_entry(id: Uuid) -> Result<keyring::Entry> {
    keyring::Entry::new("mangloss.aliyun", &id.to_string())
        .context("could not access the system credential store")
}

pub fn write_atomic(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path.parent().context("file has no parent directory")?;
    fs::create_dir_all(parent)?;
    let mut temporary = NamedTempFile::new_in(parent)?;
    temporary.write_all(contents)?;
    temporary.persist(path)?;
    Ok(())
}
