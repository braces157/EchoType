# EchoType

EchoType is a Windows-first voice-to-text dictation overlay built with Tauri, React, and TypeScript.

![EchoType overlay](image1.png)

## Features

- Floating glass-style dictation overlay inspired by Windows 11.
- Microphone capture through the webview MediaRecorder API.
- Configurable language, global keybind, transcription mode, API key, and active-app insertion.
- Cloud transcription through OpenAI `gpt-4o-mini-transcribe`.
- Local transcription through `faster-whisper`.
- Clipboard-first transcript saving, with optional Windows text insertion using `SendInput`.
- Browser-safe preview mode for UI development without Tauri.

## Development

Install JavaScript dependencies:

```powershell
npm install
```

Run the browser preview:

```powershell
npm run dev
```

Run the Tauri app:

```powershell
npm run tauri:dev
```

Build the frontend:

```powershell
npm run build
```

Build the Windows app:

```powershell
npm run tauri:build
```

## Requirements

- Node.js and npm.
- Rust and Cargo for Tauri development.
- Windows WebView2 runtime.
- Python 3.10+ and `faster-whisper` for local transcription.
- An OpenAI API key for cloud transcription.

## Local Transcription

Install the local transcription dependency:

```powershell
python -m pip install faster-whisper
```

EchoType uses `base.en` for English and `base` for other languages by default. The model is downloaded once by `faster-whisper` and then reused from the local cache. To use a different local model size, set `ECHOTYPE_WHISPER_MODEL` before launching the app:

```powershell
$env:ECHOTYPE_WHISPER_MODEL = "tiny.en"
npm run tauri:dev
```

Useful local tuning options:

```powershell
# Faster, lower accuracy
$env:ECHOTYPE_WHISPER_MODEL = "tiny.en"
$env:ECHOTYPE_WHISPER_BEAM_SIZE = "1"
$env:ECHOTYPE_WHISPER_BATCH_SIZE = "8"

# Better English accuracy, slower
$env:ECHOTYPE_WHISPER_MODEL = "small.en"
$env:ECHOTYPE_WHISPER_BEAM_SIZE = "1"
$env:ECHOTYPE_WHISPER_BATCH_SIZE = "8"

# Try this if recordings include long silence
$env:ECHOTYPE_WHISPER_VAD = "1"
```

Use **Local only** in settings to avoid OpenAI API usage. Use **Hybrid** to try cloud first and fall back to the local model.

Local mode keeps a Python Whisper worker alive while the app is running. The first transcription after launch can still be slow because it loads the model, but later transcriptions reuse that loaded model.
