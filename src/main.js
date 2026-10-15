// Lwnny front end. Talks only to the Rust side through Tauri commands.
const invoke = window.__TAURI__.core.invoke;

const $ = (id) => document.getElementById(id);
const statusEl = $("status");

let snap = null;          // last snapshot from the backend
let pending = null;       // debounce timer for color pickers

const SWATCHES = ["#ffffff", "#ff2a2a", "#ff8a00", "#ffe600", "#39ff6a", "#00e5ff", "#2f6bff", "#9b4dff", "#ff3cc7", "#ff6a9a"];

function setStatus(msg, bad = false) {
  statusEl.textContent = msg;
  statusEl.className = "status" + (bad ? " bad" : "");
}

function reportResults(results, verb = "Applied") {
  if (!Array.isArray(results)) return;
  const failed = results.filter((r) => !r.ok);
  if (failed.length === 0) {
    setStatus(`${verb} to ${results.length} device${results.length === 1 ? "" : "s"} at ${new Date().toLocaleTimeString()}`);
  } else {
    setStatus(`${failed.map((f) => `${f.id}: ${f.error}`).join(" | ")}`, true);
  }
  results.forEach((r) => {
    const dot = document.querySelector(`[data-dot="${r.id}"]`);
    if (dot) dot.className = "dot" + (r.ok ? " ok" : "");
  });
}

async function call(cmd, args, verb) {
  try {
    const res = await invoke(cmd, args || {});
    if (verb) reportResults(res, verb);
    return res;
  } catch (e) {
    setStatus(`${cmd}: ${e}`, true);
    throw e;
  }
}

// Debounce color pickers: fire at most every 120 ms while dragging.
function debounced(fn) {
  if (pending) clearTimeout(pending);
  pending = setTimeout(() => {
    pending = null;
    fn();
  }, 120);
}

function esc(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
}

function renderDevices() {
  const root = $("devices");
  root.innerHTML = "";
  const statusById = Object.fromEntries(snap.status.map((s) => [s.id, s]));

  if (snap.devices.length === 0) {
    root.innerHTML = `<div class="card"><h3>No devices found</h3><p class="note">${snap.status
      .map((s) => `${s.id}: ${s.error || "ok"}`)
      .join("<br>")}</p></div>`;
    return;
  }

  for (const dev of snap.devices) {
    const st = statusById[dev.id] || { ok: true };
    const colors = snap.scene.colors[dev.id] || [];
    const card = document.createElement("div");
    card.className = "card" + (st.ok ? "" : " off");
    card.innerHTML = `
      <div class="card-head">
        <div class="title">
          <span class="dot ${st.ok ? "ok" : ""}" data-dot="${dev.id}"></span>
          <h3>${esc(dev.name)}</h3>
          <span class="kind">${esc(dev.kind)}</span>
        </div>
        <div class="row">
          <label class="check" title="Keep this device's own colors when 'Everything' changes">
            <input type="checkbox" data-lock="${dev.id}" ${(snap.scene.locked || []).includes(dev.id) ? "checked" : ""} />
            <span class="zleds">Own color</span>
          </label>
          <label class="row" title="Set every zone of this device">
            <span class="zleds">Whole device</span>
            <input type="color" class="small" data-devcolor="${dev.id}" value="${colors[0] ? toHex(colors[0]) : "#ffffff"}" />
          </label>
        </div>
      </div>
      <p class="note">${esc(dev.note)}${dev.supports_save ? " · can save to device" : ""}${st.error ? ` · <b>${esc(st.error)}</b>` : ""}</p>
      <div class="zones"></div>
      <details class="fix" data-fixdev="${dev.id}">
        <summary>Color correction${fixActive(dev.id) ? " · active" : ""}</summary>
        <div class="fix-body">
          <label class="field"><span>Channel order the device receives (R,G,B slots)</span>
            <select data-fixorder="${dev.id}">
              ${["rgb", "rbg", "grb", "gbr", "brg", "bgr"].map((o) => `<option value="${o}">${o.toUpperCase()}</option>`).join("")}
            </select></label>
          <label class="field"><span>Red gain <b data-fixval="${dev.id}-0"></b></span><input type="range" min="0" max="200" data-fixgain="${dev.id}" data-ch="0" /></label>
          <label class="field"><span>Green gain <b data-fixval="${dev.id}-1"></b></span><input type="range" min="0" max="200" data-fixgain="${dev.id}" data-ch="1" /></label>
          <label class="field"><span>Blue gain <b data-fixval="${dev.id}-2"></b></span><input type="range" min="0" max="200" data-fixgain="${dev.id}" data-ch="2" /></label>
          <div class="row">
            <button class="tiny" data-calibrate="${dev.id}">Calibrate…</button>
            <button class="tiny ghost" data-fixreset="${dev.id}">Reset</button>
            <span class="zleds">Calibrate walks you through it with numbered test colors.</span>
          </div>
        </div>
      </details>`;

    const zones = card.querySelector(".zones");
    dev.zones.forEach((z, i) => {
      const c = colors[i] || { r: 255, g: 255, b: 255 };
      const row = document.createElement("div");
      row.className = "zone" + (z.leds === 0 ? " empty" : "");
      row.innerHTML = `
        <input type="color" class="small" data-dev="${dev.id}" data-zone="${i}" value="${toHex(c)}" ${z.leds === 0 ? "disabled" : ""} />
        <input type="text" class="zname" data-rename="${dev.id}" data-zone="${i}" value="${esc(z.name)}" title="Click to rename this zone" spellcheck="false" />
        <span class="zleds">${z.leds} LED${z.leds === 1 ? "" : "s"}</span>
        <button class="tiny ghost" data-find="${dev.id}" data-zone="${i}" title="Flash this zone white for 3 seconds so you can see where it is" ${z.leds === 0 ? "disabled" : ""}>Find</button>`;
      zones.appendChild(row);
    });
    root.appendChild(card);
  }

  wireDeviceControls(root);
}

