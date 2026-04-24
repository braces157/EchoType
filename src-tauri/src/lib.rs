use arboard::Clipboard;
use base64::{engine::general_purpose, Engine as _};
use reqwest::blocking::multipart;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::PathBuf,
    sync::Mutex,
};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

#[cfg(target_os = "windows")]
use windows::Win32::{
    Foundation::{HWND, LPARAM, WPARAM},
    UI::{
        Input::KeyboardAndMouse::{
            SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
            KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, VIRTUAL_KEY,
        },
        WindowsAndMessaging::{
            GetForegroundWindow, PostMessageW, SetForegroundWindow, ShowWindow, SW_RESTORE,
            WM_CHAR,
        },
    },
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppSettings {
    language: String,
    hotkey: String,
    transcription_mode: TranscriptionMode,
    cloud_provider: String,
    api_key: String,
    auto_insert: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum TranscriptionMode {
    Hybrid,
    Cloud,
    Local,
}

#[derive(Debug, Serialize)]
struct TranscriptResult {
    text: String,
    engine: &'static str,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TranscribeRequest {
    audio_base64: String,
    language: String,
    mode: TranscriptionMode,
}

#[derive(Default)]
struct AppState {
    target_window: Mutex<Option<isize>>,
    current_shortcut: Mutex<Option<Shortcut>>,
}

pub fn run() {
    tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state == ShortcutState::Pressed {
                        #[cfg(target_os = "windows")]
                        {
                            let hwnd = unsafe { GetForegroundWindow() };
                            let state = app.state::<AppState>();
                            let mut target = state.target_window.lock().ok();
                            if let Some(target) = target.as_mut() {
                                target.replace(hwnd.0 as isize);
                            };
                        }
                        let _ = app.emit("hotkey-triggered", ());
                    }
                })
                .build(),
        )
        .manage(AppState::default())
        .setup(|app| {
            let window = app.get_webview_window("main").expect("main window missing");
            let _ = window.set_always_on_top(true);
            let _ = window.set_decorations(false);
            let _ = window.set_shadow(true);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            load_settings,
            save_settings,
            register_hotkey,
            capture_active_window,
            reset_webview_permissions,
            transcribe_audio,
            insert_text,
            copy_text
        ])
        .run(tauri::generate_context!())
        .expect("error while running EchoType");
}

#[tauri::command]
fn load_settings() -> Result<AppSettings, String> {
    let path = settings_path()?;
    if !path.exists() {
        return Ok(default_settings());
    }

    let contents = fs::read_to_string(path).map_err(|error| error.to_string())?;
    serde_json::from_str(&contents).map_err(|error| error.to_string())
}

#[tauri::command]
fn save_settings(settings: AppSettings) -> Result<(), String> {
    let path = settings_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }

    let contents = serde_json::to_string_pretty(&settings).map_err(|error| error.to_string())?;
    fs::write(path, contents).map_err(|error| error.to_string())
}

#[tauri::command]
fn register_hotkey(app: AppHandle, state: State<AppState>, hotkey: String) -> Result<(), String> {
    let shortcut = parse_shortcut(&hotkey)?;
    let manager = app.global_shortcut();
    let mut current = state
        .current_shortcut
        .lock()
        .map_err(|_| "Shortcut state could not be locked.".to_string())?;

    if let Some(existing) = current.take() {
        let _ = manager.unregister(existing);
    }

    manager.register(shortcut).map_err(|error| error.to_string())?;
    *current = Some(shortcut);
    Ok(())
}

#[tauri::command]
fn capture_active_window(state: State<AppState>) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let hwnd = unsafe { GetForegroundWindow() };
        let mut target = state
            .target_window
            .lock()
            .map_err(|_| "Window state could not be locked.".to_string())?;
        target.replace(hwnd.0 as isize);
        Ok(())
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = state;
        Ok(())
    }
}

