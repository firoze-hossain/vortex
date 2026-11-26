const healthBadge = document.getElementById("healthBadge");
const scanPageBtn = document.getElementById("scanPageBtn");
const streamList = document.getElementById("streamList");
const taskList = document.getElementById("taskList");
const statusMsg = document.getElementById("statusMsg");

function sendMsg(payload) {
  return new Promise((resolve) => {
    chrome.runtime.sendMessage(payload, (resp) => {
      if (chrome.runtime.lastError) {
        resolve({ ok: false, error: chrome.runtime.lastError.message });
      } else {
        resolve(resp || { ok: false, error: "No response" });
      }
    });
  });
}

function setStatus(text, isErr = false) {
  statusMsg.textContent = text;
  statusMsg.classList.toggle("err", isErr);
}

function fmtBytes(bytes) {
  if (!bytes || bytes <= 0) return "";
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  if (bytes >= 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${bytes} B`;
}

async function getActiveTab() {
  return new Promise((resolve) => {
    chrome.tabs.query({ active: true, currentWindow: true }, (tabs) => {
      resolve((tabs && tabs[0]) || null);
    });
  });
}

async function refreshHealthAndTasks() {
  const health = await sendMsg({ type: "CHECK_HEALTH" });
  if (health.ok) {
    healthBadge.textContent = "Desktop Connected";
    healthBadge.className = "pill online";
  } else {
    healthBadge.textContent = "Desktop Offline";
    healthBadge.className = "pill offline";
  }

  const tasksRes = await sendMsg({ type: "GET_TASKS" });
  taskList.innerHTML = "";
  const tasks = tasksRes.data?.tasks || [];
  if (tasks.length === 0) {
    taskList.innerHTML = `<div class="row-sub">No active Vortex tasks yet.</div>`;
    return;
  }

  for (const t of tasks.slice(0, 5)) {
    const row = document.createElement("div");
    row.className = "row";
    row.innerHTML = `
      <div style="min-width:0">
        <div class="row-title">${t.name}</div>
        <div class="row-sub">${t.status} · ${(t.progress_pct || 0).toFixed(1)}%</div>
      </div>
    `;
    taskList.appendChild(row);
  }
}

function renderStreams(streams, pageTitle, pageUrl) {
  streamList.innerHTML = "";
  if (!streams || streams.length === 0) {
    streamList.innerHTML = `<div class="row-sub">No video streams intercepted on this tab yet.</div>`;
    return;
  }

  for (const s of streams) {
    const row = document.createElement("div");
    row.className = "row";

    const info = document.createElement("div");
    info.style.minWidth = "0";
    info.innerHTML = `
      <div class="row-title">${s.label}</div>
      <div class="row-sub">${fmtBytes(s.size) || s.mime || "Media"}</div>
    `;

    const btn = document.createElement("button");
    btn.className = "dl-btn";
    btn.textContent = "Download";
    btn.addEventListener("click", async () => {
      setStatus(`Sending ${s.label} to Vortex...`);
      const res = await sendMsg({
        type: "SEND_DOWNLOAD",
        url: s.url,
        title: pageTitle || "Vortex Video",
        quality: s.label,
        size: s.size || 0,
        isHls: Boolean(s.isHls),
        referer: pageUrl || null
      });
      if (res.ok && res.data?.ok) {
        setStatus(`Queued: ${res.data.filename}`);
        refreshHealthAndTasks();
      } else {
        setStatus(res.data?.error || res.error || "Failed to send", true);
      }
    });

    row.appendChild(info);
    row.appendChild(btn);
    streamList.appendChild(row);
  }
}

async function init() {
  await refreshHealthAndTasks();
  const tab = await getActiveTab();
  if (!tab) return;

  const res = await sendMsg({ type: "GET_TAB_STREAMS", tabId: tab.id });
  renderStreams(res.streams || [], tab.title, tab.url);

  scanPageBtn.addEventListener("click", async () => {
    if (!tab.url) return;
    setStatus("Analyzing active URL with Vortex...");
    const prev = await sendMsg({ type: "FETCH_PREVIEW", url: tab.url });
    if (prev.ok && Array.isArray(prev.data?.qualities)) {
      const mapped = prev.data.qualities.map((q) => ({
        label: q.label,
        url: q.url,
        size: q.estimated_size_bytes || 0,
        mime: q.mime_type,
        isHls: Boolean(q.is_hls)
      }));
      renderStreams(mapped, prev.data.title || tab.title, tab.url);
      setStatus(`Found ${mapped.length} stream format(s).`);
    } else {
      setStatus(prev.error || "Could not inspect page URL", true);
    }
  });
}

init();