function wireDeviceControls(root) {
  // color correction controls
  root.querySelectorAll("details[data-fixdev]").forEach((det) => {
    const id = det.dataset.fixdev;
    const fix = currentFix(id);
    det.querySelector("select").value = fix.order;
    det.querySelectorAll("input[data-fixgain]").forEach((r) => {
      r.value = Math.round(fix.gain[Number(r.dataset.ch)] * 100);
      det.querySelector(`[data-fixval="${id}-${r.dataset.ch}"]`).textContent = `${r.value}%`;
    });
    const send = () => {
      const f = {
        order: det.querySelector("select").value,
        gain: [0, 1, 2].map((ch) => Number(det.querySelector(`input[data-ch="${ch}"]`).value) / 100),
      };
      snap.settings.color_fix[id] = f;
      det.querySelector("summary").textContent = "Color correction" + (fixActive(id) ? " · active" : "");
      debounced(() => call("set_color_fix", { device: id, fix: f }, "Applied"));
    };
    det.querySelector("select").addEventListener("change", send);
    det.querySelectorAll("input[data-fixgain]").forEach((r) =>
      r.addEventListener("input", () => {
        det.querySelector(`[data-fixval="${id}-${r.dataset.ch}"]`).textContent = `${r.value}%`;
        send();
      })
    );
    det.querySelector("[data-calibrate]").addEventListener("click", () => openCalibration(id));
    det.querySelector("[data-fixreset]").addEventListener("click", () => {
      det.querySelector("select").value = "rgb";
      det.querySelectorAll("input[data-fixgain]").forEach((r) => {
        r.value = 100;
        det.querySelector(`[data-fixval="${id}-${r.dataset.ch}"]`).textContent = "100%";
      });
      send();
    });
  });

  root.querySelectorAll("input[data-lock]").forEach((cb) => {
    cb.addEventListener("change", async () => {
      const id = cb.dataset.lock;
      snap.scene.locked = (snap.scene.locked || []).filter((d) => d !== id);
      if (cb.checked) snap.scene.locked.push(id);
      await call("set_locked", { device: id, locked: cb.checked });
      setStatus(cb.checked ? `${id} keeps its own color now` : `${id} follows "Everything" again`);
    });
  });

  root.querySelectorAll("input[data-zone][type=color]").forEach((inp) => {
    inp.addEventListener("input", () => {
      const device = inp.dataset.dev;
      const zone = Number(inp.dataset.zone);
      const color = inp.value;
      snap.scene.colors[device][zone] = fromHex(color);
      debounced(() => call("set_zone_color", { device, zone, color }, "Applied"));
    });
  });

  root.querySelectorAll("input[data-rename]").forEach((inp) => {
    const commit = async () => {
      const device = inp.dataset.rename;
      const zone = Number(inp.dataset.zone);
      snap.devices = await call("set_zone_name", { device, zone, name: inp.value });
      const z = snap.devices.find((d) => d.id === device)?.zones[zone];
      if (z) inp.value = z.name;
      setStatus(`Zone renamed to "${inp.value}"`);
    };
    inp.addEventListener("change", commit);
    inp.addEventListener("keydown", (e) => {
      if (e.key === "Enter") inp.blur();
    });
  });

  root.querySelectorAll("button[data-find]").forEach((b) => {
    b.addEventListener("click", async () => {
      const device = b.dataset.find;
      const zone = Number(b.dataset.zone);
      b.disabled = true;
      setStatus("Flashing zone white for 3 seconds…");
      try {
        await call("identify_zone", { device, zone });
        await new Promise((r) => setTimeout(r, 3000));
      } finally {
        await call("apply", {}, "Restored");
        b.disabled = false;
      }
    });
  });

  root.querySelectorAll("input[data-devcolor]").forEach((inp) => {
    inp.addEventListener("input", () => {
      const device = inp.dataset.devcolor;
      const color = inp.value;
      (snap.scene.colors[device] || []).forEach((_, i) => (snap.scene.colors[device][i] = fromHex(color)));
      root.querySelectorAll(`input[data-dev="${device}"]`).forEach((z) => (z.value = color));
      debounced(() => call("set_device_color", { device, color }, "Applied"));
    });
  });
}

