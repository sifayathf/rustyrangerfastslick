use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct UserSettings {
    pub explorer_view: bool,
    pub theme_light: bool,
    pub office_full: bool,
    pub pdf_visual: bool,
    pub dir_preview_clickable: bool,
    pub sort_mode: String,
    pub sort_descending: bool,
    pub show_file_details: bool,
    pub rounded_selection: bool,
    pub hover_enabled: bool,
    pub font_face: String,
    pub font_size: u16,
    pub font_weight: u16,
    pub preview_mode: String,
    pub sidebar_width: u16,
    pub column_ratios: Vec<f32>,
    pub last_location: Option<String>,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            explorer_view: false,
            theme_light: false,
            office_full: false,
            pdf_visual: false,
            dir_preview_clickable: true,
            sort_mode: "name".to_string(),
            sort_descending: false,
            show_file_details: false,
            rounded_selection: false,
            hover_enabled: true,
            font_face: "Cascadia Code".to_string(),
            font_size: 9,
            font_weight: 400,
            preview_mode: "normal".to_string(),
            sidebar_width: 26,
            column_ratios: vec![0.10, 0.10, 0.12, 0.18, 0.50],
            last_location: None,
        }
    }
}

fn settings_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("rusty-ranger-fast").join("settings.json"))
}

pub fn load() -> UserSettings {
    let Some(path) = settings_path() else {
        return UserSettings::default();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return UserSettings::default();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return UserSettings::default();
    };

    let mut settings = UserSettings::default();
    settings.explorer_view = value
        .get("explorer_view")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(settings.explorer_view);
    settings.preview_mode = preview_mode_setting(&value);
    settings.theme_light = bool_setting(&value, "theme_light", settings.theme_light);
    settings.office_full = bool_setting(&value, "office_full", settings.office_full);
    settings.pdf_visual = bool_setting(&value, "pdf_visual", settings.pdf_visual);
    settings.dir_preview_clickable = bool_setting(
        &value,
        "dir_preview_clickable",
        settings.dir_preview_clickable,
    );
    settings.sort_descending = bool_setting(&value, "sort_descending", settings.sort_descending);
    settings.show_file_details =
        bool_setting(&value, "show_file_details", settings.show_file_details);
    settings.rounded_selection =
        bool_setting(&value, "rounded_selection", settings.rounded_selection);
    settings.hover_enabled = bool_setting(&value, "hover_enabled", settings.hover_enabled);
    settings.sort_mode = value
        .get("sort_mode")
        .and_then(serde_json::Value::as_str)
        .filter(|mode| matches!(*mode, "name" | "modified" | "size"))
        .unwrap_or(&settings.sort_mode)
        .to_string();
    settings.font_face = value
        .get("font_face")
        .and_then(serde_json::Value::as_str)
        .filter(|face| {
            matches!(
                *face,
                "Cascadia Code" | "Consolas" | "Lucida Console" | "Nirmala UI"
            )
        })
        .unwrap_or(&settings.font_face)
        .to_string();
    settings.font_size = value
        .get("font_size")
        .and_then(serde_json::Value::as_u64)
        .map(|value| value.clamp(6, 24) as u16)
        .unwrap_or(settings.font_size);
    settings.font_weight = value
        .get("font_weight")
        .and_then(serde_json::Value::as_u64)
        .map(|value| ((value.clamp(300, 800) / 100) * 100) as u16)
        .unwrap_or(settings.font_weight);
    settings.sidebar_width = value
        .get("sidebar_width")
        .and_then(serde_json::Value::as_u64)
        .map(|width| width.clamp(18, 36) as u16)
        .unwrap_or(settings.sidebar_width);
    if let Some(values) = value
        .get("column_ratios")
        .and_then(serde_json::Value::as_array)
    {
        let ratios: Vec<f32> = values
            .iter()
            .filter_map(serde_json::Value::as_f64)
            .map(|ratio| ratio as f32)
            .collect();
        if ratios.len() == 5
            && ratios
                .iter()
                .all(|ratio| ratio.is_finite() && *ratio >= 0.04)
        {
            let total: f32 = ratios.iter().sum();
            if total > 0.0 {
                settings.column_ratios = ratios.into_iter().map(|ratio| ratio / total).collect();
            }
        }
    }
    settings.last_location = value
        .get("last_location")
        .and_then(serde_json::Value::as_str)
        .filter(|path| !path.trim().is_empty())
        .map(ToOwned::to_owned);
    settings
}

