#!/usr/bin/env bash
# 自检脚本（Linux/macOS）。任何一步失败就停下并返回非零。
set -euo pipefail
cd "$(dirname "$0")/.."

echo '==> cargo test --release --lib'
cargo test --release --lib

echo '==> cargo test --release --test golden'
cargo test --release --test golden

echo '==> cargo check --features win32-check --bins'
cargo check --features win32-check --bins

py=''
for c in python3 python; do
    if command -v "$c" >/dev/null 2>&1; then py="$c"; break; fi
done
if [ -n "$py" ] && "$py" -c 'import cv2, numpy' >/dev/null 2>&1; then
    keyfile="$(mktemp)"
    trap 'rm -f "$keyfile"' EXIT
    printf '%s' "$(printf '%02x' {0..31})" > "$keyfile"
    out="$("$py" python/blindmark.py detect tests/golden/marked.png --key "$keyfile")"
    if ! printf '%s' "$out" | grep -q '"id_hex": "0x0badf00d"'; then
        echo 'python reference mismatch' >&2
        exit 1
    fi
    echo 'python interop: ok'
else
    echo '跳过 Python 互通测试'
fi

echo 'ALL CHECKS PASSED'
