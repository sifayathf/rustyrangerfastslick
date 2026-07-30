use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct UserSettings {
    pub explorer_view: bool,
    pub font_face: String,
    pub font_size: u16,
    pub font_weight: u16,
    pub ultra_fast: bool,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            explorer_view: false,
            font_face: "Cascadia Code".to_string(),
            font_size: 9,
            font_weight: 400,
            ultra_fast: false,
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
    settings.ultra_fast = value
        .get("ultra_fast")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(settings.ultra_fast);
    settings.font_face = value
        .get("font_face")
        .and_then(serde_json::Value::as_str)
        .filter(|face| matches!(*face, "Cascadia Code" | "Consolas" | "Lucida Console"))
        .unwrap_or(&settings.font_face)
        .to_string();
    settings.font_size = value
        .get("font_size")
        .and_then(serde_json::Value::as_u64)
        .map(|value| value.clamp(8, 16) as u16)
        .unwrap_or(settings.font_size);
    settings.font_weight = value
        .get("font_weight")
        .and_then(serde_json::Value::as_u64)
        .map(|value| ((value.clamp(300, 800) / 100) * 100) as u16)
        .unwrap_or(settings.font_weight);
    settings
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
        "font_face": settings.font_face,
        "font_size": settings.font_size,
        "font_weight": settings.font_weight,
        "ultra_fast": settings.ultra_fast,
    }))?;
    std::fs::write(path, content)
}

#[cfg(test)]
mod tests {
    use super::UserSettings;

    #[test]
    fn defaults_are_fast_but_not_busy_spinning() {
        let settings = UserSettings::default();
        assert_eq!(settings.font_face, "Cascadia Code");
        assert_eq!(settings.font_size, 9);
        assert_eq!(settings.font_weight, 400);
        assert!(!settings.ultra_fast);
        assert!(!settings.explorer_view);
    }
}
