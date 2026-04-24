use arboard::Clipboard;
use base64::{engine::general_purpose, Engine as _};
use reqwest::blocking::multipart;
use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{menu::MenuBuilder, tray::TrayIconBuilder};
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Position, Size, State};
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
            GetForegroundWindow, PostMessageW, SetForegroundWindow, ShowWindow, SW_RESTORE, WM_CHAR,
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
    mime_type: String,
    language: String,
    mode: TranscriptionMode,
}

#[derive(Default)]
struct AppState {
    target_window: Mutex<Option<isize>>,
    current_shortcut: Mutex<Option<Shortcut>>,
    local_worker: Mutex<Option<LocalWhisperWorker>>,
}

struct LocalWhisperWorker {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
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
                        let _ = restore_overlay_window(app);
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
            setup_tray_menu(app)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            load_settings,
            save_settings,
            register_hotkey,
            capture_active_window,
            reset_webview_permissions,
            minimize_overlay,
            set_overlay_compact,
            transcribe_audio,
            insert_text,
            copy_text
        ])
        .run(tauri::generate_context!())
        .expect("error while running EchoType");
}

fn setup_tray_menu(app: &mut tauri::App) -> tauri::Result<()> {
    let tray_menu = MenuBuilder::new(app)
        .text("show", "Show EchoType")
        .text("minimize", "Minimize")
        .separator()
        .text("quit", "Quit")
        .build()?;

    let mut tray_builder = TrayIconBuilder::with_id("echotype-tray")
        .tooltip("EchoType")
        .menu(&tray_menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| {
            if event.id() == "show" {
                let _ = restore_overlay_window(app);
            } else if event.id() == "minimize" {
                let _ = minimize_main_window(app);
            } else if event.id() == "quit" {
                app.exit(0);
            }
        });

    if let Some(icon) = app.default_window_icon().cloned() {
        tray_builder = tray_builder.icon(icon);
    }

    tray_builder.build(app)?;
    Ok(())
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

    manager
        .register(shortcut)
        .map_err(|error| error.to_string())?;
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
fn minimize_overlay(app: AppHandle) -> Result<(), String> {
    minimize_main_window(&app)
}

#[tauri::command]
fn set_overlay_compact(app: AppHandle, compact: bool) -> Result<(), String> {
    let window = restore_overlay_window(&app)?;
    let size = if compact {
        LogicalSize {
            width: 380.0,
            height: 78.0,
        }
    } else {
        LogicalSize {
            width: 920.0,
            height: 420.0,
        }
    };

    window
        .set_size(Size::Logical(size))
        .map_err(|error| error.to_string())?;

    if compact {
        position_compact_overlay(&window)?;
    } else {
        window.center().map_err(|error| error.to_string())?;
    }

    Ok(())
}

#[tauri::command]
fn transcribe_audio(
    request: TranscribeRequest,
    state: State<AppState>,
) -> Result<TranscriptResult, String> {
    let settings = load_settings().unwrap_or_else(|_| default_settings());

    match request.mode {
        TranscriptionMode::Cloud => transcribe_with_openai(&request, &settings.api_key),
        TranscriptionMode::Local => transcribe_with_local_whisper(&request, &state),
        TranscriptionMode::Hybrid => match transcribe_with_openai(&request, &settings.api_key) {
            Ok(result) => Ok(result),
            Err(cloud_error) => match transcribe_with_local_whisper(&request, &state) {
                Ok(result) => Ok(result),
                Err(local_error) => Err(format!(
                    "Cloud transcription failed: {cloud_error}. Local fallback failed: {local_error}"
                )),
            },
        },
    }
}

fn restore_overlay_window(app: &AppHandle) -> Result<tauri::WebviewWindow, String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "Main window was not found.".to_string())?;
    window.show().map_err(|error| error.to_string())?;
    window.unminimize().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())?;
    Ok(window)
}

fn minimize_main_window(app: &AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "Main window was not found.".to_string())?;
    window.minimize().map_err(|error| error.to_string())
}

