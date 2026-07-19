// soksak-sidecar-speech-sherpa — sherpa-onnx 음성 service 사이드카(stdio JSON-lines).
// 계약 = soksak-spec-sidecar-speech (버전 없는 contract-id — 런타임 식별은 무버전 문자열):
//   요청 {id,op,...} 1줄 → 응답 1줄. 단 op:tts + stream:true 는 합성 진행에 따라
//   청크 줄 {id,ok,chunk,sampleRate,pcmBase64(s16le)} N개 → 종결 줄 {id,ok,done:true}.
// 모델은 동봉하지 않는다 — --model-dir 로 사용자가 지정(라이선스는 모델 문서 참조).
// M2a 범위 = op:info/tts. ASR/VAD 는 M2b 에서 같은 계약에 op 를 더한다.
mod backend;
mod engine;
mod supertonic;

use std::io::{BufRead, Write};
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use backend::Backend;
use base64::Engine as _;
use serde::Deserialize;
use serde_json::json;

const SPEC: &str = "soksak-spec-sidecar-speech";

struct Args {
    engine: String, // vits | kokoro | supertonic
    model_dir: PathBuf,
}

fn parse_args() -> Result<Args> {
    let mut engine = "vits".to_string();
    let mut model_dir: Option<PathBuf> = None;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--engine" => engine = it.next().context("--engine value missing")?,
            "--model-dir" => {
                model_dir = Some(PathBuf::from(it.next().context("--model-dir value missing")?))
            }
            "--help" | "-h" => {
                eprintln!("usage: soksak-sidecar-speech-sherpa --model-dir <dir> [--engine vits|kokoro|supertonic]");
                std::process::exit(0);
            }
            other => return Err(anyhow!("unknown argument: {other}")),
        }
    }
    Ok(Args {
        engine,
        model_dir: model_dir.context("--model-dir required")?,
    })
}

// f32 [-1,1] → s16le 바이트.
fn pcm_s16le(samples: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 2);
    for s in samples {
        out.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    out
}

