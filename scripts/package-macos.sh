#!/bin/sh
# Build the plugin and its speech helper, and stage them for install:
#   dist/audio/plugin.json
#   dist/audio/lib/libschrodinger_plugin_audio.dylib
#   dist/audio/lib/schrodinger-speech      (the transcription helper)
set -eu
cd "$(dirname "$0")/.."

# CARGO_FLAGS passes extra arguments, e.g. a --config patch pointing at a
# local checkout of the SDK.
# shellcheck disable=SC2086
cargo build --release ${CARGO_FLAGS:-}

OUT=dist/audio
rm -rf "$OUT"
mkdir -p "$OUT/lib"
cp plugin.json "$OUT/"
cp target/release/libschrodinger_plugin_audio.dylib "$OUT/lib/"

swiftc -parse-as-library -O -target arm64-apple-macos13.0 helper/main.swift -o "$OUT/lib/schrodinger-speech"

# Ad hoc unless CODESIGN_IDENTITY names a certificate.
codesign --force --sign "${CODESIGN_IDENTITY:--}" "$OUT/lib/schrodinger-speech" "$OUT/lib/libschrodinger_plugin_audio.dylib"

echo "Staged in $OUT. Install with:"
echo "  rsync -a --delete $OUT/ ~/.schrodinger/plugins/audio/"