fn position_compact_overlay(window: &tauri::WebviewWindow) -> Result<(), String> {
    let Some(monitor) = window
        .current_monitor()
        .map_err(|error| error.to_string())?
    else {
        return window.center().map_err(|error| error.to_string());
    };

    let scale_factor = monitor.scale_factor();
    let monitor_size = monitor.size();
    let monitor_position = monitor.position();
    let width = 380.0;
    let top_offset = 176.0;
    let monitor_width = monitor_size.width as f64 / scale_factor;
    let monitor_x = monitor_position.x as f64 / scale_factor;
    let monitor_y = monitor_position.y as f64 / scale_factor;
    let x = monitor_x + ((monitor_width - width) / 2.0).max(0.0);
    let y = monitor_y + top_offset;

    window
        .set_position(Position::Logical(LogicalPosition { x, y }))
        .map_err(|error| error.to_string())
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

    if audio_bytes.is_empty() {
        return Err("Recording was empty. No audio bytes were received.".to_string());
    }

    let mut form = multipart::Form::new()
        .text("model", "gpt-4o-mini-transcribe")
        .text("response_format", "json")
        .part("file", audio_part(audio_bytes, &request.mime_type)?);

    if let Some(language) = openai_language_code(&request.language) {
        form = form.text("language", language);
    }

    let response = reqwest::blocking::Client::new()
        .post("https://api.openai.com/v1/audio/transcriptions")
        .bearer_auth(api_key.trim())
        .multipart(form)
        .send()
        .map_err(|error| error.to_string())?;

    let status = response.status();
    if !status.is_success() {
        let body = response
            .text()
            .unwrap_or_else(|_| "No error body returned.".to_string());
        return Err(format!("status {status}: {body}"));
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

fn transcribe_with_local_whisper(
    request: &TranscribeRequest,
    state: &State<AppState>,
) -> Result<TranscriptResult, String> {
    let audio_bytes = general_purpose::STANDARD
        .decode(&request.audio_base64)
        .map_err(|error| error.to_string())?;

    if audio_bytes.is_empty() {
        return Err("Recording was empty. No audio bytes were received.".to_string());
    }

    let extension = audio_extension(&request.mime_type);
    let audio_path = temp_audio_path(extension)?;
    fs::write(&audio_path, audio_bytes).map_err(|error| error.to_string())?;

    let result = run_local_whisper_worker(state, &audio_path, &request.language);
    let _ = fs::remove_file(&audio_path);
    result
}

fn run_local_whisper_worker(
    state: &State<AppState>,
    audio_path: &Path,
    language: &str,
) -> Result<TranscriptResult, String> {
    let mut worker = state
        .local_worker
        .lock()
        .map_err(|_| "Local transcription worker state could not be locked.".to_string())?;

    for attempt in 0..2 {
        if worker.is_none() {
            *worker = Some(start_local_whisper_worker()?);
        }

        let Some(active_worker) = worker.as_mut() else {
            continue;
        };

        match active_worker.transcribe(audio_path, language) {
            Ok(result) => return Ok(result),
            Err(error) if attempt == 0 => {
                if let Some(mut stale_worker) = worker.take() {
                    let _ = stale_worker.child.kill();
                }
                eprintln!("Restarting local transcription worker after error: {error}");
            }
            Err(error) => return Err(error),
        }
    }

    Err("Local transcription worker could not be started.".to_string())
}

fn start_local_whisper_worker() -> Result<LocalWhisperWorker, String> {
    let script_path = local_transcribe_script_path();
    if !script_path.exists() {
        return Err(format!(
            "Local transcription script was not found at {}.",
            script_path.display()
        ));
    }

    let mut last_error = None;
    for python in ["py", "python"] {
        let mut command = Command::new(python);
        if python == "py" {
            command.arg("-3");
        }

        let child = command
            .arg(&script_path)
            .arg("--worker")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();

        match child {
            Ok(mut child) => {
                let stdin = child
                    .stdin
                    .take()
                    .ok_or_else(|| "Local worker stdin was unavailable.".to_string())?;
                let stdout = child
                    .stdout
                    .take()
                    .ok_or_else(|| "Local worker stdout was unavailable.".to_string())?;
                let mut worker = LocalWhisperWorker {
                    child,
                    stdin,
                    stdout: BufReader::new(stdout),
                };

                worker.read_ready()?;
                return Ok(worker);
            }
            Err(error) => {
                last_error = Some(error.to_string());
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        "Python is required for local transcription, but no Python runner was found.".to_string()
    }))
}

impl LocalWhisperWorker {
    fn read_ready(&mut self) -> Result<(), String> {
        let response = self.read_json_line()?;
        if response
            .get("ready")
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
        {
            return Ok(());
        }

        Err(format!(
            "Local worker returned unexpected startup response: {response}"
        ))
    }

    fn transcribe(
        &mut self,
        audio_path: &Path,
        language: &str,
    ) -> Result<TranscriptResult, String> {
        let request = serde_json::json!({
            "audioFile": audio_path,
            "language": local_language_code(language),
        });

        writeln!(self.stdin, "{request}").map_err(|error| error.to_string())?;
        self.stdin.flush().map_err(|error| error.to_string())?;

        let response = self.read_json_line()?;
        if response
            .get("ok")
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
        {
            let text = response
                .get("text")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .trim()
                .to_string();

            if text.is_empty() {
                return Err("Local transcription returned no text.".to_string());
            }

            return Ok(TranscriptResult {
                text,
                engine: "local",
            });
        }

        Err(response
            .get("error")
            .and_then(|value| value.as_str())
            .unwrap_or("Local transcription failed.")
            .to_string())
    }

    fn read_json_line(&mut self) -> Result<serde_json::Value, String> {
        let mut line = String::new();
        let bytes_read = self
            .stdout
            .read_line(&mut line)
            .map_err(|error| error.to_string())?;

        if bytes_read == 0 {
            return Err("Local transcription worker exited unexpectedly.".to_string());
        }

        serde_json::from_str(line.trim()).map_err(|error| error.to_string())
    }
}

fn local_transcribe_script_path() -> PathBuf {
    if let Ok(path) = env::var("ECHOTYPE_LOCAL_TRANSCRIBE") {
        return PathBuf::from(path);
    }

    let relative_script = Path::new("scripts").join("local-transcribe.py");
    let mut candidates = vec![relative_script.clone()];

    if let Ok(current_dir) = env::current_dir() {
        candidates.push(current_dir.join(&relative_script));
        candidates.push(current_dir.join("..").join(&relative_script));
    }

    if let Ok(exe_path) = env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            candidates.push(exe_dir.join(&relative_script));
            candidates.push(
                exe_dir
                    .join("..")
                    .join("..")
                    .join("..")
                    .join(&relative_script),
            );
        }
    }

    candidates
        .into_iter()
        .find(|path| path.exists())
        .unwrap_or(relative_script)
}