function renderControls() {
  $("brightness").value = snap.scene.brightness;
  $("bright-val").textContent = `${snap.scene.brightness}%`;

  for (let i = 0; i < 4; i++) $(`fans-${i}`).value = snap.settings.hub_fans_per_port[i];
  for (let i = 0; i < 2; i++) $(`argb-${i}`).value = snap.settings.aura_header_leds[i];
  $("autostart").checked = !!snap.autostart;

  const list = $("profiles");
  list.innerHTML = "";
  if (snap.profiles.length === 0) {
    list.innerHTML = `<li><span class="zleds">No profiles yet</span></li>`;
  }
  for (const name of snap.profiles) {
    const li = document.createElement("li");
    li.innerHTML = `<span>${esc(name)}</span>
      <button class="tiny" data-load="${esc(name)}">Load</button>
      <button class="tiny ghost" data-del="${esc(name)}">Delete</button>`;
    list.appendChild(li);
  }
  list.querySelectorAll("[data-load]").forEach((b) =>
    b.addEventListener("click", async () => {
      await call("load_profile", { name: b.dataset.load }, "Loaded profile, applied");
      await refresh();
    })
  );
  list.querySelectorAll("[data-del]").forEach((b) =>
    b.addEventListener("click", async () => {
      snap.profiles = await call("delete_profile", { name: b.dataset.del });
      renderControls();
      setStatus(`Deleted profile "${b.dataset.del}"`);
    })
  );
}

function renderSwatches() {
  const box = $("swatches");
  box.innerHTML = "";
  for (const hex of SWATCHES) {
    const s = document.createElement("div");
    s.className = "swatch";
    s.style.background = hex;
    s.title = hex;
    s.addEventListener("click", () => applyAll(hex));
    box.appendChild(s);
  }
}

async function applyAll(hex) {
  $("all-color").value = hex;
  const locked = snap.scene.locked || [];
  for (const id of Object.keys(snap.scene.colors)) {
    if (locked.includes(id)) continue;
    snap.scene.colors[id] = snap.scene.colors[id].map(() => fromHex(hex));
  }
  document.querySelectorAll("input[data-zone][type=color], input[data-devcolor]").forEach((i) => {
    const id = i.dataset.dev || i.dataset.devcolor;
    if (!i.disabled && !locked.includes(id)) i.value = hex;
  });
  await call("set_all_color", { color: hex }, "Applied");
}

