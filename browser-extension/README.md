# Vortex Studio Browser Extension (IDM Video & Stream Catcher)

Adds an IDM-style floating **`⬇ Download with Vortex ▾`** bar on `<video>` players (YouTube, Vimeo, HTML5, HLS `.m3u8`), intercepts live browser media streams, and sends downloads straight to the **Vortex Studio** desktop app (`http://127.0.0.1:47391`).

## Quick Install

1. Start **Vortex Studio** desktop (`cargo run -p vortex-app` inside `/home/firoze/projects/others/vortex`).

### Brave / Google Chrome / Microsoft Edge
1. Open the Extensions page:
   - **Brave:** `brave://extensions`
   - **Chrome:** `chrome://extensions`
   - **Edge:** `edge://extensions`
2. Enable **Developer mode** (top-right toggle).
3. Click **Load unpacked** and select:
   `/home/firoze/projects/others/vortex/browser-extension`

### Mozilla Firefox
1. Open `about:debugging#/runtime/this-firefox`.
2. Click **Load Temporary Add-on...**.
3. Select:
   `/home/firoze/projects/others/vortex/browser-extension-firefox/manifest.json`

### NovaOS Aero Browser
No browser extension required — Aero Browser (`aero-ui`) integrates the **Vortex Studio** floating badge and bridge client natively on watch pages.