#[tauri::command]
fn reset_webview_permissions() -> Result<bool, String> {
    let preferences_path = webview_preferences_path()?;
    if !preferences_path.exists() {
        return Ok(false);
    }

    let contents = fs::read_to_string(&preferences_path).map_err(|error| error.to_string())?;
    let mut preferences: serde_json::Value =
        serde_json::from_str(&contents).map_err(|error| error.to_string())?;

    if let Some(exceptions) = preferences
        .pointer_mut("/profile/content_settings/exceptions")
        .and_then(|value| value.as_object_mut())
    {
        exceptions.remove("media_stream_mic");
        exceptions.remove("media_stream_camera");
    }

    if let Some(actions) = preferences
        .pointer_mut("/profile/content_settings/permission_actions")
        .and_then(|value| value.as_object_mut())
    {
        actions.remove("mic_stream");
        actions.remove("camera_stream");
    }

    let updated = serde_json::to_string(&preferences).map_err(|error| error.to_string())?;
    fs::write(preferences_path, updated).map_err(|error| error.to_string())?;
    Ok(true)
}

#[tauri::command]
fn transcribe_audio(request: TranscribeRequest) -> Result<TranscriptResult, String> {
    let settings = load_settings().unwrap_or_else(|_| default_settings());

    match request.mode {
        TranscriptionMode::Cloud => transcribe_with_openai(&request, &settings.api_key),
        TranscriptionMode::Local => transcribe_with_local_windows(),
        TranscriptionMode::Hybrid => match transcribe_with_openai(&request, &settings.api_key) {
            Ok(result) => Ok(result),
            Err(_) => transcribe_with_local_windows(),
        },
    }
}

#[tauri::command]
fn insert_text(state: State<AppState>, text: String) -> Result<bool, String> {
    #[cfg(target_os = "windows")]
    {
        let target = state
            .target_window
            .lock()
            .map_err(|_| "Window state could not be locked.".to_string())?
            .to_owned();

        if let Some(raw_hwnd) = target {
            let hwnd = HWND(raw_hwnd as _);
            unsafe {
                let _ = ShowWindow(hwnd, SW_RESTORE);
                let _ = SetForegroundWindow(hwnd);
            }
        }

        if send_unicode_text(&text) {
            return Ok(true);
        }

        copy_text(text)?;
        Ok(false)
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = state;
        copy_text(text)?;
        Ok(false)
    }
}

#[tauri::command]
fn copy_text(text: String) -> Result<bool, String> {
    let mut clipboard = Clipboard::new().map_err(|error| error.to_string())?;
    clipboard
        .set_text(text)
        .map(|_| true)
        .map_err(|error| error.to_string())
}

fn transcribe_with_openai(
    request: &TranscribeRequest,
    api_key: &str,
) -> Result<TranscriptResult, String> {
    if api_key.trim().is_empty() {
        return Err("Add an OpenAI API key in settings to use cloud transcription.".to_string());
    }

    let audio_bytes = general_purpose::STANDARD
        .decode(&request.audio_base64)
        .map_err(|error| error.to_string())?;

    let mut form = multipart::Form::new()
        .text("model", "gpt-4o-mini-transcribe")
        .text("response_format", "json")
        .part(
            "file",
            multipart::Part::bytes(audio_bytes)
                .file_name("echotype-recording.webm")
                .mime_str("audio/webm")
                .map_err(|error| error.to_string())?,
        );

    if request.language != "auto" {
        form = form.text("language", request.language.clone());
    }

    let response = reqwest::blocking::Client::new()
        .post("https://api.openai.com/v1/audio/transcriptions")
        .bearer_auth(api_key.trim())
        .multipart(form)
        .send()
        .map_err(|error| error.to_string())?;

    if !response.status().is_success() {
        return Err(format!("Cloud transcription failed with status {}.", response.status()));
    }

    let payload: serde_json::Value = response.json().map_err(|error| error.to_string())?;
    let text = payload
        .get("text")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .trim()
        .to_string();

    if text.is_empty() {
        return Err("Cloud transcription returned no text.".to_string());
    }

    Ok(TranscriptResult {
        text,
        engine: "cloud",
    })
}

fn transcribe_with_local_windows() -> Result<TranscriptResult, String> {
    Err(
        "Local Windows speech fallback is not available in this build. Add an API key or use cloud mode."
            .to_string(),
    )
}

fn settings_path() -> Result<PathBuf, String> {
    let base = dirs::config_dir().ok_or_else(|| "Could not locate the user config folder.".to_string())?;
    Ok(base.join("EchoType").join("settings.json"))
}

fn webview_preferences_path() -> Result<PathBuf, String> {
    let base = dirs::data_local_dir()
        .ok_or_else(|| "Could not locate the local app data folder.".to_string())?;
    Ok(base
        .join("com.echotype.desktop")
        .join("EBWebView")
        .join("Default")
        .join("Preferences"))
}