fn audio_part(audio_bytes: Vec<u8>, mime_type: &str) -> Result<multipart::Part, String> {
    let normalized = if mime_type.trim().is_empty() {
        "audio/webm"
    } else {
        mime_type.trim()
    };

    let extension = audio_extension(normalized);

    multipart::Part::bytes(audio_bytes)
        .file_name(format!("echotype-recording.{extension}"))
        .mime_str(normalized)
        .map_err(|error| error.to_string())
}

fn audio_extension(mime_type: &str) -> &'static str {
    match mime_type
        .trim()
        .split(';')
        .next()
        .unwrap_or(mime_type.trim())
    {
        "audio/webm" => "webm",
        "audio/mp4" => "mp4",
        "audio/mpeg" => "mp3",
        "audio/mp3" => "mp3",
        "audio/wav" => "wav",
        "audio/x-wav" => "wav",
        "audio/ogg" => "ogg",
        _ => "webm",
    }
}

fn temp_audio_path(extension: &str) -> Result<PathBuf, String> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_millis();
    Ok(env::temp_dir().join(format!("echotype-recording-{timestamp}.{extension}")))
}

fn local_language_code(language: &str) -> String {
    match language {
        "auto" => "auto".to_string(),
        "en-US" | "en-GB" => "en".to_string(),
        "th-TH" => "th".to_string(),
        other => other
            .split(['-', '_'])
            .next()
            .filter(|value| !value.is_empty())
            .unwrap_or("auto")
            .to_string(),
    }
}

fn openai_language_code(language: &str) -> Option<String> {
    match language {
        "auto" => None,
        "en-US" | "en-GB" => Some("en".to_string()),
        "th-TH" => Some("th".to_string()),
        other => other
            .split(['-', '_'])
            .next()
            .filter(|value| !value.is_empty())
            .map(str::to_string),
    }
}

fn settings_path() -> Result<PathBuf, String> {
    let base =
        dirs::config_dir().ok_or_else(|| "Could not locate the user config folder.".to_string())?;
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
        auto_insert: false,
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
