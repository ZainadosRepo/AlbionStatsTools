//! Atomic JSON settings persistence in the user's application config directory.
//! Writes to `.json.tmp` then renames, and moves corrupted files aside to `.json.bad`.

use serde::Serialize;
use serde_json::Value;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

const MAX_BYTES: usize = 1 << 20;

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LoadedSettings {
    pub settings: Value,
    pub warning: Option<String>,
}

pub fn load(path: &Path) -> Result<LoadedSettings, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return Ok(LoadedSettings {
                settings: Value::Null,
                warning: None,
            })
        }
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    match serde_json::from_str(&text) {
        Ok(settings) => Ok(LoadedSettings {
            settings,
            warning: None,
        }),
        Err(e) => {
            let backup = path.with_extension("json.bad");
            fs::rename(path, &backup)
                .map_err(|r| format!("{}: {e}, sauvegarde impossible : {r}", path.display()))?;
            let warning = format!(
                "réglages illisibles ({e}), copie conservée dans {}",
                backup.display()
            );
            Ok(LoadedSettings {
                settings: Value::Null,
                warning: Some(warning),
            })
        }
    }
}

pub fn save(path: &Path, settings: &Value) -> Result<(), String> {
    if !settings.is_object() {
        return Err("les réglages doivent être un objet JSON".into());
    }
    let text = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    if text.len() > MAX_BYTES {
        return Err(format!("réglages trop volumineux ({} octets)", text.len()));
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("albion-stats-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir.join("settings.json")
    }

    #[test]
    fn missing_file_reads_as_null() {
        let loaded = load(&temp("missing")).unwrap();
        assert_eq!((loaded.settings, loaded.warning), (Value::Null, None));
    }

    #[test]
    fn saves_then_loads_the_same_object() {
        let path = temp("roundtrip");
        let settings =
            json!({ "region": "europe", "compactOverlay": false, "meterMode": "damage" });
        save(&path, &settings).unwrap();
        assert_eq!(load(&path).unwrap().settings, settings);
    }

    #[test]
    fn moves_a_corrupt_file_aside_and_warns() {
        let path = temp("corrupt");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{ invalid json").unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.settings, Value::Null);
        assert!(loaded.warning.is_some());
        assert!(!path.exists());
        assert_eq!(
            fs::read_to_string(path.with_extension("json.bad")).unwrap(),
            "{ invalid json"
        );
    }

    #[test]
    fn refuses_non_objects() {
        assert!(save(&temp("array"), &json!([1, 2])).is_err());
    }
}
