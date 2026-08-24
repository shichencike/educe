#!/usr/bin/env bash
# 交叉编译 Linux musl 静态二进制（x86_64 / aarch64），单文件、零外部依赖。
# aarch64 产物同样适用于 Termux（Android）。
#
# 依赖：cargo-zigbuild + zig
#   cargo install cargo-zigbuild --locked
#   zig:  https://ziglang.org/download/  （或 pip install ziglang）
#
# 用法: bash scripts/build-musl.sh [target...]   # 默认编译两个架构
set -euo pipefail
cd "$(dirname "$0")/.."

TARGETS=("${@:-x86_64-unknown-linux-musl aarch64-unknown-linux-musl}")

if ! command -v cargo-zigbuild >/dev/null 2>&1; then
  echo "错误: 未找到 cargo-zigbuild。安装: cargo install cargo-zigbuild --locked" >&2
  exit 1
fi
if ! command -v zig >/dev/null 2>&1; then
  echo "错误: 未找到 zig。安装: https://ziglang.org/download/ 或 pip install ziglang" >&2
  exit 1
fi

rustup target add "${TARGETS[@]}" >/dev/null 2>&1 || true
for t in "${TARGETS[@]}"; do
  echo "==> cargo zigbuild --release --target $t"
  cargo zigbuild --release --target "$t"
done

echo ""
echo "==> 产物:"
ls -lh target/*/release/educe 2>/dev/null || true
