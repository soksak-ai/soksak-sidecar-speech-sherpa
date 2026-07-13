# soksak-sidecar-speech-sherpa

soksak 음성 service 사이드카 — Supertonic / sherpa-onnx(VITS·Kokoro) TTS 를 stdio JSON-lines 계약(`soksak-spec-sidecar-speech@1`) 뒤에 둔다.

English: [README.md](README.md)

## 프로토콜

한 줄 JSON 요청 ↔ 한 줄 JSON 응답. `op:"tts"` + `stream:true` 는 합성 진행에 따라 청크 줄(`pcmBase64`, s16le 모노)을 흘리고 `done` 줄로 종결한다. `op:"info"` 는 spec/엔진/샘플레이트 보고.

## 사용

```sh
soksak-sidecar-speech-sherpa --model-dir <dir> [--engine vits|kokoro|supertonic]
```

sherpa 엔진은 모델 디렉토리를 관례(`*.onnx`, `tokens.txt`, `lexicon.txt`, `espeak-ng-data/`, `dict/`, `voices.bin`)로 자동 탐지한다. `supertonic` 엔진은 `onnx/` + `voice_styles/` 구조를 기대한다(화자 번호=스타일 선택, 요청에 `lang` 동반, Supertonic 3 는 인라인 `<laugh>`/`<breath>`/`<sigh>` 태그를 발성으로 렌더).

## 모델은 동봉하지 않는다

sherpa-onnx 호환 모델을 직접 받고 라이선스를 확인하라. 참고: 한국어 `vits-mimic3-ko_KO-kss_low` 는 KSS 데이터셋 기반(CC BY-NC-SA — 비상업). MIT 인 MeloTTS-Korean 은 lexicon 변환이 필요하며 후속 과제로 추적한다.

## 빌드

```sh
cargo build --release   # dylib 는 바이너리 옆에, rpath 는 @loader_path
```
