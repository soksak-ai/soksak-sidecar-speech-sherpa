// sherpa-onnx OfflineTts 직결(sys FFI) — 안전 래퍼(sherpa-rs) 대신 sys 를 쓰는 이유는
// GenerateWithCallbackWithArg(합성 중 청크 스트리밍)가 래퍼에 노출되지 않아서다.
// 청크 콜백은 generate 호출 스레드에서 인라인 실행된다(별도 스레드 없음).
use std::ffi::{c_void, CString};
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use sherpa_rs_sys as sys;

pub struct SynthChunkSink<'a> {
    /// 청크 수신 — false 반환 시 합성 중단.
    pub on_chunk: &'a mut dyn FnMut(&[f32]) -> bool,
}

pub struct TtsEngine {
    ptr: *const sys::SherpaOnnxOfflineTts,
    pub sample_rate: u32,
    pub num_speakers: i32,
    // 구성 문자열 수명 유지(FFI 가 참조하는 CString 들)
    _held: Vec<CString>,
}

unsafe impl Send for TtsEngine {}

fn cstr(s: &str) -> CString {
    CString::new(s).unwrap_or_default()
}

fn opt_path(dir: &Path, name: &str) -> String {
    let p = dir.join(name);
    if p.exists() {
        p.display().to_string()
    } else {
        String::new()
    }
}

fn find_onnx(dir: &Path) -> Result<String> {
    let mut cands: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("model dir unreadable: {}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "onnx"))
        .collect();
    cands.sort();
    cands
        .first()
        .map(|p| p.display().to_string())
        .context("no .onnx model in model dir")
}

impl TtsEngine {
    /// engine = "vits" | "kokoro", 모델 디렉토리에서 sherpa 관례 파일 자동 탐지.
    pub fn open(engine: &str, model_dir: &Path) -> Result<(Self, String)> {
        let model = find_onnx(model_dir)?;
        let tokens = opt_path(model_dir, "tokens.txt");
        let lexicon = opt_path(model_dir, "lexicon.txt");
        let data_dir = opt_path(model_dir, "espeak-ng-data");
        let dict_dir = opt_path(model_dir, "dict");
        let voices = opt_path(model_dir, "voices.bin");

        let held: Vec<CString> = vec![
            cstr(&model),    // 0
            cstr(&tokens),   // 1
            cstr(&lexicon),  // 2
            cstr(&data_dir), // 3
            cstr(&dict_dir), // 4
            cstr(&voices),   // 5
            cstr("cpu"),     // 6 provider
            cstr(""),        // 7 rule_fsts/fars/lang 공용 빈 문자열
        ];

        let empty = held[7].as_ptr();
        let mut vits: sys::SherpaOnnxOfflineTtsVitsModelConfig = unsafe { std::mem::zeroed() };
        let mut kokoro: sys::SherpaOnnxOfflineTtsKokoroModelConfig = unsafe { std::mem::zeroed() };
        match engine {
            "vits" => {
                vits = sys::SherpaOnnxOfflineTtsVitsModelConfig {
                    model: held[0].as_ptr(),
                    lexicon: held[2].as_ptr(),
                    tokens: held[1].as_ptr(),
                    data_dir: held[3].as_ptr(),
                    noise_scale: 0.667,
                    noise_scale_w: 0.8,
                    length_scale: 1.0,
                    dict_dir: held[4].as_ptr(),
                };
            }
            "kokoro" => {
                kokoro = sys::SherpaOnnxOfflineTtsKokoroModelConfig {
                    model: held[0].as_ptr(),
                    voices: held[5].as_ptr(),
                    tokens: held[1].as_ptr(),
                    data_dir: held[3].as_ptr(),
                    length_scale: 1.0,
                    dict_dir: held[4].as_ptr(),
                    lexicon: held[2].as_ptr(),
                    lang: empty,
                };
            }
            other => return Err(anyhow!("unknown engine: {other} (vits|kokoro)")),
        }

        let config = sys::SherpaOnnxOfflineTtsConfig {
            model: sys::SherpaOnnxOfflineTtsModelConfig {
                vits,
                num_threads: 2,
                debug: 0,
                provider: held[6].as_ptr(),
                matcha: unsafe { std::mem::zeroed() },
                kokoro,
                kitten: unsafe { std::mem::zeroed() },
            },
            rule_fsts: empty,
            // 1 = 내부 문장 단위마다 콜백 → 스트리밍 최소 지연
            max_num_sentences: 1,
            rule_fars: empty,
            silence_scale: 0.2,
        };

        let ptr = unsafe { sys::SherpaOnnxCreateOfflineTts(&config) };
        if ptr.is_null() {
            return Err(anyhow!("SherpaOnnxCreateOfflineTts failed (model files?)"));
        }
        let sample_rate = unsafe { sys::SherpaOnnxOfflineTtsSampleRate(ptr) } as u32;
        let num_speakers = unsafe { sys::SherpaOnnxOfflineTtsNumSpeakers(ptr) };
        Ok((
            Self {
                ptr,
                sample_rate,
                num_speakers,
                _held: held,
            },
            model,
        ))
    }

    /// 통짜 합성 — 전체 샘플 반환.
    pub fn generate(&self, text: &str, sid: i32, speed: f32) -> Result<Vec<f32>> {
        let c_text = cstr(text);
        let audio = unsafe { sys::SherpaOnnxOfflineTtsGenerate(self.ptr, c_text.as_ptr(), sid, speed) };
        if audio.is_null() {
            return Err(anyhow!("generate returned null"));
        }
        let out = unsafe {
            let a = &*audio;
            let s = std::slice::from_raw_parts(a.samples, a.n.max(0) as usize).to_vec();
            sys::SherpaOnnxDestroyOfflineTtsGeneratedAudio(audio);
            s
        };
        Ok(out)
    }

    /// 스트리밍 합성 — 내부 문장 단위 청크마다 sink 호출(합성과 동시에 소비).
    pub fn generate_streamed(&self, text: &str, sid: i32, speed: f32, sink: &mut SynthChunkSink) -> Result<usize> {
        unsafe extern "C" fn trampoline(samples: *const f32, n: i32, arg: *mut c_void) -> i32 {
            let sink = unsafe { &mut *(arg as *mut SynthChunkSink) };
            let chunk = unsafe { std::slice::from_raw_parts(samples, n.max(0) as usize) };
            if (sink.on_chunk)(chunk) {
                1
            } else {
                0
            }
        }
        let c_text = cstr(text);
        let audio = unsafe {
            sys::SherpaOnnxOfflineTtsGenerateWithCallbackWithArg(
                self.ptr,
                c_text.as_ptr(),
                sid,
                speed,
                Some(trampoline),
                sink as *mut SynthChunkSink as *mut c_void,
            )
        };
        if audio.is_null() {
            return Err(anyhow!("generate(streamed) returned null"));
        }
        let total = unsafe {
            let n = (*audio).n.max(0) as usize;
            sys::SherpaOnnxDestroyOfflineTtsGeneratedAudio(audio);
            n
        };
        Ok(total)
    }
}

impl Drop for TtsEngine {
    fn drop(&mut self) {
        unsafe { sys::SherpaOnnxDestroyOfflineTts(self.ptr) };
    }
}
