use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut};

pub fn register_hotkey(app: &tauri::AppHandle, hotkey_str: &str) -> Result<String, String> {
    let shortcut = parse_hotkey(hotkey_str)?;
    let _ = app.global_shortcut().unregister_all();
    app.global_shortcut()
        .register(shortcut)
        .map_err(|e| e.to_string())?;
    Ok(normalize_hotkey(hotkey_str))
}

pub fn parse_hotkey(s: &str) -> Result<Shortcut, String> {
    let parts: Vec<&str> = s.split('+').collect();
    if parts.is_empty() {
        return Err("Empty hotkey string".to_string());
    }
    let mut modifiers = Modifiers::empty();
    let mut code_part = String::new();
    for part in &parts {
        let lower = part.to_lowercase();
        match lower.as_str() {
            "ctrl" | "control" => modifiers |= Modifiers::CONTROL,
            "cmd" | "command" | "super" => modifiers |= Modifiers::SUPER,
            "alt" | "option" => modifiers |= Modifiers::ALT,
            "shift" => modifiers |= Modifiers::SHIFT,
            other => code_part = other.to_string(),
        }
    }
    let code = match code_part.as_str() {
        "space" => Code::Space,
        c if c.len() == 1 && c.chars().next().unwrap().is_ascii_alphabetic() => {
            let ch = c.chars().next().unwrap().to_ascii_uppercase();
            match ch {
                'A' => Code::KeyA,
                'B' => Code::KeyB,
                'C' => Code::KeyC,
                'D' => Code::KeyD,
                'E' => Code::KeyE,
                'F' => Code::KeyF,
                'G' => Code::KeyG,
                'H' => Code::KeyH,
                'I' => Code::KeyI,
                'J' => Code::KeyJ,
                'K' => Code::KeyK,
                'L' => Code::KeyL,
                'M' => Code::KeyM,
                'N' => Code::KeyN,
                'O' => Code::KeyO,
                'P' => Code::KeyP,
                'Q' => Code::KeyQ,
                'R' => Code::KeyR,
                'S' => Code::KeyS,
                'T' => Code::KeyT,
                'U' => Code::KeyU,
                'V' => Code::KeyV,
                'W' => Code::KeyW,
                'X' => Code::KeyX,
                'Y' => Code::KeyY,
                'Z' => Code::KeyZ,
                _ => return Err(format!("Unknown key: {}", c)),
            }
        }
        _ => return Err(format!("Unknown key: {}", code_part)),
    };
    Ok(Shortcut::new(Some(modifiers), code))
}

pub fn normalize_hotkey(s: &str) -> String {
    let mut parts = Vec::new();
    let mut key = None;

    for part in s.split('+') {
        let lower = part.trim().to_lowercase();
        match lower.as_str() {
            "ctrl" | "control" => parts.push("ctrl".to_string()),
            "cmd" | "command" | "super" => parts.push("cmd".to_string()),
            "alt" | "option" => parts.push("alt".to_string()),
            "shift" => parts.push("shift".to_string()),
            "" => {}
            other => key = Some(other.to_string()),
        }
    }

    if let Some(key) = key {
        parts.push(key);
    }

    parts.join("+")
}
