#!/bin/sh
# Restores the binary files that the GitHub text-only mirror of this
# vendored tree cannot carry.
#
# Only ONE binary file is needed to build the engine: the fallback device
# font embedded by ruffle_core (core/src/player.rs). Every other binary
# file in this tree is Ruffle's own test data (test .swf/.fla movies,
# images, audio) -- needed only to run Ruffle's test suite, not to build
# Dovahengine.
#
# Usage: run from this directory (vendor/ruffle/):
#   sh restore-binaries.sh
#
# It fetches the font from upstream ruffle-rs/ruffle at the exact pinned
# commit (v0.7.1, 89a7049965f3090e706265670df309a5128233ac) and verifies
# its SHA-256 before accepting it. To restore ALL binary files instead,
# clone upstream at that tag or use the with-vendor source ZIP.
set -eu
cd "$(dirname "$0")"
COMMIT=89a7049965f3090e706265670df309a5128233ac
FILE=core/assets/notosans.subset.ttf.gz
SHA256=7169d53b641ef63abb98fc771ff1c944b53fbeab3ed040ed57d1f01339ffa32b
if [ -f "$FILE" ]; then
  if [ "$(sha256sum "$FILE" | cut -d' ' -f1)" = "$SHA256" ]; then
    echo "already present and verified: $FILE"
    exit 0
  fi
  echo "refusing to overwrite $FILE: present but hash differs" >&2
  exit 1
fi
echo "fetching $FILE ..."
curl -fsSL -o "$FILE.tmp" "https://raw.githubusercontent.com/ruffle-rs/ruffle/$COMMIT/$FILE"
if [ "$(sha256sum "$FILE.tmp" | cut -d' ' -f1)" != "$SHA256" ]; then
  echo "hash mismatch after download, refusing to install" >&2
  rm -f "$FILE.tmp"
  exit 1
fi
mv "$FILE.tmp" "$FILE"
echo "restored and verified: $FILE"