async function refresh() {
  snap = await call("snapshot");
  renderDevices();
  renderControls();
}

// ----- calibration wizard ---------------------------------------------------

const cal = {
  id: null,
  original: null,
  fix: null,
  seen: { r: null, g: null, b: null },
};

// White-balance candidates: red and green gain with blue at 100%.
const WHITE_STEPS = [
  [1.0, 1.0], [0.75, 0.65], [0.55, 0.45], [0.4, 0.3], [0.3, 0.22],
  [0.22, 0.15], [0.15, 0.1], [0.1, 0.07], [0.07, 0.045], [0.045, 0.03],
];

async function calApply(colorHex) {
  snap.settings.color_fix[cal.id] = { order: cal.fix.order, gain: [...cal.fix.gain] };
  await call("set_color_fix", { device: cal.id, fix: cal.fix });
  if (colorHex) {
    (snap.scene.colors[cal.id] || []).forEach((_, i) => (snap.scene.colors[cal.id][i] = fromHex(colorHex)));
    await call("set_device_color", { device: cal.id, color: colorHex }, "Test color applied");
  }
  $("cal-gains").textContent = `Order ${cal.fix.order.toUpperCase()} · red ${Math.round(cal.fix.gain[0] * 100)}% · green ${Math.round(
    cal.fix.gain[1] * 100
  )}% · blue ${Math.round(cal.fix.gain[2] * 100)}%`;
}

function calOrderFromSeen() {
  const s = cal.seen;
  if (!s.r || !s.g || !s.b) return null;
  const set = new Set([s.r, s.g, s.b]);
  if (set.size !== 3) return null;
  const bySeen = { [s.r]: "r", [s.g]: "g", [s.b]: "b" };
  return bySeen.r + bySeen.g + bySeen.b;
}

function openCalibration(id) {
  cal.id = id;
  cal.original = currentFix(id);
  cal.fix = { order: "rgb", gain: [1, 1, 1] };
  cal.seen = { r: null, g: null, b: null };
  const dev = snap.devices.find((d) => d.id === id);
  $("cal-title").textContent = `Calibrate · ${dev ? dev.name : id}`;

  const ch = $("cal-channels");
  ch.innerHTML = "";
  for (const [key, label, hex] of [["r", "Pure red", "#ff0000"], ["g", "Pure green", "#00ff00"], ["b", "Pure blue", "#0000ff"]]) {
    const box = document.createElement("div");
    box.className = "test";
    box.innerHTML = `<button class="tiny" data-show="${hex}">${label}</button>
      <span class="zleds">I see:</span>
      <span class="seen">
        <button data-seen="${key}:r" style="background:#c62828">Red</button>
        <button data-seen="${key}:g" style="background:#2e7d32">Green</button>
        <button data-seen="${key}:b" style="background:#1565c0">Blue</button>
      </span>`;
    ch.appendChild(box);
  }
  ch.querySelectorAll("[data-show]").forEach((b) =>
    b.addEventListener("click", () => {
      cal.fix = { order: "rgb", gain: [1, 1, 1] };
      calApply(b.dataset.show);
    })
  );
  ch.querySelectorAll("[data-seen]").forEach((b) =>
    b.addEventListener("click", () => {
      const [key, seen] = b.dataset.seen.split(":");
      cal.seen[key] = seen;
      b.parentElement.querySelectorAll("button").forEach((x) => x.classList.remove("picked"));
      b.classList.add("picked");
      const order = calOrderFromSeen();
      const answered = Object.values(cal.seen).filter(Boolean).length;
      $("cal-order").textContent =
        answered < 3
          ? `Order so far: ${answered}/3 answered`
          : order
          ? `Channel order: ${order.toUpperCase()}${order === "rgb" ? " (normal, no swap needed)" : " (swapped, will be corrected)"}`
          : "Two channels looked the same: no swap, this is a brightness issue (go to step 2)";
      if (order) cal.fix.order = order;
    })
  );

  const w = $("cal-whites");
  w.innerHTML = "";
  WHITE_STEPS.forEach(([r, g], i) => {
    const b = document.createElement("button");
    b.className = "num";
    b.textContent = String(i + 1);
    b.title = `red ${Math.round(r * 100)}% · green ${Math.round(g * 100)}% · blue 100%`;
    b.addEventListener("click", () => {
      w.querySelectorAll(".num").forEach((x) => x.classList.remove("picked"));
      b.classList.add("picked");
      cal.fix.gain = [r, g, 1.0];
      calApply("#ffffff");
    });
    w.appendChild(b);
  });

  $("cal-gains").textContent = "";
  $("cal").showModal();
}

