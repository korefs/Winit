use crate::domain::{CustomApp, ManualPending, Operation, OperationState, PackageRef, Profile};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub dark_theme: bool,
    pub downloads: PathBuf,
    pub silent_installers: bool,
    pub accept_source_agreements: bool,
    pub repositories: Vec<String>,
    pub last_profile: Option<String>,
}
impl Default for Settings {
    fn default() -> Self {
        let base = std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        Self {
            dark_theme: true,
            downloads: base.join("Downloads").join("Winit"),
            silent_installers: false,
            accept_source_agreements: false,
            repositories: vec!["itsfatduck/optimizerDuck".into()],
            last_profile: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedData {
    pub schema_version: u32,
    pub settings: Settings,
    pub profiles: Vec<Profile>,
    pub favorites: Vec<PackageRef>,
    pub history: Vec<Operation>,
    pub custom_apps: Vec<CustomApp>,
    pub manual_pending: Vec<ManualPending>,
    pub last_batch: Option<String>,
}
impl Default for SavedData {
    fn default() -> Self {
        Self {
            schema_version: 2,
            settings: Settings::default(),
            profiles: vec![
                serde_json::from_str(include_str!("../../assets/essentials.json"))
                    .expect("Perfil embarcado válido"),
            ],
            favorites: vec![],
            history: vec![],
            custom_apps: vec![],
            manual_pending: vec![],
            last_batch: None,
        }
    }
}

pub struct Storage {
    pub root: PathBuf,
    pub portable: bool,
}
impl Storage {
    pub fn discover() -> Result<Self> {
        let exe_dir = std::env::current_exe()?
            .parent()
            .context("Executável sem diretório")?
            .to_path_buf();
        let portable =
            exe_dir.join("portable.flag").exists() || std::env::args().any(|s| s == "--portable");
        let root = if portable {
            compatible_data_root(&exe_dir, "winit-data", "aio-data")
        } else {
            compatible_data_root(
                &PathBuf::from(
                    std::env::var_os("LOCALAPPDATA").context("LOCALAPPDATA indisponível")?,
                ),
                "Winit",
                "AIO Toolkit",
            )
        };
        fs::create_dir_all(&root).context("Não foi possível criar o diretório de dados")?;
        Ok(Self { root, portable })
    }
    pub fn load(&self) -> Result<SavedData> {
        let path = self.root.join("state.json");
        if !path.exists() {
            return Ok(SavedData::default());
        }
        if fs::metadata(&path)?.len() > 32 * 1024 * 1024 {
            bail!("Arquivo de dados excede 32 MB");
        }
        let mut data: SavedData = serde_json::from_slice(&fs::read(&path)?).context(
            "state.json inválido; restaure state.backup.json. O original não foi sobrescrito.",
        )?;
        if data.schema_version != 1 && data.schema_version != 2 {
            bail!("Versão dos dados não suportada");
        }
        if data.schema_version == 1 {
            let migration_backup = self.root.join("state.v1.backup.json");
            if !migration_backup.exists() {
                atomic_write(&migration_backup, &fs::read(&path)?)?;
            }
            data.schema_version = 2;
        }
        crate::services::catalog::validate(&data.custom_apps)?;
        if data.manual_pending.len() > 1000 {
            bail!("Arquivo excede o limite de pendências");
        }
        let mut pending_ids = std::collections::HashSet::new();
        for p in &data.manual_pending {
            p.validate()?;
            if !pending_ids.insert(p.id) {
                bail!("ID de pendência duplicado");
            }
        }
        for p in &data.profiles {
            p.validate()?;
        }
        for f in &data.favorites {
            f.validate()?;
        }
        for r in &data.settings.repositories {
            crate::domain::validate_repo(r)?;
        }
        for op in &mut data.history {
            if matches!(op.state, OperationState::Running | OperationState::Pending) {
                op.state = OperationState::Failed;
                op.message = "Sessão anterior interrompida. Verifique o sistema antes de tentar novamente; um instalador pode ainda estar ativo.".into();
            }
        }
        Ok(data)
    }
    pub fn save(&self, data: &SavedData) -> Result<()> {
        let path = self.root.join("state.json");
        let bytes = serde_json::to_vec_pretty(data)?;
        if bytes.len() > 32 * 1024 * 1024 {
            bail!(
                "Os dados excedem 32 MB. Exporte e reduza perfis, catálogo ou histórico antes de continuar."
            );
        }
        if path.exists() {
            fs::copy(&path, self.root.join("state.backup.json"))?;
        }
        atomic_write(&path, &bytes)
    }
}

// Keep existing installations on the same directory and instance lock after the rename.
fn compatible_data_root(parent: &Path, name: &str, legacy_name: &str) -> PathBuf {
    let current = parent.join(name);
    let legacy = parent.join(legacy_name);
    if !current.exists() && legacy.is_dir() {
        legacy
    } else {
        current
    }
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("Caminho sem diretório")?;
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!(".winit-{}.tmp", Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        replace_file(&temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

#[cfg(windows)]
pub fn publish_new_file(from: &Path, to: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};
    let a: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let b: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe { MoveFileExW(a.as_ptr(), b.as_ptr(), MOVEFILE_WRITE_THROUGH) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}
#[cfg(not(windows))]
pub fn publish_new_file(from: &Path, to: &Path) -> Result<()> {
    fs::hard_link(from, to)?;
    fs::remove_file(from)?;
    Ok(())
}

#[cfg(windows)]
fn replace_file(from: &Path, to: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    let a: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let b: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe {
        MoveFileExW(
            a.as_ptr(),
            b.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}
#[cfg(not(windows))]
fn replace_file(from: &Path, to: &Path) -> Result<()> {
    fs::rename(from, to)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renamed_app_reuses_existing_state_and_prefers_explicit_new_directory() {
        let parent = std::env::temp_dir().join(format!("winit-storage-{}", Uuid::new_v4()));
        fs::create_dir(&parent).unwrap();
        for (name, legacy_name) in [("Winit", "AIO Toolkit"), ("winit-data", "aio-data")] {
            assert_eq!(
                compatible_data_root(&parent, name, legacy_name),
                parent.join(name)
            );
            let legacy = Storage {
                root: parent.join(legacy_name),
                portable: true,
            };
            let mut data = SavedData::default();
            data.settings.dark_theme = false;
            legacy.save(&data).unwrap();
            let discovered = Storage {
                root: compatible_data_root(&parent, name, legacy_name),
                portable: true,
            };
            assert_eq!(discovered.root, legacy.root);
            assert!(!discovered.load().unwrap().settings.dark_theme);
            fs::create_dir(parent.join(name)).unwrap();
            assert_eq!(
                compatible_data_root(&parent, name, legacy_name),
                parent.join(name)
            );
            fs::remove_file(legacy.root.join("state.json")).unwrap();
            fs::remove_dir(legacy.root).unwrap();
            fs::remove_dir(parent.join(name)).unwrap();
        }
        fs::remove_dir(parent).unwrap();
    }
}
