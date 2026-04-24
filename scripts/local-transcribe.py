#!/usr/bin/env python3
import argparse
import json
import os
import sys

_MODEL = None
_MODEL_NAME = None


def env_int(name: str, default: int) -> int:
    raw_value = os.environ.get(name)
    if raw_value is None:
        return default
    try:
        return int(raw_value)
    except ValueError:
        return default


def default_model(language: str) -> str:
    if language in {"en", "en-US", "en-GB"}:
        return "base.en"
    return "base"


def load_model(model_name: str):
    global _MODEL
    global _MODEL_NAME

    if _MODEL is not None and _MODEL_NAME == model_name:
        return _MODEL

    from faster_whisper import WhisperModel

    _MODEL = WhisperModel(
        model_name,
        device="auto",
        compute_type=os.environ.get("ECHOTYPE_WHISPER_COMPUTE_TYPE", "int8"),
        cpu_threads=env_int("ECHOTYPE_WHISPER_CPU_THREADS", 0),
    )
    _MODEL_NAME = model_name
    return _MODEL


def transcribe(audio_file: str, language_arg: str) -> str:
    model_name = os.environ.get("ECHOTYPE_WHISPER_MODEL", default_model(language_arg))
    language = None if language_arg == "auto" else language_arg
    beam_size = env_int("ECHOTYPE_WHISPER_BEAM_SIZE", 1)
    vad_filter = os.environ.get("ECHOTYPE_WHISPER_VAD", "0") == "1"

    model = load_model(model_name)
    segments, _info = model.transcribe(
        audio_file,
        language=language,
        beam_size=beam_size,
        vad_filter=vad_filter,
    )
    return " ".join(segment.text.strip() for segment in segments).strip()


def worker() -> int:
    print(json.dumps({"ready": True}), flush=True)

    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue

        try:
            request = json.loads(line)
            text = transcribe(request["audioFile"], request.get("language", "auto"))
            print(json.dumps({"ok": True, "text": text}, ensure_ascii=False), flush=True)
        except Exception as error:
            print(json.dumps({"ok": False, "error": str(error)}), flush=True)

    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description="Transcribe an audio file with local Whisper.")
    parser.add_argument("audio_file", nargs="?")
    parser.add_argument("--language", default="auto")
    parser.add_argument("--worker", action="store_true")
    args = parser.parse_args()

    try:
        import faster_whisper  # noqa: F401
    except ImportError:
        print(
            "Missing local dependency: install with `python -m pip install faster-whisper`.",
            file=sys.stderr,
        )
        return 2

    if args.worker:
        return worker()

    if not args.audio_file:
        parser.error("audio_file is required unless --worker is used")

    try:
        text = transcribe(args.audio_file, args.language)
    except Exception as error:
        print(f"Local Whisper transcription failed: {error}", file=sys.stderr)
        return 1

    print(json.dumps({"text": text}, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
