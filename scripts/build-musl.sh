#!/usr/bin/env bash
# 交叉编译 Linux musl 静态二进制（x86_64 / aarch64），单文件、零外部依赖。
# aarch64 产物同样适用于 Termux（Android）。
#
# 依赖（二选一）：
#   1) cargo-zigbuild + zig:  cargo install cargo-zigbuild && (装 zig)
#   2) docker
#
# 用法: bash scripts/build-musl.sh [target...]   # 默认编译两个架构
set -euo pipefail
cd "$(dirname "$0")/.."

TARGETS=("${@:-x86_64-unknown-linux-musl aarch64-unknown-linux-musl}")

if command -v cargo-zigbuild >/dev/null 2>&1; then
  rustup target add "${TARGETS[@]}" >/dev/null 2>&1 || true
  for t in "${TARGETS[@]}"; do
    echo "==> cargo zigbuild --release --target $t"
    cargo zigbuild --release --target "$t"
  done
elif command -v docker >/dev/null 2>&1; then
  for t in "${TARGETS[@]}"; do
    echo "==> docker 构建 $t"
    docker run --rm -v "$PWD:/src" -w /src rust:1 bash -c "
      apt-get update -qq && \
      if [ \"$t\" = \"aarch64-unknown-linux-musl\" ]; then \
        curl -fsSL https://musl.cc/aarch64-linux-musl-cross.tgz | tar xz -C /opt && \
        export PATH=\"/opt/aarch64-linux-musl-cross/bin:\$PATH\"; \
      else \
        apt-get install -y -qq musl-tools >/dev/null 2>&1; \
      fi && \
      rustup target add $t >/dev/null 2>&1 && cargo build --release --target $t"
  done
else
  echo "错误: 需要 cargo-zigbuild（含 zig）或 docker 才能交叉编译。" >&2
  exit 1
fi

echo ""
echo "==> 产物:"
ls -lh target/*/release/educe 2>/dev/null || true
