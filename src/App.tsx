import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  CheckCircle,
  ClipboardText,
  Copy,
  GearSix,
  Keyboard,
  Microphone,
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
  autoInsert: true,
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
    if (command === "transcribe_audio") return { text: sampleTranscript, engine: "mock" } as T;
    if (command === "insert_text") return true as T;
    if (command === "copy_text") return true as T;
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
  const [isInsertBlocked, setIsInsertBlocked] = useState(false);
  const [hotkeyDraft, setHotkeyDraft] = useState(defaultSettings.hotkey);
  const recorderRef = useRef<MediaRecorder | null>(null);
  const chunksRef = useRef<BlobPart[]>([]);

  const statusLabel = useMemo(() => {
    if (listenState === "listening") return "Listening";
    if (listenState === "paused") return "Paused";
    if (listenState === "processing") return "Transcribing";
    if (listenState === "success") return "Inserted";
    if (listenState === "error") return "Needs attention";
    return "Ready";
  }, [listenState]);

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

  const stopRecorder = useCallback(() => {
    const recorder = recorderRef.current;
    if (recorder && recorder.state !== "inactive") {
      recorder.stop();
    }
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
        const base64Audio = await blobToBase64(audioBlob);
        const result = await invokeCommand<TranscriptResult>("transcribe_audio", {
          audioBase64: base64Audio,
          language: settings.language,
          mode: settings.transcriptionMode,
        });

        setTranscript(result.text);
        setEngine(result.engine);

        if (settings.autoInsert) {
          const inserted = await invokeCommand<boolean>("insert_text", { text: result.text });
          setIsInsertBlocked(!inserted);
          setListenState(inserted ? "success" : "error");
          if (!inserted) setError("Active app would not accept text. The transcript is ready to copy.");
        } else {
          setListenState("success");
        }
      } catch (cause) {
        setListenState("error");
        setError(cause instanceof Error ? cause.message : "Transcription failed.");
      }
    },
    [settings.autoInsert, settings.language, settings.transcriptionMode],
  );

  const startListening = useCallback(async () => {
    if (listenState === "listening") {
      stopRecorder();
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
      chunksRef.current = [];

      await refreshMicrophoneStatus();
      const stream = await navigator.mediaDevices.getUserMedia({
        audio: {
          echoCancellation: true,
          noiseSuppression: true,
          autoGainControl: true,
        },
      });
      const recorder = new MediaRecorder(stream);
      recorderRef.current = recorder;

      recorder.ondataavailable = (event) => {
        if (event.data.size > 0) chunksRef.current.push(event.data);
      };

      recorder.onstop = () => {
        stream.getTracks().forEach((track) => track.stop());
        const audioBlob = new Blob(chunksRef.current, { type: recorder.mimeType || "audio/webm" });
        void processAudio(audioBlob);
      };

      recorder.start();
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
      .then(({ listen }) => listen("hotkey-triggered", () => void startListening()))
      .then((cleanup) => {
        unlisten = cleanup;
      })
      .catch(() => undefined);

    return () => unlisten?.();
  }, [startListening]);

  const pauseListening = useCallback(() => {
    const recorder = recorderRef.current;
    if (!recorder) return;

    if (recorder.state === "recording") {
      recorder.pause();
      setListenState("paused");
      return;
    }

    if (recorder.state === "paused") {
      recorder.resume();
      setListenState("listening");
    }
  }, []);

  const reset = useCallback(() => {
    recorderRef.current?.stream.getTracks().forEach((track) => track.stop());
    setListenState("idle");
    setSeconds(0);
    setTranscript("");
    setError("");
    setIsInsertBlocked(false);
  }, []);

  const applyHotkeyDraft = useCallback(async () => {
    await saveSettings({ ...settings, hotkey: hotkeyDraft.trim() || defaultSettings.hotkey });
  }, [hotkeyDraft, saveSettings, settings]);

  const openSettings = useCallback(() => {
    setIsSettingsOpen(true);
    void refreshMicrophoneStatus();
  }, [refreshMicrophoneStatus]);

  return (
    <main className="desktop-shell">
      <section className="overlay-card" aria-label="EchoType dictation overlay">
        <header className="overlay-header">
          <div className="brand-lockup">
            <div className="brand-mark">
              <Microphone weight="fill" />
            </div>
            <strong>
              Echo<span>Type</span>
            </strong>
          </div>

          <div className={`status-pill ${listenState}`}>
            {statusIcon}
            <span>{statusLabel}</span>
          </div>

          <button className="shortcut-pill" type="button" onClick={openSettings}>
            {settings.hotkey}
          </button>

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
              {transcript || (listenState === "idle" ? "Press the shortcut or microphone to dictate..." : "Speak naturally. EchoType is capturing your voice.")}
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
            <span>{settings.autoInsert ? "Transcript sent to the active app." : "Transcript is ready."}</span>
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
                <small>When disabled, EchoType keeps the transcript in the overlay for copying.</small>
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

export default App;
