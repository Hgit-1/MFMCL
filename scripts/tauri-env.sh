#!/usr/bin/env bash
# Build environment for the Tauri desktop shell in this container.
#
# The container has the GTK/WebKit *runtime* libraries but not the matching
# `-dev` packages, and there is no root access to install them. Instead a
# local sysroot is populated from `apt-get download`-ed dev packages and
# pkg-config is pointed at it. Runtime .so files are symlinked to the
# system libraries so the linker resolves them.
#
# Usage:  source scripts/tauri-env.sh   then   cargo build -p mfmcl-desktop
SYSROOT=${SYSROOT:-/tmp/mfmcl-sysroot/root}

export PKG_CONFIG_PATH="$SYSROOT/usr/lib/x86_64-linux-gnu/pkgconfig:$SYSROOT/usr/share/pkgconfig"
export PKG_CONFIG_SYSROOT_DIR="$SYSROOT"
export LIBRARY_PATH="$SYSROOT/usr/lib/x86_64-linux-gnu${LIBRARY_PATH:+:$LIBRARY_PATH}"
export LD_LIBRARY_PATH="/usr/lib/x86_64-linux-gnu${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export PATH="$HOME/.cargo/bin:$PATH"

# 加载仓库根目录的 `.env`（如果存在）。
#
# 这里只导出文件里出现的变量，不执行文件内容——`.env` 是可以被随便写的东西，
# 用 `source` 会让其中任意一行 shell 代码被执行。只认 `KEY='value'` /
# `KEY=value` 这两种形态，且忽略注释与空行。
_ENV_FILE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/.env"
if [ -f "$_ENV_FILE" ]; then
  while IFS= read -r _line || [ -n "$_line" ]; do
    case "$_line" in
      ''|\#*) continue ;;
    esac
    _key=${_line%%=*}
    _value=${_line#*=}
    # 去掉包裹的成对引号；不处理转义，保持简单可预测。
    case "$_value" in
      \'*\') _value=${_value#\'}; _value=${_value%\'} ;;
      \"*\") _value=${_value#\"}; _value=${_value%\"} ;;
    esac
    case "$_key" in
      ''|*[!A-Za-z0-9_]*) continue ;;
    esac
    export "$_key=$_value"
  done < "$_ENV_FILE"
  unset _line _key _value
fi
unset _ENV_FILE
