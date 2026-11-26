// Vortex Studio — Cross-Browser Background Media Sniffer & Bridge Client
// Works identically in Chrome, Brave, Microsoft Edge (MV3 Service Worker), and Firefox (Background Script).

const BRIDGE_BASE = "http://127.0.0.1:47391";
const MAX_STREAMS_PER_TAB = 20;

// In-memory store of media streams intercepted per tabId
const tabStreams = new Map();
const sniffSyncTimers = new Map();

function cleanMediaUrl(rawUrl) {
  try {
    const u = new URL(rawUrl);
    // YouTube/Googlevideo requests include byte-range query params for MSE buffering.
    // Removing range/rn/rbuf lets Vortex request the full stream while keeping the browser PO-Token (`pot=`).
    if (u.hostname.includes("googlevideo.com") && u.pathname.includes("videoplayback")) {
      u.searchParams.delete("range");
      u.searchParams.delete("rn");
      u.searchParams.delete("rbuf");
      u.searchParams.delete("ump");
      u.searchParams.delete("srfvp");
    }
    return u.toString();
  } catch {
    return rawUrl;
  }
}

function youtubeMetaForItag(itag, mime) {
  const table = {
    "18": { label: "360p MP4 (Video+Audio)", res: [640, 360], hasAudio: true },
    "22": { label: "720p MP4 (Video+Audio)", res: [1280, 720], hasAudio: true },
    "37": { label: "1080p MP4 (Video+Audio)", res: [1920, 1080], hasAudio: true },
    "137": { label: "1080p MP4 (HD Video)", res: [1920, 1080], hasAudio: false },
    "299": { label: "1080p 60fps MP4", res: [1920, 1080], hasAudio: false },
    "399": { label: "1080p AV1 Video", res: [1920, 1080], hasAudio: false },
    "136": { label: "720p MP4 (HD Video)", res: [1280, 720], hasAudio: false },
    "298": { label: "720p 60fps MP4", res: [1280, 720], hasAudio: false },
    "135": { label: "480p MP4 (Video)", res: [854, 480], hasAudio: false },
    "134": { label: "360p MP4 (Video)", res: [640, 360], hasAudio: false },
    "248": { label: "1080p WebM (HD)", res: [1920, 1080], hasAudio: false },
    "247": { label: "720p WebM (HD)", res: [1280, 720], hasAudio: false },
    "271": { label: "1440p QHD WebM", res: [2560, 1440], hasAudio: false },
    "308": { label: "1440p 60fps WebM", res: [2560, 1440], hasAudio: false },
    "400": { label: "1440p AV1", res: [2560, 1440], hasAudio: false },
    "313": { label: "4K 2160p WebM", res: [3840, 2160], hasAudio: false },
    "315": { label: "4K 2160p 60fps WebM", res: [3840, 2160], hasAudio: false },
    "401": { label: "4K 2160p AV1", res: [3840, 2160], hasAudio: false },
    "140": { label: "Audio M4A (128k)", res: [0, 0], hasAudio: true },
    "251": { label: "Audio Opus (160k)", res: [0, 0], hasAudio: true }
  };
  if (itag && table[itag]) return table[itag];
  if (mime.includes("audio")) return { label: itag ? `Audio (${itag})` : "Audio Stream", res: [0, 0], hasAudio: true };
  if (mime.includes("webm")) return { label: itag ? `WebM (${itag})` : "WebM Video", res: [1280, 720], hasAudio: false };
  return { label: itag ? `MP4 (${itag})` : "Video Stream", res: [1280, 720], hasAudio: false };
}

