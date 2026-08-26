#!/usr/bin/env bash
# ============================================================
# Educe · Termux（Android）一键安装 + 启动脚本
#
# 特性：
#   - 自动安装依赖（rust / binutils / clang），无需交叉编译，bionic 本机构建
#   - 首次使用自动 pkg update 同步软件源（全新 Termux 也能直接装包）
#   - 用轻量 release-termux profile 构建，并按内存自动限制并行度，避免 OOM
#   - 自动生成 config.toml：host=0.0.0.0，手机与电脑同一 Wi-Fi 即可互访
#   - 自动安装 termux-api 并启用 termux-wake-lock 防止后台休眠断网
#   - 可选注册 termux-services 自启动（--install-service）
#   - 打印本机 / 局域网访问地址
#
# 用法：
#   bash scripts/termux-setup.sh                 # 安装依赖 + 构建 + 启动
#   bash scripts/termux-setup.sh --no-build      # 仅启动（已构建过）
#   bash scripts/termux-setup.sh --install-service   # 额外注册开机自启
#   bash scripts/termux-setup.sh --port 9000     # 指定端口
# ============================================================
set -euo pipefail

# ---- 参数解析 ----
DO_BUILD=1
INSTALL_SERVICE=0
PORT=8080
while [ $# -gt 0 ]; do
  case "$1" in
    --no-build) DO_BUILD=0 ;;
    --install-service) INSTALL_SERVICE=1 ;;
    --port) PORT="${2:?--port 需要端口号}"; shift ;;
    *) echo "未知参数: $1" >&2; exit 1 ;;
  esac
  shift
done

cd "$(dirname "$0")/.."

# ---- 环境检查：必须是 Termux ----
if [ -z "${PREFIX:-}" ] || [ ! -d "${PREFIX:-/nonexistent}" ]; then
  echo "错误: 未检测到 Termux 环境（\$PREFIX 为空）。" >&2
  echo "请在 Termux 内运行: pkg install bash && bash scripts/termux-setup.sh" >&2
  exit 1
fi

echo "==> 检测到 Termux（\$PREFIX=$PREFIX）"
echo "    设备: $(getprop ro.product.model 2>/dev/null || echo 未知)"
echo "    安卓: $(getprop ro.build.version.release 2>/dev/null || echo 未知)"

# ---- 安装依赖 ----
echo ""
echo "==> 检查依赖（rust / binutils / clang）..."
# 全新 Termux 本地没有软件源列表，pkg install 会直接失败，先同步一次
if ! pkg show rust >/dev/null 2>&1; then
  echo "    首次使用，先 pkg update 同步软件源..."
  pkg update
fi
# clang 用于编译 ring 的 C 部分（tls-rustls 后端必需）
pkg install -y rust binutils clang

# ---- 构建（bionic 本机）----
if [ "$DO_BUILD" -eq 1 ]; then
  echo ""
  echo "==> 开始本机构建（bionic，首次约 3~10 分钟，后续增量很快）..."
  # 手机内存有限：按 /proc/meminfo 自动限制并行编译，避免 OOM
  MEM_KB=$(grep '^MemTotal:' /proc/meminfo 2>/dev/null | tr -dc '0-9')
  MEM_KB=${MEM_KB:-0}
  JOBS=""
  if [ "$MEM_KB" -gt 0 ]; then
    if   [ "$MEM_KB" -lt 4194304 ]; then JOBS=2   # < 4GB
    elif [ "$MEM_KB" -lt 8388608 ]; then JOBS=4   # 4~8GB
    else JOBS=8                                    # >= 8GB
    fi
  fi
  if [ -n "$JOBS" ]; then
    echo "    检测到内存 $((MEM_KB / 1024))MB，限制并行编译为 $JOBS 任务（防 OOM）"
    cargo build --profile release-termux --jobs "$JOBS"
  else
    cargo build --profile release-termux
  fi
  echo "    构建完成: $(pwd)/target/release-termux/educe"
fi

