use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;

use super::model::FoundDevice;
use super::scan_model::ScanIdentity;
use crate::error::InteractionError;

const OVERRIDES_FILE_NAME: &str = "model-overrides.tsv";

/// Runtime model-resolution options supplied by CLI arguments.
#[derive(Debug, Clone, Eq, PartialEq, Default)]
pub struct ModelResolutionConfig {
    led_type_override: Option<u8>,
    overrides_path: Option<PathBuf>,
}

impl ModelResolutionConfig {
    /// Creates model-resolution options.
    #[must_use]
    pub fn new(led_type_override: Option<u8>, overrides_path: Option<PathBuf>) -> Self {
        Self {
            led_type_override,
            overrides_path,
        }
    }

    /// Returns the optional explicit LED-type override.
    #[must_use]
    pub fn led_type_override(&self) -> Option<u8> {
        self.led_type_override
    }

    /// Returns the optional custom overrides file path.
    #[must_use]
    pub fn overrides_path(&self) -> Option<&Path> {
        self.overrides_path.as_deref()
    }
}

/// Persistent store for per-device ambiguous-shape LED-type choices.
#[derive(Debug, Default)]
pub(crate) struct ModelOverrideStore {
    path: PathBuf,
    entries: HashMap<String, u8>,
}

impl ModelOverrideStore {
    pub(crate) fn load(config: &ModelResolutionConfig) -> Result<Self, InteractionError> {
        let path = config
            .overrides_path()
            .map(Path::to_path_buf)
            .unwrap_or_else(default_override_path);
        Self::load_from_path(path)
    }

    pub(crate) fn load_from_path(path: PathBuf) -> Result<Self, InteractionError> {
        let entries = if path.exists() {
            let raw = fs::read_to_string(&path)
                .map_err(|source| InteractionError::ModelOverrideIo { source })?;
            parse_entries(&raw)?
        } else {
            HashMap::new()
        };
        Ok(Self { path, entries })
    }

    #[must_use]
    pub(crate) fn led_type_for(&self, device: &FoundDevice, identity: &ScanIdentity) -> Option<u8> {
        self.entries.get(&entry_key(device, identity)).copied()
    }

    pub(crate) fn persist_led_type_for(
        &mut self,
        device: &FoundDevice,
        identity: &ScanIdentity,
        led_type: u8,
    ) -> Result<(), InteractionError> {
        if !is_supported_led_type(led_type) {
            return Err(InteractionError::InvalidLedTypeOverride { value: led_type });
        }

        self.entries.insert(entry_key(device, identity), led_type);
        self.save()
    }

    fn save(&self) -> Result<(), InteractionError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|source| InteractionError::ModelOverrideIo { source })?;
        }

        let mut rows = self.entries.iter().collect::<Vec<_>>();
        rows.sort_by(|(left_key, _), (right_key, _)| left_key.cmp(right_key));
        let serialised = rows
            .into_iter()
            .map(|(key, value)| format!("{key}\t{value}\n"))
            .collect::<String>();

        fs::write(&self.path, serialised)
            .map_err(|source| InteractionError::ModelOverrideIo { source })?;
        Ok(())
    }
}

pub(crate) fn is_supported_led_type(value: u8) -> bool {
    matches!(value, 1 | 2 | 3 | 4 | 6 | 7 | 11)
}

fn parse_entries(contents: &str) -> Result<HashMap<String, u8>, InteractionError> {
    let mut entries = HashMap::new();
    for raw_line in contents.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let mut fields = line.split('\t');
        let (Some(key), Some(value), None) = (fields.next(), fields.next(), fields.next()) else {
            return Err(InteractionError::InvalidModelOverrideRecord {
                record: line.to_string(),
            });
        };

        let led_type =
            value
                .parse::<u8>()
                .map_err(|_error| InteractionError::InvalidModelOverrideRecord {
                    record: line.to_string(),
                })?;
        if !is_supported_led_type(led_type) {
            return Err(InteractionError::InvalidLedTypeOverride { value: led_type });
        }

        entries.insert(key.to_string(), led_type);
    }

    Ok(entries)
}

fn entry_key(device: &FoundDevice, identity: &ScanIdentity) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}",
        device.device_id().to_ascii_lowercase(),
        identity.cid,
        identity.pid,
        identity.shape,
        identity.group_id,
        identity.device_id
    )
}

fn default_override_path() -> PathBuf {
    let project_dirs = ProjectDirs::from("uk.co", "OrangeSquash", "idm");
    let Some(project_dirs) = project_dirs else {
        return std::env::temp_dir().join("idm").join(OVERRIDES_FILE_NAME);
    };

    let root = project_dirs
        .state_dir()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| project_dirs.data_local_dir().to_path_buf());
    root.join(OVERRIDES_FILE_NAME)
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    use pretty_assertions::assert_eq;

    use super::*;
    use crate::hw::scan_model::ScanModelHandler;

    fn unique_temp_path(file_name: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("idm-{file_name}-{suffix}.tsv"))
    }

    fn remove_if_exists(path: &Path) {
        if path.exists() {
            fs::remove_file(path).expect("temporary fixture file should be removable");
        }
    }

    fn device() -> FoundDevice {
        FoundDevice::new(
            "hci0".to_string(),
            "AA:BB:CC:DD".to_string(),
            Some("IDM-1+3_TEST".to_string()),
            Some(-40),
        )
    }

    fn ambiguous_identity() -> ScanIdentity {
        let payload = [
            0x54, 0x52, 0x00, 0x70, 0x81, 0x01, 0x02, 0x00, 0x01, 0x07, 0x20, 0x00, 0x21, 0x00,
        ];
        ScanModelHandler::parse_identity(&payload).expect("fixture payload should parse")
    }

    #[test]
    fn store_round_trips_persisted_led_type() {
        let path = unique_temp_path("model-override");
        remove_if_exists(&path);
        let mut store =
            ModelOverrideStore::load_from_path(path.clone()).expect("new store should load");
        let device = device();
        let identity = ambiguous_identity();

        assert_eq!(None, store.led_type_for(&device, &identity));
        store
            .persist_led_type_for(&device, &identity, 2)
            .expect("persisting supported led type should succeed");

        let reloaded =
            ModelOverrideStore::load_from_path(path.clone()).expect("stored file should reload");
        assert_eq!(Some(2), reloaded.led_type_for(&device, &identity));

        remove_if_exists(&path);
    }

    #[test]
    fn store_rejects_invalid_record() {
        let path = unique_temp_path("model-override-invalid");
        fs::write(&path, "broken-record\n").expect("invalid fixture should write");

        let loaded = ModelOverrideStore::load_from_path(path.clone());
        assert!(matches!(
            loaded,
            Err(InteractionError::InvalidModelOverrideRecord { .. })
        ));

        remove_if_exists(&path);
    }
}
