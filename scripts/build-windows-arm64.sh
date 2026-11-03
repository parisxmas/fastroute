#!/bin/sh
# Cross-compiles fastroute.exe for Windows on ARM64 (aarch64-pc-windows-msvc) with clang-cl,
# lld-link and the MSVC CRT + Windows SDK fetched by xwin
# (`brew install llvm lld`, `cargo install xwin`, `rustup target add aarch64-pc-windows-msvc`,
#  `xwin --accept-license --arch aarch64 splat --output <XWIN>`).
# The CRT is linked statically; the binary only depends on system DLLs (Windows 10+).
#
# Usage: XWIN=<splat dir> scripts/build-windows-arm64.sh   -> dist/windows-arm64/fastroute.exe
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
TARGET=${CARGO_TARGET_DIR:-$ROOT/target}
XWIN=${XWIN:-$HOME/.xwin-arm64}
LLVM=${LLVM:-$(brew --prefix llvm)/bin}
T=aarch64-pc-windows-msvc
inc="-imsvc $XWIN/crt/include -imsvc $XWIN/sdk/include/ucrt -imsvc $XWIN/sdk/include/um -imsvc $XWIN/sdk/include/shared"
export CC_aarch64_pc_windows_msvc="$LLVM/clang-cl" CXX_aarch64_pc_windows_msvc="$LLVM/clang-cl"
export AR_aarch64_pc_windows_msvc="$LLVM/llvm-lib"
export CFLAGS_aarch64_pc_windows_msvc="--target=$T $inc" CXXFLAGS_aarch64_pc_windows_msvc="--target=$T $inc"
export CARGO_TARGET_AARCH64_PC_WINDOWS_MSVC_LINKER=${CARGO_TARGET_AARCH64_PC_WINDOWS_MSVC_LINKER:-lld-link}
export CARGO_TARGET_AARCH64_PC_WINDOWS_MSVC_RUSTFLAGS="-Lnative=$XWIN/crt/lib/aarch64 -Lnative=$XWIN/sdk/lib/um/aarch64 -Lnative=$XWIN/sdk/lib/ucrt/aarch64 -Ctarget-feature=+crt-static"
(cd "$ROOT" && CARGO_PROFILE_RELEASE_STRIP=symbols cargo build --release -p fastroute --target $T)
mkdir -p "$ROOT/dist/windows-arm64"
cp "$TARGET/$T/release/fastroute.exe" "$ROOT/dist/windows-arm64/fastroute.exe"
echo "wrote dist/windows-arm64/fastroute.exe ($(du -h "$ROOT/dist/windows-arm64/fastroute.exe" | cut -f1))"
