import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  CheckCircle,
  ClipboardText,
  Copy,
  GearSix,
  Keyboard,
  Microphone,
  Minus,
  Pause,
  Play,
  Stop,
  WarningCircle,
  X,
} from "@phosphor-icons/react";
import "./App.css";

type AppSettings = {
  language: string;
  hotkey: string;
  transcriptionMode: "hybrid" | "cloud" | "local";
  cloudProvider: "openai";
  apiKey: string;
  autoInsert: boolean;
};

type TranscriptResult = {
  text: string;
  engine: "cloud" | "local" | "mock";
};

type TranscribePayload = {
  audioBase64: string;
  mimeType: string;
  language: string;
  mode: AppSettings["transcriptionMode"];
};

type MicrophoneStatus = {
  available: boolean;
  secureContext: boolean;
  devices: number;
  permission: PermissionState | "unsupported" | "unknown";
  detail: string;
};

type ListenState = "idle" | "listening" | "paused" | "processing" | "success" | "error";

const defaultSettings: AppSettings = {
  language: "en-US",
  hotkey: "Alt+Space",
  transcriptionMode: "hybrid",
  cloudProvider: "openai",
  apiKey: "",
  autoInsert: false,
};

const languages = [
  { label: "English (US)", value: "en-US" },
  { label: "English (UK)", value: "en-GB" },
  { label: "Thai", value: "th-TH" },
  { label: "Auto detect", value: "auto" },
];

const sampleTranscript = "Schedule meeting at 10 AM and send the project notes after lunch.";

function isTauriRuntime() {
  return "__TAURI_INTERNALS__" in window;
}

async function invokeCommand<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauriRuntime()) {
    if (command === "load_settings") return defaultSettings as T;
    if (command === "save_settings") return undefined as T;
    if (command === "register_hotkey") return undefined as T;
    if (command === "reset_webview_permissions") return true as T;
    if (command === "minimize_overlay") return undefined as T;
    if (command === "set_overlay_compact") return undefined as T;
    if (command === "transcribe_audio") {
      const payload = (args as { request?: TranscribePayload } | undefined)?.request;
      if (!payload?.audioBase64) throw new Error("Recording was empty. No audio data was captured.");
      return { text: sampleTranscript, engine: "mock" } as T;
    }
    if (command === "insert_text") return true as T;
    if (command === "copy_text") {
      const text = (args as { text?: string } | undefined)?.text ?? "";
      await writeBrowserClipboard(text);
      return true as T;
    }
  }

  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(command, args);
}

function formatDuration(seconds: number) {
  const minutes = Math.floor(seconds / 60).toString().padStart(2, "0");
  const rest = (seconds % 60).toString().padStart(2, "0");
  return `${minutes}:${rest}`;
}

