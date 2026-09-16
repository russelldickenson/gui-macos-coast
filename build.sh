#!/bin/sh
# Builds a release Coast.app bundle: target/release/bundle/Coast.app
set -eu

ROOT="$(cd "$(dirname "$0")" && pwd)"
"$ROOT/packaging/macos/bundle.sh" release
