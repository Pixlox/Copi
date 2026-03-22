use tauri_plugin_global_shortcut::{GlobalShortcutExt, Modifiers, Shortcut};

pub fn register_hotkey(app: &tauri::AppHandle, hotkey_str: &str) -> Result<(), String> {
    let _ = app.global_shortcut().unregister_all();
    let shortcut = parse_hotkey(hotkey_str)?;
    app.global_shortcut()
        .register(shortcut)
        .map_err(|e| e.to_string())
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
            "ctrl" | "control" | "cmd" | "command" | "super" => modifiers |= Modifiers::CONTROL,
            "alt" | "option" => modifiers |= Modifiers::ALT,
            "shift" => modifiers |= Modifiers::SHIFT,
            other => code_part = other.to_string(),
        }
    }
    let code = match code_part.as_str() {
        "space" => tauri_plugin_global_shortcut::Code::Space,
        c if c.len() == 1 && c.chars().next().unwrap().is_ascii_alphabetic() => {
            let ch = c.chars().next().unwrap().to_ascii_uppercase();
            match ch {
                'A' => tauri_plugin_global_shortcut::Code::KeyA,
                'B' => tauri_plugin_global_shortcut::Code::KeyB,
                'C' => tauri_plugin_global_shortcut::Code::KeyC,
                'D' => tauri_plugin_global_shortcut::Code::KeyD,
                'E' => tauri_plugin_global_shortcut::Code::KeyE,
                'F' => tauri_plugin_global_shortcut::Code::KeyF,
                'G' => tauri_plugin_global_shortcut::Code::KeyG,
                'H' => tauri_plugin_global_shortcut::Code::KeyH,
                'I' => tauri_plugin_global_shortcut::Code::KeyI,
                'J' => tauri_plugin_global_shortcut::Code::KeyJ,
                'K' => tauri_plugin_global_shortcut::Code::KeyK,
                'L' => tauri_plugin_global_shortcut::Code::KeyL,
                'M' => tauri_plugin_global_shortcut::Code::KeyM,
                'N' => tauri_plugin_global_shortcut::Code::KeyN,
                'O' => tauri_plugin_global_shortcut::Code::KeyO,
                'P' => tauri_plugin_global_shortcut::Code::KeyP,
                'Q' => tauri_plugin_global_shortcut::Code::KeyQ,
                'R' => tauri_plugin_global_shortcut::Code::KeyR,
                'S' => tauri_plugin_global_shortcut::Code::KeyS,
                'T' => tauri_plugin_global_shortcut::Code::KeyT,
                'U' => tauri_plugin_global_shortcut::Code::KeyU,
                'V' => tauri_plugin_global_shortcut::Code::KeyV,
                'W' => tauri_plugin_global_shortcut::Code::KeyW,
                'X' => tauri_plugin_global_shortcut::Code::KeyX,
                'Y' => tauri_plugin_global_shortcut::Code::KeyY,
                'Z' => tauri_plugin_global_shortcut::Code::KeyZ,
                _ => return Err(format!("Unknown key: {}", c)),
            }
        }
        _ => return Err(format!("Unknown key: {}", code_part)),
    };
    Ok(Shortcut::new(Some(modifiers), code))
}