function App() {
  const [settings, setSettings] = useState<AppSettings>(defaultSettings);
  const [listenState, setListenState] = useState<ListenState>("idle");
  const [seconds, setSeconds] = useState(0);
  const [transcript, setTranscript] = useState("");
  const [engine, setEngine] = useState<TranscriptResult["engine"]>("mock");
  const [error, setError] = useState("");
  const [microphoneStatus, setMicrophoneStatus] = useState<MicrophoneStatus | null>(null);
  const [isSettingsOpen, setIsSettingsOpen] = useState(false);
  const [isCompact, setIsCompact] = useState(false);
  const [isInsertBlocked, setIsInsertBlocked] = useState(false);
  const [hotkeyDraft, setHotkeyDraft] = useState(defaultSettings.hotkey);
  const recorderRef = useRef<MediaRecorder | null>(null);
  const streamRef = useRef<MediaStream | null>(null);
  const sessionRef = useRef(0);
  const cancelSessionRef = useRef(false);

  const statusLabel = useMemo(() => {
    if (listenState === "listening") return isCompact ? "Listening..." : "Listening";
    if (listenState === "paused") return "Paused";
    if (listenState === "processing") return "Transcribing";
    if (listenState === "success") return settings.autoInsert ? "Inserted" : "Copied";
    if (listenState === "error") return "Needs attention";
    return "Ready";
  }, [isCompact, listenState, settings.autoInsert]);

  const statusIcon = listenState === "error" ? <WarningCircle weight="fill" /> : <span className="status-dot" />;

  const selectedLanguage = languages.find((item) => item.value === settings.language)?.label ?? settings.language;

  useEffect(() => {
    invokeCommand<AppSettings>("load_settings")
      .then((loaded) => {
        setSettings(loaded);
        setHotkeyDraft(loaded.hotkey);
      })
      .catch(() => setError("Settings could not be loaded. Defaults are active."));
  }, []);

  const refreshMicrophoneStatus = useCallback(async () => {
    const status: MicrophoneStatus = {
      available: Boolean(navigator.mediaDevices?.getUserMedia),
      secureContext: window.isSecureContext,
      devices: 0,
      permission: "unknown",
      detail: "",
    };

    try {
      if ("permissions" in navigator && "query" in navigator.permissions) {
        const permission = await navigator.permissions.query({ name: "microphone" as PermissionName });
        status.permission = permission.state;
      } else {
        status.permission = "unsupported";
      }
    } catch {
      status.permission = "unsupported";
    }

    try {
      if (navigator.mediaDevices?.enumerateDevices) {
        const devices = await navigator.mediaDevices.enumerateDevices();
        status.devices = devices.filter((device) => device.kind === "audioinput").length;
      }
    } catch (cause) {
      status.detail = cause instanceof Error ? cause.message : "Could not enumerate audio devices.";
    }

    setMicrophoneStatus(status);
    return status;
  }, []);

  useEffect(() => {
    if (listenState !== "listening") return;

    const interval = window.setInterval(() => setSeconds((value) => value + 1), 1000);
    return () => window.clearInterval(interval);
  }, [listenState]);

  useEffect(() => {
    invokeCommand("register_hotkey", { hotkey: settings.hotkey }).catch(() => {
      setError("This shortcut could not be registered. Choose another keybind in settings.");
      setListenState("error");
    });
  }, [settings.hotkey]);

  const saveSettings = useCallback(
    async (nextSettings: AppSettings) => {
      setSettings(nextSettings);
      await invokeCommand("save_settings", { settings: nextSettings });
    },
    [],
  );

  const setOverlayMode = useCallback(async (compact: boolean) => {
    setIsCompact(compact);
    if (isTauriRuntime()) {
      await invokeCommand("set_overlay_compact", { compact });
    }
  }, []);

  const stopRecorder = useCallback(() => {
    const recorder = recorderRef.current;
    if (recorder && recorder.state !== "inactive") {
      if (recorder.state === "recording") {
        recorder.requestData();
      }
      recorder.stop();
    }
  }, []);

  const minimizeOverlay = useCallback(async () => {
    await invokeCommand("minimize_overlay");
  }, []);

  const copyTranscript = useCallback(async () => {
    if (!transcript) return;
    try {
      await invokeCommand("copy_text", { text: transcript });
    } catch {
      await navigator.clipboard.writeText(transcript);
    }
  }, [transcript]);

  const processAudio = useCallback(
    async (audioBlob: Blob) => {
      setListenState("processing");
      setError("");

      try {
        if (audioBlob.size < 1200) {
          throw new Error("Recording was too small to transcribe. Speak for at least one second, then stop again.");
        }

        const base64Audio = await blobToBase64(audioBlob);
        const result = await invokeCommand<TranscriptResult>("transcribe_audio", {
          request: {
            audioBase64: base64Audio,
            mimeType: audioBlob.type || "audio/webm",
            language: settings.language,
            mode: settings.transcriptionMode,
          },
        });

        setTranscript(result.text);
        setEngine(result.engine);
        await copyTextToClipboard(result.text);

        if (settings.autoInsert) {
          const inserted = await invokeCommand<boolean>("insert_text", { text: result.text });
          setIsInsertBlocked(!inserted);
          setListenState(inserted ? "success" : "error");
          if (!inserted) setError("Saved to clipboard. Active app would not accept typed text.");
        } else {
          setListenState("success");
        }

        if (isCompact) void setOverlayMode(false);
      } catch (cause) {
        setListenState("error");
        if (isCompact) void setOverlayMode(false);
        setError(formatUnknownError(cause, "Transcription failed."));
      }
    },
    [isCompact, setOverlayMode, settings.autoInsert, settings.language, settings.transcriptionMode],
  );

  const startListening = useCallback(async () => {
    if (listenState === "listening") {
      stopRecorder();
      return;
    }

    if (listenState === "paused") {
      pauseRecorder(recorderRef.current);
      setListenState("listening");
      return;
    }

    try {
      if (!navigator.mediaDevices?.getUserMedia) {
        throw new Error("This WebView does not expose navigator.mediaDevices.getUserMedia. Run EchoType through Tauri or a secure localhost browser.");
      }

      if (typeof MediaRecorder === "undefined") {
        throw new Error("This WebView does not support MediaRecorder audio capture.");
      }

      setSeconds(0);
      setTranscript("");
      setError("");
      setIsInsertBlocked(false);
      cancelSessionRef.current = false;
      sessionRef.current += 1;
      const sessionId = sessionRef.current;
      const chunks: BlobPart[] = [];

      await refreshMicrophoneStatus();
      const stream = await navigator.mediaDevices.getUserMedia({
        audio: {
          echoCancellation: true,
          noiseSuppression: true,
          autoGainControl: true,
        },
      });
      const mimeType = getSupportedAudioMimeType();
      const recorder = new MediaRecorder(stream, mimeType ? { mimeType } : undefined);
      recorderRef.current = recorder;
      streamRef.current = stream;

      recorder.ondataavailable = (event) => {
        if (event.data.size > 0) chunks.push(event.data);
      };

      recorder.onerror = () => {
        cancelSessionRef.current = true;
        setListenState("error");
        setError("The recorder stopped because WebView2 reported a microphone capture error.");
      };

      recorder.onstop = () => {
        stream.getTracks().forEach((track) => track.stop());
        if (recorderRef.current === recorder) recorderRef.current = null;
        if (streamRef.current === stream) streamRef.current = null;
        if (cancelSessionRef.current || sessionRef.current !== sessionId) return;

        const audioBlob = new Blob(chunks, { type: recorder.mimeType || mimeType || "audio/webm" });
        void processAudio(audioBlob);
      };

      recorder.start(250);
      setListenState("listening");
    } catch (cause) {
      setListenState("error");
      void refreshMicrophoneStatus();
      setError(getMicrophoneErrorMessage(cause));
    }
  }, [listenState, processAudio, refreshMicrophoneStatus, stopRecorder]);

  const resetWebviewPermissions = useCallback(async () => {
    try {
      await invokeCommand("reset_webview_permissions");
      await refreshMicrophoneStatus();
      setError("WebView2 permission data was reset. Close EchoType completely, reopen it, then allow microphone access again.");
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Could not reset WebView2 permission data.");
    }
  }, [refreshMicrophoneStatus]);

  useEffect(() => {
    if (!isTauriRuntime()) return;

    let unlisten: (() => void) | undefined;
    import("@tauri-apps/api/event")
      .then(({ listen }) =>
        listen("hotkey-triggered", async () => {
          await setOverlayMode(true);
          await startListening();
        }),
      )
      .then((cleanup) => {
        unlisten = cleanup;
      })
      .catch(() => undefined);

    return () => unlisten?.();
  }, [setOverlayMode, startListening]);

  const pauseListening = useCallback(() => {
    if (pauseRecorder(recorderRef.current)) {
      setListenState("paused");
      return;
    }

    if (resumeRecorder(recorderRef.current)) {
      setListenState("listening");
    }
  }, []);

  const reset = useCallback(() => {
    void setOverlayMode(false);
    cancelSessionRef.current = true;
    sessionRef.current += 1;
    if (recorderRef.current && recorderRef.current.state !== "inactive") {
      recorderRef.current.stop();
    }
    streamRef.current?.getTracks().forEach((track) => track.stop());
    recorderRef.current = null;
    streamRef.current = null;
    setListenState("idle");
    setSeconds(0);
    setTranscript("");
    setError("");
    setIsInsertBlocked(false);
  }, [setOverlayMode]);

  const applyHotkeyDraft = useCallback(async () => {
    await saveSettings({ ...settings, hotkey: hotkeyDraft.trim() || defaultSettings.hotkey });
  }, [hotkeyDraft, saveSettings, settings]);

  const openSettings = useCallback(() => {
    void setOverlayMode(false);
    setIsSettingsOpen(true);
    void refreshMicrophoneStatus();
  }, [refreshMicrophoneStatus, setOverlayMode]);

  return (
    <main className={`desktop-shell ${isCompact ? "compact-shell" : ""}`}>
      <section className={`overlay-card ${isCompact ? "compact" : ""}`} aria-label="EchoType dictation overlay">
        <header className="overlay-header">
          <div className="brand-lockup">
            <div className="brand-mark">
              <img src="/app-logo.svg" alt="" />
            </div>
            <strong>
              Echo<span>Type</span>
            </strong>
          </div>

          <div className={`status-pill ${listenState}`}>
            {statusIcon}
            <span>{statusLabel}</span>
            {isCompact && listenState === "listening" && <i aria-hidden="true" />}
          </div>

          <button className="shortcut-pill" type="button" onClick={openSettings}>
            {settings.hotkey}
          </button>

          {!isCompact && (
            <button className="icon-button subtle" type="button" aria-label="Minimize overlay" onClick={minimizeOverlay}>
              <Minus weight="bold" />
            </button>
          )}

          <button className="icon-button subtle" type="button" aria-label="Close overlay" onClick={reset}>
            <X weight="bold" />
          </button>
        </header>

        <div className="overlay-body">
          <button
            className={`mic-button ${listenState}`}
            type="button"
            aria-label={listenState === "listening" ? "Stop listening" : "Start listening"}
            onClick={startListening}
          >
            <Microphone weight="fill" />
          </button>

          <div className="transcription-panel">
            <Waveform active={listenState === "listening" || listenState === "processing"} />
            <p className={`transcript-preview ${transcript ? "has-text" : ""}`}>
              {transcript || getPreviewText(listenState)}
            </p>
            <div className="meta-row">
              <span>{selectedLanguage}</span>
              <span>{formatDuration(seconds)}</span>
              <span>{engine === "mock" ? "Preview engine" : `${engine} engine`}</span>
            </div>
          </div>

          <div className="control-cluster">
            <button className="icon-button primary" type="button" aria-label="Stop and transcribe" onClick={stopRecorder}>
              <Stop weight="fill" />
            </button>
            <button className="icon-button" type="button" aria-label="Pause or resume listening" onClick={pauseListening}>
              {listenState === "paused" ? <Play weight="fill" /> : <Pause weight="fill" />}
            </button>
            <button className="icon-button" type="button" aria-label="Copy transcript" onClick={copyTranscript}>
              <Copy />
            </button>
            <button className="icon-button" type="button" aria-label="Open settings" onClick={openSettings}>
              <GearSix />
            </button>
          </div>
        </div>

        {(error || isInsertBlocked) && (
          <div className="notice error">
            <WarningCircle weight="fill" />
            <span>{error}</span>
            {transcript && (
              <button type="button" onClick={copyTranscript}>
                Copy text
              </button>
            )}
            {!transcript && (
              <button type="button" onClick={resetWebviewPermissions}>
                Reset mic access
              </button>
            )}
          </div>
        )}

        {listenState === "success" && !error && (
          <div className="notice success">
            <CheckCircle weight="fill" />
            <span>{settings.autoInsert ? "Transcript copied and sent to the active app." : "Transcript copied to clipboard."}</span>
          </div>
        )}
      </section>

      {isSettingsOpen && (
        <section className="settings-window" aria-label="EchoType settings">
          <header>
            <div>
              <p>Settings</p>
              <h1>EchoType controls</h1>
            </div>
            <button className="icon-button subtle" type="button" aria-label="Close settings" onClick={() => setIsSettingsOpen(false)}>
              <X weight="bold" />
            </button>
          </header>

          <div className="settings-grid">
            <label className="field">
              <span>Language</span>
              <select value={settings.language} onChange={(event) => void saveSettings({ ...settings, language: event.target.value })}>
                {languages.map((language) => (
                  <option key={language.value} value={language.value}>
                    {language.label}
                  </option>
                ))}
              </select>
            </label>

            <label className="field">
              <span>Transcription mode</span>
              <select
                value={settings.transcriptionMode}
                onChange={(event) => void saveSettings({ ...settings, transcriptionMode: event.target.value as AppSettings["transcriptionMode"] })}
              >
                <option value="hybrid">Hybrid: cloud then local</option>
                <option value="cloud">Cloud only</option>
                <option value="local">Local only</option>
              </select>
            </label>

            <label className="field wide">
              <span>Cloud API key</span>
              <input
                type="password"
                placeholder="Stored locally by the Tauri backend"
                value={settings.apiKey}
                onChange={(event) => void saveSettings({ ...settings, apiKey: event.target.value })}
              />
            </label>

            <label className="field wide">
              <span>Global keybind</span>
              <div className="hotkey-row">
                <Keyboard />
                <input value={hotkeyDraft} onChange={(event) => setHotkeyDraft(event.target.value)} placeholder="Alt+Space" />
                <button type="button" onClick={applyHotkeyDraft}>
                  Save keybind
                </button>
              </div>
            </label>

            <label className="toggle-row wide">
              <span>
                <strong>Type into active app</strong>
                <small>EchoType always saves to clipboard first. Enable this to also type into the active app.</small>
              </span>
              <input
                type="checkbox"
                checked={settings.autoInsert}
                onChange={(event) => void saveSettings({ ...settings, autoInsert: event.target.checked })}
              />
            </label>
          </div>

          <footer>
            <ClipboardText />
            <span>
              Microphone: {microphoneStatus?.permission ?? "unknown"} permission,
              {" "}
              {microphoneStatus?.devices ?? 0} input device
              {(microphoneStatus?.devices ?? 0) === 1 ? "" : "s"} detected.
            </span>
          </footer>
        </section>
      )}
    </main>
  );
}

