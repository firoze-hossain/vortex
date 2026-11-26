#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
FF_DIR="$(cd "${ROOT_DIR}/../browser-extension-firefox" && pwd)"

mkdir -p "${FF_DIR}/icons"
cp "${ROOT_DIR}/background.js" "${FF_DIR}/background.js"
cp "${ROOT_DIR}/content.js" "${FF_DIR}/content.js"
cp "${ROOT_DIR}/content.css" "${FF_DIR}/content.css"
cp "${ROOT_DIR}/popup.html" "${FF_DIR}/popup.html"
cp "${ROOT_DIR}/popup.js" "${FF_DIR}/popup.js"
cp "${ROOT_DIR}/README.md" "${FF_DIR}/README.md"
cp "${ROOT_DIR}/icons/"*.png "${FF_DIR}/icons/"

echo "Synced shared extension assets to ${FF_DIR}"
