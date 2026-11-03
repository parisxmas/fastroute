#!/bin/sh
# Builds the KiCad Plugin and Content Manager (PCM) package.
#
# Usage: integrations/kicad/package.sh [--pgo] [--bin DIR/fastroute[.exe] PLATFORM]...
#   Without --bin, the fastroute binary for the current platform is built
#   (release, or PGO with --pgo) and bundled as plugins/bin/<platform>/.
#   --bin may be repeated to bundle prebuilt binaries for more platforms
#   (platform tags: macos-arm64, macos-x64, linux-x64, linux-arm64, windows-x64,
#   windows-arm64).
# Output: dist/fastroute-kicad-<version>.zip (install via PCM "Install from File").
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
TARGET=${CARGO_TARGET_DIR:-$ROOT/target}
VERSION=$(python3 -c "import json;print(json.load(open('$HERE/metadata.json'))['versions'][0]['version'])")
STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT

pgo=0; bins=""
while [ $# -gt 0 ]; do
  case $1 in
    --pgo) pgo=1; shift ;;
    --bin) bins="$bins $2:$3"; shift 3 ;;
    *) echo "unknown argument $1" >&2; exit 2 ;;
  esac
done

mkdir -p "$STAGE/plugins" "$STAGE/resources"
cp "$HERE"/plugins/*.py "$HERE"/plugins/icon_24x24.png "$STAGE/plugins/"
cp "$HERE/resources/icon.png" "$STAGE/resources/"

if [ -z "$bins" ]; then
  tag=$(cd "$HERE/plugins" && python3 -c "
import platform
s={'Darwin':'macos','Windows':'windows','Linux':'linux'}.get(platform.system(),platform.system().lower())
m=platform.machine().lower(); a={'amd64':'x64','x86_64':'x64','arm64':'arm64','aarch64':'arm64'}.get(m,m)
print(f'{s}-{a}')")
  if [ $pgo = 1 ]; then
    "$ROOT/scripts/build-pgo.sh"
  else
    (cd "$ROOT" && cargo build --release -p fastroute)
  fi
  bins="$TARGET/release/fastroute:$tag"
fi
for b in $bins; do
  path=${b%:*}; tag=${b##*:}
  mkdir -p "$STAGE/plugins/bin/$tag"
  cp "$path" "$STAGE/plugins/bin/$tag/"
  case $tag in
    windows-*) ;;
    *) strip "$STAGE/plugins/bin/$tag/fastroute" 2>/dev/null || true ;;
  esac
  echo "bundled $path as $tag"
done

# PCM metadata inside the archive carries only the version entry itself.
python3 - "$HERE/metadata.json" "$STAGE/metadata.json" <<'PY'
import json, sys
m = json.load(open(sys.argv[1]))
m["versions"] = [{k: v for k, v in m["versions"][0].items() if k in ("version", "status", "kicad_version")}]
json.dump(m, open(sys.argv[2], "w"), indent=2)
PY

mkdir -p "$ROOT/dist"
out="$ROOT/dist/fastroute-kicad-$VERSION.zip"
rm -f "$out"
(cd "$STAGE" && zip -qr "$out" metadata.json plugins resources)
echo "wrote $out ($(du -h "$out" | cut -f1))"
