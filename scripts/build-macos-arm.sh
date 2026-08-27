#!/usr/bin/env bash
set -euo pipefail

# 在 Apple Silicon macOS 上构建 aarch64 DMG；副作用是生成 src-tauri/target 下的发布产物。
if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "此脚本只能在 macOS 上运行。" >&2
  exit 1
fi

if [[ "$(uname -m)" != "arm64" ]]; then
  echo "此脚本只能在 Apple Silicon（arm64）macOS 上运行。" >&2
  exit 1
fi

if ! command -v pnpm >/dev/null 2>&1; then
  echo "未找到 pnpm，请先安装 pnpm。" >&2
  exit 1
fi

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"
cd "$PROJECT_ROOT"

pnpm tauri build --target aarch64-apple-darwin --bundles dmg

echo "DMG 已生成：$PROJECT_ROOT/src-tauri/target/aarch64-apple-darwin/release/bundle/dmg/"