function Waveform({ active }: { active: boolean }) {
  const bars = [16, 30, 42, 24, 52, 34, 46, 20, 38, 56, 28, 48, 32, 18, 40, 24, 50, 36, 22];

  return (
    <div className={`waveform ${active ? "active" : ""}`} aria-hidden="true">
      {bars.map((height, index) => (
        <span key={`${height}-${index}`} style={{ height: `${height}px`, animationDelay: `${index * 55}ms` }} />
      ))}
    </div>
  );
}

async function copyTextToClipboard(text: string) {
  try {
    await invokeCommand("copy_text", { text });
  } catch {
    await writeBrowserClipboard(text);
  }
}

async function writeBrowserClipboard(text: string) {
  if (!text) {
    throw new Error("There is no transcript text to copy.");
  }

  try {
    if (navigator.clipboard?.writeText && window.isSecureContext) {
      await navigator.clipboard.writeText(text);
      return;
    }
  } catch {
    // Fall back to the selection API below. Some browser previews reject async
    // clipboard writes after transcription because the user gesture has ended.
  }

  const textarea = document.createElement("textarea");
  textarea.value = text;
  textarea.setAttribute("readonly", "true");
  textarea.style.position = "fixed";
  textarea.style.left = "-9999px";
  textarea.style.top = "0";
  document.body.appendChild(textarea);
  textarea.focus();
  textarea.select();

  const copied = document.execCommand("copy");
  document.body.removeChild(textarea);

  if (!copied) {
    throw new Error("Clipboard write was blocked by the browser. Use the Copy button after transcription.");
  }
}