BIN="$(pwd)/target/release-termux/educe"
if [ ! -x "$BIN" ]; then
  echo "错误: 未找到可执行文件 $BIN，请先去掉 --no-build 完整构建一次。" >&2
  exit 1
fi

# ---- 生成配置（不存在时）----
if [ ! -f config.toml ]; then
  echo ""
  echo "==> 未找到 config.toml，自动生成（host=0.0.0.0 以支持局域网访问）..."
  "$BIN" gen-config > config.toml
  # 把监听地址改为 0.0.0.0，手机与电脑同一 Wi-Fi 即可访问
  sed -i 's/^host = "127.0.0.1"/host = "0.0.0.0"/' config.toml
  echo "    已生成 config.toml"
else
  echo ""
  echo "==> 已存在 config.toml，跳过生成（如需局域网访问请确认 host = \"0.0.0.0\"）"
fi

# ---- 端口写入配置（若用户指定）----
if [ "$PORT" != "8080" ]; then
  sed -i "s/^port = [0-9]*/port = $PORT/" config.toml
  echo "    端口已设为 $PORT"
fi

# ---- 可选：注册 termux-services 自启动 ----
if [ "$INSTALL_SERVICE" -eq 1 ]; then
  echo ""
  echo "==> 注册 termux-services 自启动（educe 服务）..."
  pkg install -y termux-services 2>/dev/null || true
  mkdir -p "$PREFIX/var/service/educe"
  # 记录项目根目录的绝对路径（服务启动时工作目录不固定，不能依赖相对路径）
  PROJ_DIR="$(pwd)"
  cat > "$PREFIX/var/service/educe/run" <<EOF
#!/data/data/com.termux/files/usr/bin/bash
exec 2>&1
cd "$PROJ_DIR"
exec ./target/release-termux/educe serve --config config.toml
EOF
  chmod +x "$PREFIX/var/service/educe/run"
  echo "    已注册。启用: sv up educe；禁用: sv down educe；状态: sv status educe"
  echo "    注意: 自启服务默认绑定 127.0.0.1（config.toml 的 host），"
  echo "          如需开机后局域网访问，请把 config.toml 的 host 改为 0.0.0.0。"
fi

# ---- 获取局域网 IP ----
get_lan_ip() {
  local ip=""
  ip=$(ip -4 addr show scope global 2>/dev/null | awk '/inet /{print $2}' | cut -d/ -f1 | head -1) || true
  if [ -z "$ip" ]; then
    ip=$(hostname -I 2>/dev/null | awk '{print $1}') || true
  fi
  [ -n "$ip" ] && echo "$ip" || true
}
LAN_IP=$(get_lan_ip || true)

# ---- 唤醒锁（防止后台休眠断网）----
echo ""
if command -v termux-wake-lock >/dev/null 2>&1; then
  termux-wake-lock
  echo "==> 已启用 termux-wake-lock（后台不休眠）"
else
  echo "==> 未检测到 termux-wake-lock，尝试安装 termux-api..."
  pkg install -y termux-api 2>/dev/null || true
  if command -v termux-wake-lock >/dev/null 2>&1; then
    termux-wake-lock
    echo "==> 已安装 termux-api 并启用 termux-wake-lock（后台不休眠）"
  else
    echo "==> 提示: 还需在系统安装 Termux:API 应用（Google Play / F-Droid 搜索 Termux:API），"
    echo "    安装后重跑本脚本即可启用 wake-lock 防止后台休眠。"
  fi
fi

# ---- 启动 ----
echo ""
echo "==> 启动 Educe 服务..."
echo "    本机访问:  http://127.0.0.1:$PORT"
if [ -n "$LAN_IP" ]; then
  echo "    局域网访问: http://$LAN_IP:$PORT   （手机/电脑需在同一 Wi-Fi）"
else
  echo "    局域网访问: 请查看路由器分配给本机的 IP，形如 http://192.168.x.x:$PORT"
fi
echo "    停止: Ctrl+C（退出 Termux 后服务随之结束）"
echo ""

exec "$BIN" serve --config config.toml