function classifyMediaRequest(url, contentType, contentLength) {
  const lowUrl = url.toLowerCase();
  const lowCt = (contentType || "").toLowerCase();

  // Ignore local Vortex bridge calls and extension internals
  if (lowUrl.startsWith(BRIDGE_BASE) || lowUrl.startsWith("chrome-extension:") || lowUrl.startsWith("moz-extension:")) {
    return null;
  }

  const isHls = lowUrl.includes(".m3u8") || lowCt.includes("mpegurl") || lowCt.includes("vnd.apple");
  const isDash = lowUrl.includes(".mpd") || lowCt.includes("dash+xml");
  const isGoogleVideo = lowUrl.includes("googlevideo.com/videoplayback");
  const isVideoExt = /\.(mp4|webm|mkv|mov|flv|m4v|ts)(\?|$)/i.test(lowUrl);
  const isAudioExt = /\.(mp3|m4a|aac|opus|ogg|flac)(\?|$)/i.test(lowUrl);
  const isVideoCt = lowCt.startsWith("video/");
  const isAudioCt = lowCt.startsWith("audio/");

  if (!isHls && !isDash && !isGoogleVideo && !isVideoExt && !isAudioExt && !isVideoCt && !isAudioCt) {
    return null;
  }

  // Skip tiny tracking blobs (< 64 KB) unless HLS manifest or YouTube playback
  if (contentLength > 0 && contentLength < 65536 && !isHls && !isGoogleVideo) {
    return null;
  }

  let label = "Video Stream";
  let res = [0, 0];
  let hasAudio = true;
  let totalSize = contentLength;
  let isAudioOnly = isAudioCt || isAudioExt;
  let mime = contentType || (isHls ? "application/x-mpegURL" : "video/mp4");

  if (isHls) {
    label = "HLS Stream (.m3u8)";
  } else if (isDash) {
    label = "DASH Manifest (.mpd)";
  } else if (isGoogleVideo) {
    try {
      const u = new URL(url);
      const itag = u.searchParams.get("itag") || "";
      const qMime = u.searchParams.get("mime") || mime;
      const clen = parseInt(u.searchParams.get("clen") || "0", 10) || 0;
      if (clen > 0) totalSize = clen;
      mime = qMime.split(";")[0];
      isAudioOnly = qMime.includes("audio");
      const meta = youtubeMetaForItag(itag, qMime);
      label = meta.label;
      res = meta.res;
      hasAudio = meta.hasAudio;
    } catch {
      label = "YouTube Media";
    }
  } else if (isAudioOnly) {
    label = "Audio Stream";
  } else if (lowUrl.includes(".mp4")) {
    label = "MP4 Video";
  } else if (lowUrl.includes(".webm")) {
    label = "WebM Video";
  }

  return {
    url: cleanMediaUrl(url),
    rawUrl: url,
    label,
    res,
    hasAudio,
    mime,
    size: totalSize,
    isHls,
    isAudioOnly,
    timestamp: Date.now()
  };
}

function scheduleBridgeSniffSync(tabId) {
  if (sniffSyncTimers.has(tabId)) return;
  const timer = setTimeout(() => {
    sniffSyncTimers.delete(tabId);
    const streams = tabStreams.get(tabId);
    if (!streams || streams.length === 0) return;
    chrome.tabs.get(tabId, (tab) => {
      if (chrome.runtime.lastError || !tab || !tab.url || !tab.url.startsWith("http")) return;
      const payload = {
        page_url: tab.url,
        title: tab.title || "Browser Video",
        thumbnail_url: null,
        qualities: streams.slice(0, 12).map((s) => ({
          label: `${s.label} [Browser Auth]`,
          resolution: s.res || [0, 0],
          mime_type: s.mime || "video/mp4",
          estimated_size_bytes: s.size || 0,
          url: s.url,
          bitrate_kbps: 0,
          is_audio_only: Boolean(s.isAudioOnly),
          is_hls: Boolean(s.isHls),
          has_audio: s.hasAudio !== undefined ? Boolean(s.hasAudio) : true
        }))
      };
      bridgeFetch("/api/sniff", {
        method: "POST",
        body: JSON.stringify(payload)
      }).catch(() => {});
    });
  }, 700);
  sniffSyncTimers.set(tabId, timer);
}

function recordTabStream(tabId, info) {
  if (tabId < 0) return;
  const list = tabStreams.get(tabId) || [];
  // Deduplicate by cleaned URL or identical (label + roughly same URL path)
  const existingIdx = list.findIndex((x) => x.url === info.url);
  if (existingIdx >= 0) {
    list[existingIdx] = { ...list[existingIdx], ...info };
  } else {
    list.unshift(info);
    // Sort higher resolutions first in list
    list.sort((a, b) => ((b.res?.[1] || 0) - (a.res?.[1] || 0)));
    if (list.length > MAX_STREAMS_PER_TAB) {
      list.length = MAX_STREAMS_PER_TAB;
    }
  }
  tabStreams.set(tabId, list);
  updateActionBadge(tabId, list.length);
  scheduleBridgeSniffSync(tabId);
}