fn preview_mode_setting(value: &serde_json::Value) -> String {
    value
        .get("preview_mode")
        .and_then(serde_json::Value::as_str)
        .filter(|mode| matches!(*mode, "normal" | "full" | "showcase" | "blitz"))
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| {
            // Migrate the previous two-state setting without breaking an
            // existing user profile.
            if value
                .get("ultra_fast")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
            {
                "blitz".to_string()
            } else {
                "normal".to_string()
            }
        })
}

fn bool_setting(value: &serde_json::Value, key: &str, fallback: bool) -> bool {
    value
        .get(key)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(fallback)
}

pub fn save(settings: &UserSettings) -> std::io::Result<()> {
    let Some(path) = settings_path() else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(&serde_json::json!({
        "explorer_view": settings.explorer_view,
        "theme_light": settings.theme_light,
        "office_full": settings.office_full,
        "pdf_visual": settings.pdf_visual,
        "dir_preview_clickable": settings.dir_preview_clickable,
        "sort_mode": settings.sort_mode,
        "sort_descending": settings.sort_descending,
        "show_file_details": settings.show_file_details,
        "rounded_selection": settings.rounded_selection,
        "hover_enabled": settings.hover_enabled,
        "font_face": settings.font_face,
        "font_size": settings.font_size,
        "font_weight": settings.font_weight,
        "preview_mode": settings.preview_mode,
        "sidebar_width": settings.sidebar_width,
        "column_ratios": settings.column_ratios,
        "last_location": settings.last_location,
    }))?;
    write_atomic(&path, content.as_bytes())
}

pub fn write_atomic(path: &std::path::Path, content: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("settings");
    let temp = path.with_file_name(format!(".{file_name}.{}.tmp", std::process::id()));
    {
        use std::io::Write;
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(content)?;
        file.sync_all()?;
    }
    if let Err(error) = atomic_replace(&temp, path) {
        let _ = std::fs::remove_file(temp);
        return Err(error);
    }
    Ok(())
}

#[cfg(windows)]
fn atomic_replace(source: &std::path::Path, destination: &std::path::Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let ok = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if ok != 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(windows))]
fn atomic_replace(source: &std::path::Path, destination: &std::path::Path) -> std::io::Result<()> {
    std::fs::rename(source, destination)
}

#[cfg(test)]
mod tests {
    use super::{preview_mode_setting, write_atomic, UserSettings};

    #[test]
    fn preview_mode_migrates_the_old_blitz_boolean() {
        assert_eq!(
            preview_mode_setting(&serde_json::json!({ "ultra_fast": true })),
            "blitz"
        );
        assert_eq!(
            preview_mode_setting(&serde_json::json!({
                "ultra_fast": true,
                "preview_mode": "full"
            })),
            "full"
        );
        assert_eq!(
            preview_mode_setting(&serde_json::json!({
                "preview_mode": "showcase"
            })),
            "showcase"
        );
    }

    #[test]
    fn defaults_are_fast_but_not_busy_spinning() {
        let settings = UserSettings::default();
        assert_eq!(settings.font_face, "Cascadia Code");
        assert_eq!(settings.font_size, 9);
        assert_eq!(settings.font_weight, 400);
        assert_eq!(settings.preview_mode, "normal");
        assert!(!settings.explorer_view);
        assert!(settings.dir_preview_clickable);
        assert!(settings.hover_enabled);
        assert_eq!(settings.sidebar_width, 26);
        assert_eq!(settings.column_ratios.len(), 5);
    }

    #[test]
    fn atomic_write_replaces_complete_content_without_leaving_temp_files() {
        let directory =
            std::env::temp_dir().join(format!("rusty-ranger-settings-test-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("settings.json");
        write_atomic(&path, b"old").unwrap();
        write_atomic(&path, b"new complete content").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new complete content");
        assert!(std::fs::read_dir(&directory)
            .unwrap()
            .flatten()
            .all(|entry| !entry.file_name().to_string_lossy().ends_with(".tmp")));
        std::fs::remove_dir_all(directory).unwrap();
    }
}
