#!/bin/sh
# bin/ 스테이징 — 원자 교체 강제. in-place cp 는 macOS 코드서명 캐시 불일치로
# 기동 즉시 SIGKILL(exit 137)을 만든다(잠복: 상주 프로세스는 옛 바이너리로 계속 돎).
# 교체 후 신선-기동 스모크까지 한 번에 — "상주가 살아있어서 통과" 착시를 차단한다.
set -eu
cd "$(dirname "$0")"
BIN=soksak-sidecar-speech-sherpa
[ -f "target/release/$BIN" ] || { echo "build first: cargo build --release" >&2; exit 1; }
mkdir -p bin
cp "target/release/$BIN" "bin/$BIN.tmp"
for d in target/release/*.dylib; do cp "$d" bin/; done
mv -f "bin/$BIN.tmp" "bin/$BIN"   # 원자 교체(rename) — 새 inode
# 신선-기동 스모크: 모델 없이도 argv 파싱 단계 실행됨(SIGKILL 이면 여기서 즉사)
if out=$(echo "" | "bin/$BIN" --help 2>&1); then :; else
  code=$?
  [ $code -eq 137 ] && { echo "SMOKE FAIL: SIGKILL(서명) — 스테이징 실패" >&2; exit 137; }
fi
echo "staged: bin/$BIN (atomic, smoke ok)"
