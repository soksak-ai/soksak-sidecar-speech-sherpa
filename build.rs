// 프리빌드 sherpa-onnx dylib 를 바이너리 옆에서 찾는다(@loader_path rpath).
// 배포 tar.gz 는 바이너리+dylib 동봉 — browser-chromium 사이드카와 동일 모델.
fn main() {
    #[cfg(target_os = "macos")]
    println!("cargo:rustc-link-arg=-Wl,-rpath,@loader_path");
    #[cfg(target_os = "linux")]
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
}