function getSupportedAudioMimeType() {
  const candidates = ["audio/webm;codecs=opus", "audio/webm", "audio/mp4", "audio/wav"];
  return candidates.find((candidate) => MediaRecorder.isTypeSupported(candidate)) ?? "";
}

function pauseRecorder(recorder: MediaRecorder | null) {
  if (!recorder || recorder.state !== "recording") return false;
  recorder.pause();
  return true;
}

function resumeRecorder(recorder: MediaRecorder | null) {
  if (!recorder || recorder.state !== "paused") return false;
  recorder.resume();
  return true;
}

function getPreviewText(listenState: ListenState) {
  if (listenState === "idle") return "Press the shortcut or microphone to dictate...";
  if (listenState === "processing") return "Transcribing your recording...";
  if (listenState === "error") return "Review the message below, then try again.";
  if (listenState === "paused") return "Recording is paused.";
  return "Speak naturally. EchoType is capturing your voice.";
}

async function blobToBase64(blob: Blob) {
  const buffer = await blob.arrayBuffer();
  let binary = "";
  const bytes = new Uint8Array(buffer);
  for (let index = 0; index < bytes.byteLength; index += 1) {
    binary += String.fromCharCode(bytes[index]);
  }
  return window.btoa(binary);
}

function getMicrophoneErrorMessage(cause: unknown) {
  if (!(cause instanceof Error)) {
    return "Microphone capture failed for an unknown reason.";
  }

  if (cause.name === "NotAllowedError" || cause.name === "SecurityError") {
    return "Microphone access is still blocked by WebView2 or Windows privacy settings. Use Reset mic access, restart EchoType, then allow the prompt again.";
  }

  if (cause.name === "NotFoundError" || cause.name === "DevicesNotFoundError") {
    return "No microphone input device was found. Check Windows sound input settings, then try again.";
  }

  if (cause.name === "NotReadableError" || cause.name === "TrackStartError") {
    return "EchoType can see the microphone, but Windows would not start it. Close other apps using the mic and try again.";
  }

  if (cause.name === "OverconstrainedError") {
    return "The selected microphone constraints are not supported by this device.";
  }

  return `${cause.name}: ${cause.message}`;
}

function formatUnknownError(cause: unknown, fallback: string) {
  if (cause instanceof Error) return cause.message;
  if (typeof cause === "string") return cause;
  try {
    return JSON.stringify(cause);
  } catch {
    return fallback;
  }
}

export default App;
