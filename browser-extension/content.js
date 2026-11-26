// Vortex Studio — IDM-Style Floating Video Download Pill (Content Script)
// Injects a floating "Download with Vortex" control beside HTML5 <video>, YouTube, Vimeo, and embedded players.

(() => {
  if (window.__vortexIdmInjected) return;
  window.__vortexIdmInjected = true;

  const MIN_VIDEO_W = 200;
  const MIN_VIDEO_H = 120;

  let currentVideo = null;
  let badgeRoot = null;
  let dropdownOpen = false;
  let isHoveringWidget = false;
  let cachedPageUrl = "";
  let cachedPreview = null;
  let previewPending = false;
  let toastTimeout = null;

  function sendMsg(payload) {
    return new Promise((resolve) => {
      try {
        chrome.runtime.sendMessage(payload, (resp) => {
          if (chrome.runtime.lastError) {
            resolve({ ok: false, error: chrome.runtime.lastError.message });
          } else {
            resolve(resp || { ok: false, error: "No response from Vortex extension" });
          }
        });
      } catch (e) {
        resolve({ ok: false, error: String(e) });
      }
    });
  }

  function formatSize(bytes) {
    if (!bytes || bytes <= 0) return "";
    if (bytes >= 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
    if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
    if (bytes >= 1024) return `${Math.round(bytes / 1024)} KB`;
    return `${bytes} B`;
  }

  function sanitizeFilename(name) {
    return (name || "video")
      .replace(/[\\/:*?"<>|]/g, "_")
      .replace(/\s+/g, " ")
      .trim()
      .slice(0, 80);
  }

  function pageVideoTitle() {
    // Prefer YouTube watch title if present
    const ytTitle = document.querySelector("h1.ytd-watch-metadata yt-formatted-string, #title h1");
    if (ytTitle && ytTitle.textContent.trim()) {
      return ytTitle.textContent.trim();
    }
    return document.title.replace(/\s*[-|]\s*YouTube$/i, "").trim() || "Vortex Video";
  }

  function findPrimaryVideo() {
    const videos = Array.from(document.querySelectorAll("video"));
    let best = null;
    let bestScore = -1;

    for (const v of videos) {
      const rect = v.getBoundingClientRect();
      if (rect.width < MIN_VIDEO_W || rect.height < MIN_VIDEO_H) continue;
      const style = window.getComputedStyle(v);
      if (style.display === "none" || style.visibility === "hidden" || style.opacity === "0") continue;

      const inViewport = rect.bottom > 0 && rect.top < window.innerHeight && rect.right > 0 && rect.left < window.innerWidth;
      const area = rect.width * rect.height;
      const score = area + (inViewport ? 1_000_000 : 0) + (!v.paused ? 2_000_000 : 0);
      if (score > bestScore) {
        bestScore = score;
        best = v;
      }
    }
    return best;
  }

  function ensureBadgeRoot() {
    if (badgeRoot && document.documentElement.contains(badgeRoot)) {
      return badgeRoot;
    }

    badgeRoot = document.createElement("div");
    badgeRoot.className = "vortex-idm-badge-root";
    badgeRoot.innerHTML = `
      <div class="vortex-idm-pill" role="button" tabindex="0" title="Download this video with Vortex Studio">
        <span class="vortex-idm-icon" aria-hidden="true">
          <svg viewBox="0 0 20 20" width="14" height="14" fill="none">
            <path d="M10 3v9m0 0l-3.5-3.5M10 12l3.5-3.5M4 15.5h12" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
          </svg>
        </span>
        <span class="vortex-idm-label">Download with Vortex</span>
        <span class="vortex-idm-caret" title="Choose quality">▾</span>
      </div>
      <div class="vortex-idm-dropdown" hidden></div>
    `;

    const pill = badgeRoot.querySelector(".vortex-idm-pill");
    const caret = badgeRoot.querySelector(".vortex-idm-caret");

    badgeRoot.addEventListener("mouseenter", () => {
      isHoveringWidget = true;
    });
    badgeRoot.addEventListener("mouseleave", () => {
      isHoveringWidget = false;
    });

    // Clicking the main pill opens the IDM quality chooser (or triggers default if Shift-clicked)
    pill.addEventListener("click", async (e) => {
      e.stopPropagation();
      e.preventDefault();
      if (e.target === caret || !dropdownOpen) {
        await toggleDropdown(true);
      } else {
        await toggleDropdown(false);
      }
    });

    document.documentElement.appendChild(badgeRoot);
    return badgeRoot;
  }

  function showToast(text, isError = false) {
    const root = ensureBadgeRoot();
    let toast = root.querySelector(".vortex-idm-toast");
    if (!toast) {
      toast = document.createElement("div");
      toast.className = "vortex-idm-toast";
      root.appendChild(toast);
    }
    toast.textContent = text;
    toast.classList.toggle("is-error", isError);
    toast.hidden = false;

    clearTimeout(toastTimeout);
    toastTimeout = setTimeout(() => {
      if (toast) toast.hidden = true;
    }, 4200);
  }

  async function ensurePreviewData() {
    const pageUrl = window.location.href;
    if (cachedPageUrl !== pageUrl) {
      cachedPageUrl = pageUrl;
      cachedPreview = null;
    }
    if (cachedPreview || previewPending) return cachedPreview;

    previewPending = true;
    const res = await sendMsg({ type: "FETCH_PREVIEW", url: pageUrl });
    previewPending = false;
    if (res.ok && res.data && !res.data.error) {
      cachedPreview = res.data;
    }
    return cachedPreview;
  }

  async function collectDownloadOptions() {
    const options = [];
    const seenUrls = new Set();

    // 1. Live streams captured by browser webRequest (holds active browser session auth!)
    const tabRes = await sendMsg({ type: "GET_TAB_STREAMS" });
    if (tabRes.ok && Array.isArray(tabRes.streams)) {
      for (const s of tabRes.streams) {
        if (!s.url || seenUrls.has(s.url)) continue;
        seenUrls.add(s.url);
        options.push({
          label: `${s.label} [Browser Auth]`,
          quality: s.label,
          url: s.url,
          size: s.size || 0,
          resHeight: (s.res && s.res[1]) || 0,
          isHls: Boolean(s.isHls),
          isAudioOnly: Boolean(s.isAudioOnly),
          source: "browser",
          gated: false
        });
      }
    }

    // 2. Direct non-blob HTML5 <video src="..."> element source
    if (currentVideo) {
      const directSrc = currentVideo.currentSrc || currentVideo.src;
      if (directSrc && /^https?:\/\//i.test(directSrc) && !seenUrls.has(directSrc)) {
        seenUrls.add(directSrc);
        options.push({
          label: "HTML5 Video Source",
          quality: "Original",
          url: directSrc,
          size: 0,
          isHls: directSrc.includes(".m3u8"),
          isAudioOnly: false,
          source: "dom",
          gated: false
        });
      }
    }

    // 3. Server-side preview qualities from Vortex Desktop (127.0.0.1:47391/api/preview)
    const preview = await ensurePreviewData();
    if (preview && Array.isArray(preview.qualities)) {
      for (const q of preview.qualities) {
        if (!q.url || seenUrls.has(q.url)) continue;
        seenUrls.add(q.url);
        const gated = (q.label || "").includes("[PO-Token Gated]");
        options.push({
          label: q.label || "Video Stream",
          quality: q.label || "Auto",
          url: q.url,
          size: q.estimated_size_bytes || 0,
          resHeight: (q.resolution && q.resolution[1]) || 0,
          isHls: Boolean(q.is_hls) || q.url.includes(".m3u8"),
          isAudioOnly: Boolean(q.is_audio_only),
          audioUrl: q.audio_url || null,
          source: "vortex",
          gated
        });
      }
    }

    // Sort: unlocked highest resolution first, then other unlocked, then [PO-Token Gated]
    options.sort((a, b) => {
      if (Number(a.gated) !== Number(b.gated)) return Number(a.gated) - Number(b.gated);
      if (Number(a.isAudioOnly) !== Number(b.isAudioOnly)) return Number(a.isAudioOnly) - Number(b.isAudioOnly);
      return (b.resHeight || 0) - (a.resHeight || 0);
    });
    return options;
  }

  async function triggerDownload(opt) {
    const title = (cachedPreview && cachedPreview.title) || pageVideoTitle();
    const ext = opt.isHls ? "ts" : opt.url.includes(".webm") ? "webm" : opt.isAudioOnly ? "m4a" : "mp4";
    const filename = `${sanitizeFilename(title)}.${ext}`;

    showToast(`Sending to Vortex: ${opt.quality}...`);
    const res = await sendMsg({
      type: "SEND_DOWNLOAD",
      url: opt.url,
      title,
      filename,
      quality: opt.quality,
      size: opt.size || 0,
      threads: 8,
      isHls: opt.isHls,
      referer: window.location.href,
      audioUrl: opt.audioUrl || null
    });

    if (res.ok && res.data?.ok) {
      showToast(`Queued in Vortex: ${res.data.filename || filename}`);
      dropdownOpen = false;
      renderDropdown([]);
    } else {
      const err = res.data?.error || res.error || "Is Vortex Desktop running on port 47391?";
      showToast(`Vortex Bridge Offline: ${err}`, true);
    }
  }

  function renderDropdown(options, loading = false) {
    if (!badgeRoot) return;
    const dd = badgeRoot.querySelector(".vortex-idm-dropdown");
    dd.hidden = !dropdownOpen;
    if (!dropdownOpen) return;

    dd.innerHTML = "";

    const header = document.createElement("div");
    header.className = "vortex-dd-header";
    header.innerHTML = `<span class="vortex-dd-title">${sanitizeFilename(pageVideoTitle())}</span><span class="vortex-dd-tag">Vortex IDM</span>`;
    dd.appendChild(header);

    if (loading) {
      const row = document.createElement("div");
      row.className = "vortex-dd-empty";
      row.textContent = "Scanning streams & formats...";
      dd.appendChild(row);
      return;
    }

    if (!options || options.length === 0) {
      const empty = document.createElement("div");
      empty.className = "vortex-dd-empty";
      empty.textContent = "No direct stream detected yet — press Play for 1s, then reopen.";
      dd.appendChild(empty);
      return;
    }

    for (const opt of options.slice(0, 10)) {
      const item = document.createElement("button");
      item.type = "button";
      item.className = "vortex-dd-item" + (opt.gated ? " is-gated" : "");

      const left = document.createElement("span");
      left.className = "vortex-dd-label";
      left.textContent = opt.label;

      const right = document.createElement("span");
      right.className = "vortex-dd-meta";
      right.textContent = formatSize(opt.size) || (opt.source === "browser" ? "Live" : "Stream");

      item.appendChild(left);
      item.appendChild(right);
      item.addEventListener("click", (e) => {
        e.stopPropagation();
        e.preventDefault();
        triggerDownload(opt);
      });
      dd.appendChild(item);
    }
  }

  async function toggleDropdown(forceState) {
    dropdownOpen = typeof forceState === "boolean" ? forceState : !dropdownOpen;
    if (!dropdownOpen) {
      renderDropdown([]);
      return;
    }

    renderDropdown([], true);
    const options = await collectDownloadOptions();
    if (dropdownOpen) {
      renderDropdown(options, false);
    }
  }

  function positionBadge() {
    const video = findPrimaryVideo();
    currentVideo = video;

    if (!video) {
      if (badgeRoot && !dropdownOpen && !isHoveringWidget) {
        badgeRoot.style.display = "none";
      }
      return;
    }

    const rect = video.getBoundingClientRect();
    const visible = rect.bottom > 10 && rect.top < window.innerHeight - 10 && rect.right > 40 && rect.left < window.innerWidth;
    const root = ensureBadgeRoot();

    if (!visible) {
      if (!dropdownOpen) root.style.display = "none";
      return;
    }

    root.style.display = "block";
    // Top-right corner of the video player (IDM signature bar placement)
    const pillWidth = root.offsetWidth || 195;
    const left = Math.max(8, Math.min(window.innerWidth - pillWidth - 12, rect.right - pillWidth - 10));
    const top = Math.max(8, Math.min(window.innerHeight - 44, rect.top + 8));

    root.style.left = `${Math.round(left)}px`;
    root.style.top = `${Math.round(top)}px`;
  }

  // Close dropdown when clicking outside the badge
  document.addEventListener("click", (e) => {
    if (dropdownOpen && badgeRoot && !badgeRoot.contains(e.target)) {
      dropdownOpen = false;
      renderDropdown([]);
    }
  });

  // React cleanly to SPA navigation (YouTube yt-navigate-finish, history API, DOM changes)
  window.addEventListener("yt-navigate-finish", () => {
    cachedPreview = null;
    dropdownOpen = false;
    setTimeout(positionBadge, 400);
  });
  window.addEventListener("popstate", () => {
    cachedPreview = null;
    setTimeout(positionBadge, 300);
  });
  window.addEventListener("scroll", positionBadge, { passive: true });
  window.addEventListener("resize", positionBadge, { passive: true });
  document.addEventListener("fullscreenchange", () => setTimeout(positionBadge, 150));

  const observer = new MutationObserver(() => {
    positionBadge();
  });
  observer.observe(document.documentElement, { childList: true, subtree: true });

  setInterval(positionBadge, 1100);
  setTimeout(positionBadge, 500);
})();