function updateActionBadge(tabId, count) {
  try {
    if (!chrome.action || !chrome.action.setBadgeText) return;
    chrome.action.setBadgeText({ tabId, text: count > 0 ? String(count) : "" });
    if (count > 0 && chrome.action.setBadgeBackgroundColor) {
      chrome.action.setBadgeBackgroundColor({ tabId, color: "#16a34a" });
    }
  } catch {
    // Ignore errors when tab has already closed
  }
}

chrome.webRequest.onHeadersReceived.addListener(
  (details) => {
    if (details.tabId < 0 || !details.responseHeaders) return;
    let contentType = "";
    let contentLength = 0;

    for (const h of details.responseHeaders) {
      const name = (h.name || "").toLowerCase();
      if (name === "content-type") contentType = h.value || "";
      else if (name === "content-length") contentLength = parseInt(h.value || "0", 10) || 0;
    }

    const classified = classifyMediaRequest(details.url, contentType, contentLength);
    if (classified) {
      recordTabStream(details.tabId, classified);
    }
  },
  { urls: ["<all_urls>"] },
  ["responseHeaders"]
);

chrome.tabs.onRemoved.addListener((tabId) => {
  tabStreams.delete(tabId);
});

chrome.tabs.onUpdated.addListener((tabId, changeInfo) => {
  if (changeInfo.url) {
    // Keep only recent streams if it is a SPA transition, clear on hard cross-origin navigation
    try {
      const old = tabStreams.get(tabId);
      if (old && old.length > 0 && Date.now() - old[0].timestamp > 15000) {
        tabStreams.delete(tabId);
        updateActionBadge(tabId, 0);
      }
    } catch {}
  }
});

async function bridgeFetch(path, options = {}) {
  const resp = await fetch(`${BRIDGE_BASE}${path}`, {
    ...options,
    headers: {
      "Content-Type": "application/json",
      ...(options.headers || {})
    }
  });
  const data = await resp.json().catch(() => ({}));
  if (!resp.ok) {
    throw new Error(data.error || `Bridge HTTP ${resp.status}`);
  }
  return data;
}

chrome.runtime.onMessage.addListener((msg, sender, sendResponse) => {
  (async () => {
    try {
      if (msg.type === "GET_TAB_STREAMS") {
        const tabId = msg.tabId ?? sender.tab?.id ?? -1;
        sendResponse({ ok: true, streams: tabStreams.get(tabId) || [] });
        return;
      }

      if (msg.type === "CHECK_HEALTH") {
        const data = await bridgeFetch("/api/health", { method: "GET" });
        sendResponse({ ok: true, data });
        return;
      }

      if (msg.type === "GET_TASKS") {
        const data = await bridgeFetch("/api/tasks", { method: "GET" });
        sendResponse({ ok: true, data });
        return;
      }

      if (msg.type === "FETCH_PREVIEW") {
        const data = await bridgeFetch("/api/preview", {
          method: "POST",
          body: JSON.stringify({ url: msg.url })
        });
        sendResponse({ ok: true, data });
        return;
      }

      if (msg.type === "REPORT_SNIFF") {
        const data = await bridgeFetch("/api/sniff", {
          method: "POST",
          body: JSON.stringify(msg.payload)
        });
        sendResponse({ ok: true, data });
        return;
      }

      if (msg.type === "SEND_DOWNLOAD") {
        const data = await bridgeFetch("/api/download/stream", {
          method: "POST",
          body: JSON.stringify({
            url: msg.url,
            title: msg.title || "",
            filename: msg.filename || "",
            quality: msg.quality || "Auto",
            size: msg.size || 0,
            threads: msg.threads || 8,
            is_hls: Boolean(msg.isHls),
            referer: msg.referer || null,
            audio_url: msg.audioUrl || null
          })
        });
        sendResponse({ ok: true, data });
        return;
      }

      if (msg.type === "SEND_TORRENT") {
        const data = await bridgeFetch("/api/download/torrent", {
          method: "POST",
          body: JSON.stringify({
            url: msg.url,
            sequential: Boolean(msg.sequential)
          })
        });
        sendResponse({ ok: true, data });
        return;
      }

      sendResponse({ ok: false, error: `Unknown message type: ${msg.type}` });
    } catch (err) {
      sendResponse({ ok: false, error: String(err?.message || err) });
    }
  })();
  return true;
});
