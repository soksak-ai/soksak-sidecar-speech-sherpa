#!/bin/bash
# dist 스테이징의 단일 진실 — dev 와 CI(release.yml)가 같은 스크립트를 쓴다.
# 사용: stage.sh <dist-dir>   (cargo build --release 선행 전제; 이 크레이트 디렉토리에서 실행)
# 배포 아카이브 = 바이너리 + 프리빌드 sherpa-onnx/onnxruntime 공유 라이브러리(바이너리 형제, rpath
# @loader_path/$ORIGIN 로 로드). browser-chromium 사이드카와 동일 모델(자족 tar.gz).
set -euo pipefail
dist="${1:?사용: stage.sh <dist-dir>}"
cd "$(dirname "$0")"
# windows CI 는 MAX_PATH 회피로 CARGO_TARGET_DIR 을 짧은 루트로 옮길 수 있다 — 산출 위치를 맞춘다.
src="${CARGO_TARGET_DIR:-target}/release"
bin="soksak-sidecar-speech-sherpa"
mkdir -p "$dist"

case "$(uname -s)" in
  MINGW* | MSYS* | CYGWIN*) os="windows"; ext="dll"; exe=".exe" ;;
  Linux)                    os="linux";   ext="so";  exe="" ;;
  Darwin)                   os="macos";   ext="dylib"; exe="" ;;
  *) echo "unsupported OS: $(uname -s)" >&2; exit 1 ;;
esac

[ -f "$src/$bin$exe" ] || { echo "build first: cargo build --release ($src/$bin$exe 없음)" >&2; exit 1; }

# 프리빌드 sherpa-onnx / onnxruntime 공유 라이브러리 — 이게 없으면 dlopen 실패로 기동 불가.
libs=$(find "$src" -maxdepth 1 -name "*.$ext" -type f 2>/dev/null || true)
[ -n "$libs" ] || { echo "no *.$ext runtime libraries staged from $src — sherpa-rs-sys download-binaries 확인" >&2; exit 1; }
for l in $libs; do
  base="$(basename "$l")"
  tmp="$dist/.$base.tmp.$$"
  cp "$l" "$tmp"
  mv -f "$tmp" "$dist/$base"   # 원자 교체(rename) — in-place cp 는 macOS 서명 페이지 캐시 불일치로 dlopen SIGKILL
done

# 바이너리 — 원자 교체(macOS 코드서명 캐시 불일치 SIGKILL 회피, in-place cp 금지).
btmp="$dist/.$bin$exe.tmp.$$"
cp "$src/$bin$exe" "$btmp"
mv -f "$btmp" "$dist/$bin$exe"

# 신선-기동 스모크(macOS 만) — 상주 프로세스 착시 없이 서명 SIGKILL(exit 137)을 즉시 잡는다.
if [ "$os" = "macos" ]; then
  if "$dist/$bin$exe" --help >/dev/null 2>&1; then :; else
    code=$?
    [ "$code" -eq 137 ] && { echo "SMOKE FAIL: SIGKILL(서명) — 스테이징 실패" >&2; exit 137; }
  fi
fi

echo "스테이지 완료($os): $dist"
ls -la "$dist"