function calTune(kind) {
  const g = cal.fix.gain;
  const step = (v, f) => Math.min(2, Math.max(0.01, v * f));
  switch (kind) {
    case "g-": g[1] = step(g[1], 0.8); break;
    case "g+": g[1] = step(g[1], 1.25); break;
    case "r-": g[0] = step(g[0], 0.8); break;
    case "r+": g[0] = step(g[0], 1.25); break;
    case "b-": g[2] = step(g[2], 0.8); break;
    case "dim": g[0] = step(g[0], 1.3); g[1] = step(g[1], 1.3); g[2] = step(g[2], 1.3); break;
  }
  calApply("#ffffff");
}

document.querySelectorAll("#cal-tune [data-tune]").forEach((b) => b.addEventListener("click", () => calTune(b.dataset.tune)));

$("cal-done").addEventListener("click", async () => {
  await calApply(null);
  $("cal").close();
  renderDevices();
  setStatus("Calibration saved");
});

$("cal-reset").addEventListener("click", async () => {
  cal.fix = { order: "rgb", gain: [1, 1, 1] };
  await calApply("#ffffff");
});

$("cal-close").addEventListener("click", async () => {
  cal.fix = cal.original;
  await calApply(null);
  $("cal").close();
  renderDevices();
  setStatus("Calibration cancelled, previous correction restored");
});

function currentFix(id) {
  const f = (snap.settings.color_fix || {})[id];
  return f ? { order: f.order, gain: [...f.gain] } : { order: "rgb", gain: [1, 1, 1] };
}
function fixActive(id) {
  const f = currentFix(id);
  return f.order !== "rgb" || f.gain.some((g) => Math.abs(g - 1) > 0.001);
}

function toHex(c) {
  return "#" + [c.r, c.g, c.b].map((v) => v.toString(16).padStart(2, "0")).join("");
}
function fromHex(h) {
  const v = parseInt(h.replace("#", ""), 16);
  return { r: (v >> 16) & 255, g: (v >> 8) & 255, b: v & 255 };
}

// ----- wire up global controls -------------------------------------------

$("all-color").addEventListener("input", (e) => debounced(() => applyAll(e.target.value)));

$("brightness").addEventListener("input", () => {
  $("bright-val").textContent = `${$("brightness").value}%`;
  debounced(() => call("set_brightness", { brightness: Number($("brightness").value) }, "Applied"));
});

$("btn-save-hw").addEventListener("click", async () => {
  setStatus("Saving to devices…");
  await call("save_hardware", {}, "Saved");
});

$("btn-rescan").addEventListener("click", async () => {
  setStatus("Rescanning…");
  await call("rescan");
  await call("apply", {}, "Applied");
  await refresh();
});

$("btn-save-profile").addEventListener("click", async () => {
  const name = $("profile-name").value.trim();
  if (!name) return setStatus("Give the profile a name first", true);
  snap.profiles = await call("save_profile", { name });
  $("profile-name").value = "";
  renderControls();
  setStatus(`Saved profile "${name}"`);
});

$("btn-save-settings").addEventListener("click", async () => {
  const settings = {
    hub_fans_per_port: [0, 1, 2, 3].map((i) => Number($(`fans-${i}`).value) || 0),
    aura_header_leds: [0, 1].map((i) => Number($(`argb-${i}`).value) || 0),
    start_minimized: true,
    zone_names: snap.settings.zone_names || {},
    color_fix: snap.settings.color_fix || {},
  };
  setStatus("Applying settings…");
  await call("set_settings", { settings }, "Applied");
  await refresh();
});

$("autostart").addEventListener("change", async (e) => {
  const on = await call("set_autostart", { enabled: e.target.checked });
  setStatus(on ? "Lwnny will start with Windows" : "Autostart disabled");
});

renderSwatches();
refresh().catch(() => {});
