# EchoType

EchoType is a Windows-first voice-to-text dictation overlay built with Tauri, React, and TypeScript.

## Features

- Floating glass-style dictation overlay inspired by Windows 11.
- Microphone capture through the webview MediaRecorder API.
- Configurable language, global keybind, transcription mode, API key, and active-app insertion.
- Cloud transcription through OpenAI `gpt-4o-mini-transcribe`.
- Windows text insertion path using `SendInput`, with clipboard fallback.
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
- An OpenAI API key for cloud transcription.

## Current Limitation

The local Windows speech fallback is wired as a backend fallback point, but full offline speech recognition is not implemented yet. Hybrid mode currently attempts OpenAI transcription first and reports a clear fallback error if cloud transcription is unavailable.
