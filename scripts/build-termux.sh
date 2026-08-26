#!/usr/bin/env bash
# 在 Termux（Android）内本机构建，无需交叉编译。
# 前置：pkg install rust binutils clang
#   - clang 用于编译 ring 的 C 部分（tls-rustls 后端必需）
#   - 使用轻量 release-termux profile（关 LTO、提高并行编译单元），
#     并按内存自动限制并行度，避免低内存手机 OOM / 构建过慢。
set -euo pipefail
cd "$(dirname "$0")/.."

# 全新 Termux 本地没有软件源列表，pkg install 会直接失败，先同步一次
if ! pkg show rust >/dev/null 2>&1; then
  echo "==> 首次使用，先 pkg update 同步软件源..."
  pkg update
fi
pkg install -y rust binutils clang

# 按 /proc/meminfo 自动限制并行编译，避免 OOM
MEM_KB=$(grep '^MemTotal:' /proc/meminfo 2>/dev/null | tr -dc '0-9')
MEM_KB=${MEM_KB:-0}
JOBS=""
if [ "$MEM_KB" -gt 0 ]; then
  if   [ "$MEM_KB" -lt 4194304 ]; then JOBS=2   # < 4GB
  elif [ "$MEM_KB" -lt 8388608 ]; then JOBS=4   # 4~8GB
  else JOBS=8                                    # >= 8GB
  fi
fi

echo "==> 开始构建（profile=release-termux，内存 $((MEM_KB / 1024))MB，jobs=${JOBS:-默认}）..."
if [ -n "$JOBS" ]; then
  cargo build --profile release-termux --jobs "$JOBS"
else
  cargo build --profile release-termux
fi

echo ""
echo "==> 产物: $(pwd)/target/release-termux/educe"
echo "    运行: ./target/release-termux/educe serve"