fn default_settings() -> AppSettings {
    AppSettings {
        language: "en-US".to_string(),
        hotkey: "Alt+Space".to_string(),
        transcription_mode: TranscriptionMode::Hybrid,
        cloud_provider: "openai".to_string(),
        api_key: String::new(),
        auto_insert: true,
    }
}

fn parse_shortcut(hotkey: &str) -> Result<Shortcut, String> {
    let mut modifiers = Modifiers::empty();
    let mut code = None;

    for token in hotkey.split('+').map(|part| part.trim().to_lowercase()) {
        match token.as_str() {
            "ctrl" | "control" => modifiers |= Modifiers::CONTROL,
            "alt" => modifiers |= Modifiers::ALT,
            "shift" => modifiers |= Modifiers::SHIFT,
            "cmd" | "win" | "super" => modifiers |= Modifiers::SUPER,
            "space" => code = Some(Code::Space),
            "enter" => code = Some(Code::Enter),
            "tab" => code = Some(Code::Tab),
            "f1" => code = Some(Code::F1),
            "f2" => code = Some(Code::F2),
            "f3" => code = Some(Code::F3),
            "f4" => code = Some(Code::F4),
            "f5" => code = Some(Code::F5),
            "f6" => code = Some(Code::F6),
            "f7" => code = Some(Code::F7),
            "f8" => code = Some(Code::F8),
            "f9" => code = Some(Code::F9),
            "f10" => code = Some(Code::F10),
            "f11" => code = Some(Code::F11),
            "f12" => code = Some(Code::F12),
            single if single.len() == 1 => {
                let character = single.chars().next().unwrap().to_ascii_uppercase();
                code = match character {
                    'A' => Some(Code::KeyA),
                    'B' => Some(Code::KeyB),
                    'C' => Some(Code::KeyC),
                    'D' => Some(Code::KeyD),
                    'E' => Some(Code::KeyE),
                    'F' => Some(Code::KeyF),
                    'G' => Some(Code::KeyG),
                    'H' => Some(Code::KeyH),
                    'I' => Some(Code::KeyI),
                    'J' => Some(Code::KeyJ),
                    'K' => Some(Code::KeyK),
                    'L' => Some(Code::KeyL),
                    'M' => Some(Code::KeyM),
                    'N' => Some(Code::KeyN),
                    'O' => Some(Code::KeyO),
                    'P' => Some(Code::KeyP),
                    'Q' => Some(Code::KeyQ),
                    'R' => Some(Code::KeyR),
                    'S' => Some(Code::KeyS),
                    'T' => Some(Code::KeyT),
                    'U' => Some(Code::KeyU),
                    'V' => Some(Code::KeyV),
                    'W' => Some(Code::KeyW),
                    'X' => Some(Code::KeyX),
                    'Y' => Some(Code::KeyY),
                    'Z' => Some(Code::KeyZ),
                    _ => None,
                };
            }
            _ => return Err(format!("Unsupported shortcut token: {token}.")),
        }
    }

    let key = code.ok_or_else(|| "Shortcut needs a key, such as Alt+Space.".to_string())?;
    Ok(Shortcut::new(Some(modifiers), key))
}

#[cfg(target_os = "windows")]
fn send_unicode_text(text: &str) -> bool {
    let mut inputs = Vec::new();

    for unit in text.encode_utf16() {
        inputs.push(INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(0),
                    wScan: unit,
                    dwFlags: KEYEVENTF_UNICODE,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        });
        inputs.push(INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(0),
                    wScan: unit,
                    dwFlags: KEYBD_EVENT_FLAGS(KEYEVENTF_UNICODE.0 | KEYEVENTF_KEYUP.0),
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        });
    }

    if inputs.is_empty() {
        return true;
    }

    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent == inputs.len() as u32 {
        return true;
    }

    post_chars(text)
}

#[cfg(target_os = "windows")]
fn post_chars(text: &str) -> bool {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return false;
    }

    for unit in text.encode_utf16() {
        let result = unsafe { PostMessageW(Some(hwnd), WM_CHAR, WPARAM(unit as usize), LPARAM(0)) };
        if result.is_err() {
            return false;
        }
    }

    true
}
