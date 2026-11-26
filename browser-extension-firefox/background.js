// Vortex Studio — Cross-Browser Background Media Sniffer & Bridge Client
// Works identically in Chrome, Brave, Microsoft Edge (MV3 Service Worker), and Firefox (Background Script).

const BRIDGE_BASE = "http://127.0.0.1:47391";
const MAX_STREAMS_PER_TAB = 20;

// In-memory store of media streams intercepted per tabId
const tabStreams = new Map();

function cleanMediaUrl(rawUrl) {
  try {
    const u = new URL(rawUrl);
    // YouTube/Googlevideo requests include byte-range query params for MSE buffering.
    // Removing range/rn/rbuf lets Vortex request the full stream cleanly.
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

function labelForYouTubeItag(itag, mime) {
  const map = {
    "18": "360p MP4 (Video+Audio)",
    "22": "720p MP4 (Video+Audio)",
    "37": "1080p MP4 (Video+Audio)",
    "137": "1080p MP4 (Video)",
    "299": "1080p 60fps MP4",
    "136": "720p MP4 (Video)",
    "298": "720p 60fps MP4",
    "135": "480p MP4 (Video)",
    "134": "360p MP4 (Video)",
    "248": "1080p WebM",
    "247": "720p WebM",
    "313": "4K 2160p WebM",
    "401": "4K 2160p AV1",
    "140": "Audio M4A (128k)",
    "251": "Audio Opus (160k)"
  };
  if (itag && map[itag]) return map[itag];
  if (mime.includes("audio")) return itag ? `Audio (${itag})` : "Audio Stream";
  if (mime.includes("webm")) return itag ? `WebM (${itag})` : "WebM Video";
  return itag ? `MP4 (${itag})` : "Video Stream";
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
      mime = qMime;
      isAudioOnly = qMime.includes("audio");
      label = labelForYouTubeItag(itag, qMime);
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
    mime,
    size: contentLength,
    isHls,
    isAudioOnly,
    timestamp: Date.now()
  };
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
    if (list.length > MAX_STREAMS_PER_TAB) {
      list.length = MAX_STREAMS_PER_TAB;
    }
  }
  tabStreams.set(tabId, list);
  updateActionBadge(tabId, list.length);
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
            referer: msg.referer || null
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
