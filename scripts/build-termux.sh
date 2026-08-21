#!/usr/bin/env bash
# 在 Termux（Android）内本机构建，无需交叉编译。
# 前置：pkg install rust （Termux 的 rust 包自带 Android 目标工具链）
# 可选：pkg install binutils
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build --release

echo ""
echo "==> 产物: $(pwd)/target/release/educe"
echo "    运行: ./target/release/educe serve"
