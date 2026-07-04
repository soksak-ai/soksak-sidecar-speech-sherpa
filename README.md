# soksak-sidecar-speech-sherpa

Speech service sidecar for soksak — sherpa-onnx TTS behind a stdio JSON-lines contract (`soksak-sidecar-speech-spec@1`).

Korean: [README.ko.md](README.ko.md)

## Protocol

One JSON request per line; one JSON reply per line. `op:"tts"` with `stream:true` emits chunk lines (`pcmBase64`, s16le mono) as synthesis progresses, then a `done` line. `op:"info"` reports spec/engine/sampleRate.

## Usage

```sh
soksak-sidecar-speech-sherpa --model-dir <dir> [--engine vits|kokoro]
```

The model directory is auto-scanned for sherpa conventions (`*.onnx`, `tokens.txt`, `lexicon.txt`, `espeak-ng-data/`, `dict/`, `voices.bin`).

## Models are not bundled

Download a sherpa-onnx compatible model yourself and check its license. Note: the Korean `vits-mimic3-ko_KO-kss_low` model is trained on the KSS dataset (CC BY-NC-SA — non-commercial). MIT-licensed MeloTTS-Korean requires a lexicon conversion, tracked as follow-up work.

## Build

```sh
cargo build --release   # dylibs land next to the binary; rpath is @loader_path
```
