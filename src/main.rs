// soksak-sidecar-speech-sherpa — sherpa-onnx 음성 service 사이드카(stdio JSON-lines).
// 계약 = soksak-sidecar-speech-spec@1:
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

const SPEC: &str = "soksak-sidecar-speech-spec@1";

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

fn main() -> Result<()> {
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
