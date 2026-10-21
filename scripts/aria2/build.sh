#!/usr/bin/env bash
# Build ONLY the hash-locked official release. Run from bash (MSYS2 on Windows).
set -euo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
WORK="${1:?Usage: bash scripts/aria2/build.sh ABSOLUTE_EMPTY_WORK_DIR}"
case "$WORK" in /* | [A-Za-z]:/*) ;; *) echo 'Work directory must be absolute' >&2; exit 2 ;; esac
PYTHON="${PYTHON:-python3}"
export LC_ALL=C TZ=UTC
export SOURCE_DATE_EPOCH=1700041692
export ZERO_AR_DATE=1
# Keep optional dependencies and user flags from silently changing the feature set.
unset CFLAGS CXXFLAGS CPPFLAGS LDFLAGS LIBS CONFIG_SITE
export CONFIG_SITE=/dev/null
export CFLAGS='-O2 -g0'
export CXXFLAGS='-O2 -g0'
"$PYTHON" "$SCRIPT_DIR/distribute.py" prepare --work "$WORK"
cd "$WORK/build"
SOURCE="$(cd ../source/aria2-1.37.0 && pwd)"
COMMON=(
  --prefix=/usr --disable-nls --disable-shared --enable-static
  --disable-bittorrent --disable-metalink --disable-websocket --disable-libaria2
  --without-libxml2 --without-libexpat --without-libssh2
  --without-libcares --without-libz --without-sqlite3
  --without-libuv --without-tcmalloc --without-jemalloc --enable-option-checking=fatal
  --without-gnutls --without-libnettle --without-libgmp --without-libgcrypt
)
case "$(uname -s)" in
  Linux)
    export CC="${CC:-gcc}" CXX="${CXX:-g++}"
    pkg-config --exists 'openssl >= 3.0' 'openssl < 4' || { echo 'OpenSSL 3 development files required (PKG_CONFIG_PATH supported)' >&2; exit 1; }
    export CFLAGS="$CFLAGS -ffile-prefix-map=$WORK=/aria2-build -fdebug-prefix-map=$WORK=/aria2-build"
    export CXXFLAGS="$CXXFLAGS -ffile-prefix-map=$WORK=/aria2-build -fdebug-prefix-map=$WORK=/aria2-build"
    export LDFLAGS='-Wl,--build-id=none'
    COMMON+=(--with-openssl --without-appletls --without-wintls --with-ca-bundle=/etc/ssl/certs/ca-certificates.crt)
    ;;
  Darwin)
    export CC="${CC:-clang}" CXX="${CXX:-clang++}"
    export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-11.0}"
    export CFLAGS="$CFLAGS -ffile-prefix-map=$WORK=/aria2-build"
    export CXXFLAGS="$CXXFLAGS -ffile-prefix-map=$WORK=/aria2-build"
    COMMON+=(--without-openssl --with-appletls --without-wintls)
    ;;
  MINGW*|MSYS*)
    export CC="${CC:-gcc}" CXX="${CXX:-g++}"
    export CFLAGS="$CFLAGS -ffile-prefix-map=$WORK=/aria2-build"
    export CXXFLAGS="$CXXFLAGS -ffile-prefix-map=$WORK=/aria2-build"
    export LDFLAGS='-static -static-libgcc -static-libstdc++ -Wl,--no-insert-timestamp'
    COMMON+=(--without-openssl --without-appletls --with-wintls)
    ;;
  *) echo 'Supported builders: Linux, macOS, Windows MSYS2 MINGW64' >&2; exit 1 ;;
esac
# Save exact commands and tools before building; do not include the entire environment (secrets).
{
  printf 'source_date_epoch=%s\n' "$SOURCE_DATE_EPOCH"
  printf 'configure '; printf '%q ' "$SOURCE/configure" "${COMMON[@]}"; printf '\n'
  for name in CC CXX CFLAGS CXXFLAGS LDFLAGS PKG_CONFIG_PATH MACOSX_DEPLOYMENT_TARGET; do
    printf '%s=%s\n' "$name" "${!name-}"
  done
  uname -a
  "$CC" --version
  "$CXX" --version
  make --version
  ld -v 2>&1 || true
  pkg-config --modversion openssl 2>/dev/null || true
  if command -v dpkg-query >/dev/null; then dpkg-query -W 'gcc*' 'g++*' 'libssl*' 'libc6*' 'binutils*' 2>/dev/null || true; fi
  if command -v pacman >/dev/null; then pacman -Q; fi
  if command -v sw_vers >/dev/null; then sw_vers; xcodebuild -version; xcrun --show-sdk-version; fi
} > ../toolchain.txt
"$SOURCE/configure" "${COMMON[@]}" 2>&1 | tee ../configure.log
make -j"${JOBS:-4}" 2>&1 | tee ../make.log
BINARY=src/aria2c
[[ -f src/aria2c.exe ]] && BINARY=src/aria2c.exe
# macOS uses an ad-hoc signature so the final bytes (incl. arm64 signature) are hashed.
if [[ "$(uname -s)" == Darwin ]]; then codesign --force --sign - "$BINARY"; fi
"$BINARY" --version | tee ../version.txt
"$PYTHON" "$SCRIPT_DIR/distribute.py" package --work "$WORK" --binary "$BINARY"