// f32 → 16-bit PCM WAV(모노) — 통짜 응답용. 의존성 없이 44바이트 헤더 수제.
fn wav_encode(samples: &[f32], sample_rate: u32) -> Vec<u8> {
    let data = pcm_s16le(samples);
    let data_len = data.len() as u32;
    let mut out = Vec::with_capacity(44 + data.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(&data);
    out
}

#[derive(Deserialize)]
struct Req {
    id: u64,
    op: String,
    text: Option<String>,
    lang: Option<String>, // supertonic 계열용(2글자) — sherpa 모델은 무시
    sid: Option<i32>,
    speed: Option<f32>,
    stream: Option<bool>,
}

fn write_line(out: &mut impl Write, v: &serde_json::Value) -> Result<()> {
    serde_json::to_writer(&mut *out, v)?;
    out.write_all(b"\n")?;
    out.flush()?;
    Ok(())
}

// ort 전용 onnxruntime 파일명(플랫폼별) — sherpa 자체 onnxruntime(libonnxruntime.1.17.1.*)과 뚜렷이
// 구분된다. release.yml 이 이 이름으로 Microsoft onnxruntime 1.22 를 바이너리 형제에 동봉한다.
const ORT_ONNXRUNTIME_FILE: &str = if cfg!(windows) {
    "onnxruntime-ort.dll"
} else if cfg!(target_os = "macos") {
    "libonnxruntime-ort.dylib"
} else {
    "libonnxruntime-ort.so"
};

// load-dynamic: ort 는 onnxruntime 를 런타임에 dlopen 한다(빌드타임 프리빌드 불요 → Intel macOS 포함 전
// 타깃 빌드). 바이너리 형제로 동봉된 ort 전용 onnxruntime(api-22=1.22, sherpa 의 1.17.1 과 별개 인스턴스)을
// ORT_DYLIB_PATH 로 가리킨다. 두 인스턴스는 dyld 이단계 네임스페이스로 격리된다.
fn point_ort_at_bundled_onnxruntime() {
    if std::env::var_os("ORT_DYLIB_PATH").is_some() {
        return; // 명시 지정 우선(개발/시스템 onnxruntime 오버라이드).
    }
    let Ok(exe) = std::env::current_exe() else { return };
    let Some(dir) = exe.parent() else { return };
    let candidate = dir.join(ORT_ONNXRUNTIME_FILE);
    if candidate.is_file() {
        // SAFETY: 단일 스레드 시작 시점(ort Session 생성·스레드 스폰 전)에만 호출한다.
        unsafe { std::env::set_var("ORT_DYLIB_PATH", &candidate) };
    }
}

fn main() -> Result<()> {
    point_ort_at_bundled_onnxruntime();
    let args = parse_args()?;
    let (mut tts, model) = Backend::open(&args.engine, &args.model_dir)?;
    eprintln!(
        "[soksak-sidecar-speech-sherpa] ready engine={} model={} sr={} speakers={}",
        args.engine,
        model,
        tts.sample_rate(),
        tts.num_speakers()
    );

    let b64 = base64::engine::general_purpose::STANDARD;
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let mut out = stdout.lock();
        let req: Req = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                write_line(&mut out, &json!({"id":0,"ok":false,"message":format!("bad request: {e}")}))?;
                continue;
            }
        };
        match req.op.as_str() {
            "info" => write_line(
                &mut out,
                &json!({
                    "id": req.id, "ok": true, "spec": SPEC,
                    "engine": args.engine, "model": model,
                    "sampleRate": tts.sample_rate(), "numSpeakers": tts.num_speakers(),
                    "styles": tts.style_names(),
                    "ops": ["info", "tts"], "stream": true,
                }),
            )?,
            "tts" => {
                let text = req.text.unwrap_or_default();
                if text.trim().is_empty() {
                    write_line(&mut out, &json!({"id":req.id,"ok":false,"message":"text required"}))?;
                    continue;
                }
                let lang = req.lang.unwrap_or_else(|| "en".to_string());
                let sid = req.sid.unwrap_or(0);
                let speed = req.speed.unwrap_or(1.0);
                let streaming = req.stream.unwrap_or(false);
                let sr = tts.sample_rate();
                // 스트리밍: 합성되는 대로 s16le PCM 청크. 비스트리밍: 청크를 모아 통짜 WAV.
                let mut k = 0u32;
                let mut whole: Vec<f32> = Vec::new();
                let mut chunk_err: Option<anyhow::Error> = None;
                let r = {
                    let mut on_chunk = |samples: &[f32]| -> bool {
                        if !streaming {
                            whole.extend_from_slice(samples);
                            return true;
                        }
                        k += 1;
                        let v = json!({
                            "id": req.id, "ok": true, "chunk": k,
                            "sampleRate": sr, "pcmBase64": b64.encode(pcm_s16le(samples)),
                        });
                        match write_line(&mut out, &v) {
                            Ok(()) => true,
                            Err(e) => {
                                chunk_err = Some(e);
                                false // 파이프 사망 — 합성 중단
                            }
                        }
                    };
                    tts.generate_streamed(&text, &lang, sid, speed, &mut on_chunk)
                };
                if let Some(e) = chunk_err {
                    return Err(e); // stdout 소실 = 부모 사망 — 종료
                }
                match (streaming, r) {
                    (true, Ok(total)) => write_line(
                        &mut out,
                        &json!({"id":req.id,"ok":true,"done":true,"chunks":k,"numSamples":total,"sampleRate":sr}),
                    )?,
                    (false, Ok(_)) => {
                        let wav = wav_encode(&whole, sr);
                        write_line(
                            &mut out,
                            &json!({
                                "id": req.id, "ok": true,
                                "sampleRate": sr,
                                "numSamples": whole.len(),
                                "wavBase64": b64.encode(&wav),
                            }),
                        )?;
                    }
                    (_, Err(e)) => write_line(
                        &mut out,
                        &json!({"id":req.id,"ok":false,"done":true,"message":format!("tts failed: {e}")}),
                    )?,
                }
            }
            other => write_line(
                &mut out,
                &json!({"id":req.id,"ok":false,"message":format!("unknown op: {other}")}),
            )?,
        }
    }
    Ok(())
}
