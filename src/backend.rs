// 합성 백엔드 디스패치 — sherpa(vits/kokoro) 와 supertonic(ort 직결)을 한 인터페이스로.
// supertonic 은 flow-matching 통짜 합성이라 내부 청크 콜백이 없다 — 상위가 문장 단위로
// 요청하는 구조(플러그인 파이프라인)와 만나면 체감 지연은 문장 길이에 비례하는 짧은 값.
use std::path::Path;

use anyhow::{anyhow, Context, Result};

use crate::engine::TtsEngine;
use crate::supertonic::{self, Style, TextToSpeech};

pub enum Backend {
    Sherpa(TtsEngine),
    Supertonic(SupertonicBackend),
}

pub struct SupertonicBackend {
    tts: TextToSpeech,
    styles: Vec<(String, Style)>, // (이름, 스타일) — sid 로 인덱스(파일명 정렬)
    pub sample_rate: u32,
}

impl Backend {
    pub fn open(engine: &str, model_dir: &Path) -> Result<(Self, String)> {
        match engine {
            "supertonic" => {
                let onnx_dir = model_dir.join("onnx");
                if !onnx_dir.is_dir() {
                    return Err(anyhow!("supertonic model dir needs onnx/ (got {})", model_dir.display()));
                }
                let tts = supertonic::load_text_to_speech(
                    onnx_dir.to_str().context("model dir not utf-8")?,
                    false,
                )
                .map_err(|e| anyhow!("supertonic load failed: {e}"))?;
                let mut styles = Vec::new();
                let styles_dir = model_dir.join("voice_styles");
                let mut names: Vec<_> = std::fs::read_dir(&styles_dir)
                    .with_context(|| format!("voice_styles unreadable: {}", styles_dir.display()))?
                    .filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.extension().is_some_and(|x| x == "json"))
                    .collect();
                names.sort();
                for p in names {
                    let name = p.file_stem().and_then(|s| s.to_str()).unwrap_or("style").to_string();
                    let style = supertonic::load_voice_style(&[p.display().to_string()], false)
                        .map_err(|e| anyhow!("voice style {name} load failed: {e}"))?;
                    styles.push((name, style));
                }
                if styles.is_empty() {
                    return Err(anyhow!("no voice styles in {}", styles_dir.display()));
                }
                let sample_rate = tts.sample_rate as u32;
                let model = onnx_dir.display().to_string();
                Ok((
                    Backend::Supertonic(SupertonicBackend { tts, styles, sample_rate }),
                    model,
                ))
            }
            _ => {
                let (e, model) = TtsEngine::open(engine, model_dir)?;
                Ok((Backend::Sherpa(e), model))
            }
        }
    }

    pub fn sample_rate(&self) -> u32 {
        match self {
            Backend::Sherpa(e) => e.sample_rate,
            Backend::Supertonic(b) => b.sample_rate,
        }
    }

    pub fn num_speakers(&self) -> i32 {
        match self {
            Backend::Sherpa(e) => e.num_speakers.max(1),
            Backend::Supertonic(b) => b.styles.len() as i32,
        }
    }

    pub fn style_names(&self) -> Vec<String> {
        match self {
            Backend::Sherpa(_) => Vec::new(),
            Backend::Supertonic(b) => b.styles.iter().map(|(n, _)| n.clone()).collect(),
        }
    }

    /// 스트리밍 합성 — 청크 콜백. supertonic 은 통짜 1청크(빠른 합성으로 상쇄).
    pub fn generate_streamed(
        &mut self,
        text: &str,
        lang: &str,
        sid: i32,
        speed: f32,
        on_chunk: &mut dyn FnMut(&[f32]) -> bool,
    ) -> Result<usize> {
        match self {
            Backend::Sherpa(e) => {
                let mut sink = crate::engine::SynthChunkSink { on_chunk };
                e.generate_streamed(text, sid, speed, &mut sink)
            }
            Backend::Supertonic(b) => {
                let lang2 = &lang.to_lowercase()[..lang.len().min(2)];
                let lang_ok = if supertonic::is_valid_lang(lang2) { lang2 } else { "en" };
                let idx = (sid.max(0) as usize).min(b.styles.len() - 1);
                let style = &b.styles[idx].1;
                // total_step 8 = 레퍼런스 기본(품질/속도 균형), 문장 사이 무음 0.15s
                let (wav, _dur) = b
                    .tts
                    .call(text, lang_ok, style, 8, speed, 0.15)
                    .map_err(|e| anyhow!("supertonic synth failed: {e}"))?;
                on_chunk(&wav);
                Ok(wav.len())
            }
        }
    }
}
