const worker = new Worker(new URL("./worker.js", import.meta.url), { type: "module" });
const pending = new Map();
const on = {};
let nextId = 1;
worker.onmessage = ({ data }) => {
  if (data.event) return on[data.event]?.(data);
  const p = pending.get(data.id);
  pending.delete(data.id);
  data.error !== undefined ? p.reject(new Error(data.error)) : p.resolve(data.result);
};
const call = (op, args = {}) =>
  new Promise((resolve, reject) => {
    const id = nextId++;
    pending.set(id, { resolve, reject });
    worker.postMessage({ id, op, ...args });
  });

worker.onerror = (e) => {
  const err = new Error(e.message || "the transfer engine failed to load");
  for (const p of pending.values()) p.reject(err);
  pending.clear();
  const boot = document.querySelector("#boot");
  if (boot) {
    boot.hidden = false;
    boot.textContent = "Couldn't start the app: " + err.message;
  }
};

const $ = (s) => document.querySelector(s);
const LIMIT = 512 * 1024 * 1024;
const TICKET = /[a-z2-7]{52}/i;

const size = (n) =>
  n < 1024 ? `${n} B` : n < 1048576 ? `${(n / 1024).toFixed(1)} KB` : n < 1073741824 ? `${(n / 1048576).toFixed(1)} MB` : `${(n / 1073741824).toFixed(2)} GB`;

function trail(done, total, start) {
  const pct = Math.min(100, Math.floor((done * 100) / Math.max(total, 1)));
  const n = Math.floor(pct / 10);
  const secs = (Date.now() - start) / 1000;
  let left = "";
  if (done > 0 && done < total && secs >= 1) {
    const s = Math.round(((total - done) * secs) / done);
    left = s < 60 ? `  ${s}s left` : `  ${Math.floor(s / 60)}m ${s % 60}s left`;
  }
  return "o-".repeat(n) + ". ".repeat(10 - n) + ` ${String(pct).padStart(3)}%` + left;
}

function tab(which) {
  $("#tab-send").classList.toggle("on", which === "send");
  $("#tab-get").classList.toggle("on", which === "get");
  $("#send").hidden = which !== "send";
  $("#get").hidden = which !== "get";
}
$("#tab-send").onclick = () => tab("send");
$("#tab-get").onclick = () => tab("get");

// ---- send ----
let picked = [];
let sender = null;

function render() {
  $("#list").innerHTML = "";
  let total = 0;
  for (const p of picked) {
    total += p.file.size;
    const li = document.createElement("li");
    li.innerHTML = "<span></span><span></span>";
    li.firstChild.textContent = p.name;
    li.lastChild.textContent = size(p.file.size);
    $("#list").append(li);
  }
  $("#sum").textContent = "";
  const big = total > LIMIT;
  $("#serr").textContent = big ? "Too big for the browser (512 MB max). Use the yoink CLI for this one." : "";
  $("#go").disabled = !picked.length || big;
}

async function walk(entry, prefix, out) {
  if (entry.isFile) {
    const file = await new Promise((r) => entry.file(r));
    out.push({ name: prefix + entry.name, file });
  } else {
    const rd = entry.createReader();
    for (;;) {
      const batch = await new Promise((r) => rd.readEntries(r));
      if (!batch.length) break;
      for (const e of batch) await walk(e, prefix + entry.name + "/", out);
    }
  }
}

$("#drop").onclick = () => $("#fi").click();
$("#dirbtn").onclick = () => $("#di").click();
$("#fi").onchange = (e) => {
  picked.push(...[...e.target.files].map((file) => ({ name: file.name, file })));
  e.target.value = "";
  render();
};
$("#di").onchange = (e) => {
  picked.push(...[...e.target.files].map((file) => ({ name: file.webkitRelativePath || file.name, file })));
  e.target.value = "";
  render();
};
$("#drop").ondragover = (e) => {
  e.preventDefault();
  $("#drop").classList.add("over");
};
$("#drop").ondragleave = () => $("#drop").classList.remove("over");
$("#drop").ondrop = async (e) => {
  e.preventDefault();
  $("#drop").classList.remove("over");
  const entries = [...e.dataTransfer.items].map((i) => i.webkitGetAsEntry?.()).filter(Boolean);
  const out = [];
  for (const en of entries) await walk(en, "", out);
  picked.push(...out);
  render();
};

async function drawQr(text) {
  const m = await call("qr", { text });
  const w = Math.round(Math.sqrt(m.length));
  const s = 6;
  const q = 4;
  const c = $("#qr");
  c.width = c.height = (w + 2 * q) * s;
  const g = c.getContext("2d");
  g.fillStyle = "#fff";
  g.fillRect(0, 0, c.width, c.height);
  g.fillStyle = "#000";
  for (let i = 0; i < m.length; i++) if (m[i]) g.fillRect(((i % w) + q) * s, (Math.floor(i / w) + q) * s, s, s);
}

$("#go").onclick = async () => {
  $("#go").disabled = true;
  $("#serr").textContent = "";
  $("#sum").textContent = "Reading files…";
  try {
    const bundle = picked.length > 1 || picked.some((p) => p.name.includes("/"));
    $("#sum").textContent = "Creating ticket…";
    on.finished = () => {
      $("#sstatus").textContent = "Sent ✓";
      $("#stop").textContent = "Done";
    };
    const t = await call("send", { files: picked.map((p) => ({ name: p.name, file: p.file })), bundle });
    sender = true;
    const link = new URL("app", location.href).href + "#" + t;
    $("#ticket").textContent = t;
    $("#cli").textContent = "yoink get " + t;
    $("#cp-t").onclick = () => navigator.clipboard.writeText(t);
    $("#cp-l").onclick = () => navigator.clipboard.writeText(link);
    await drawQr(link);
    $("#pick").hidden = true;
    $("#shared").hidden = false;
  } catch (err) {
    $("#serr").textContent = String(err?.message ?? err);
    $("#go").disabled = false;
  }
};

$("#stop").onclick = async () => {
  try {
    if (sender) await call("close");
  } catch {}
  sender = null;
  picked = [];
  render();
  $("#shared").hidden = true;
  $("#pick").hidden = false;
  $("#sstatus").textContent = "Waiting for the receiver. Keep this tab open.";
  $("#stop").textContent = "Stop sharing";
};

// ---- receive ----
let info = null;

async function connect() {
  const m = $("#tin").value.match(TICKET);
  if (!m) return ($("#rerr").textContent = "That doesn't look like a ticket.");
  $("#rerr").textContent = "";
  $("#conn").disabled = true;
  $("#conn").textContent = "Connecting…";
  try {
    info = await call("receive", { ticket: m[0] });
    $("#fname").textContent = info.bundle ? "Bundle of files" : info.name;
    $("#fsize").textContent = size(info.size) + (info.bundle ? " (compressed)" : "");
    $("#ask").hidden = true;
    $("#found").hidden = false;
  } catch (err) {
    $("#rerr").textContent = String(err?.message ?? err);
    $("#conn").disabled = false;
    $("#conn").textContent = "Connect";
  }
}
async function scan(blob) {
  $("#rerr").textContent = "";
  try {
    const bmp = await createImageBitmap(blob);
    const k = Math.min(1, 1600 / Math.max(bmp.width, bmp.height));
    const c = document.createElement("canvas");
    c.width = Math.round(bmp.width * k);
    c.height = Math.round(bmp.height * k);
    const g = c.getContext("2d", { willReadFrequently: true });
    g.drawImage(bmp, 0, 0, c.width, c.height);
    const d = g.getImageData(0, 0, c.width, c.height);
    const m = jsQR(d.data, d.width, d.height)?.data.match(TICKET);
    if (!m) return ($("#rerr").textContent = "No yoink QR code found in that image.");
    $("#tin").value = m[0];
    connect();
  } catch {
    $("#rerr").textContent = "Couldn't read that image.";
  }
}
const img = (files) => [...files].find((f) => f.type.startsWith("image/"));
$("#qrdrop").onclick = () => $("#qi").click();
$("#qi").onchange = (e) => {
  const f = img(e.target.files);
  e.target.value = "";
  if (f) scan(f);
};
$("#qrdrop").ondragover = (e) => {
  e.preventDefault();
  $("#qrdrop").classList.add("over");
};
$("#qrdrop").ondragleave = () => $("#qrdrop").classList.remove("over");
$("#qrdrop").ondrop = (e) => {
  e.preventDefault();
  $("#qrdrop").classList.remove("over");
  const f = img(e.dataTransfer.files);
  if (f) scan(f);
};
document.addEventListener("paste", (e) => {
  const f = $("#get").hidden ? null : img(e.clipboardData.files);
  if (f) {
    e.preventDefault();
    scan(f);
  }
});
$("#conn").onclick = connect;
$("#tin").onkeydown = (e) => e.key === "Enter" && connect();

on.ask = ({ id, path }) =>
  worker.postMessage({ askReply: id, yes: confirm(`${path} already exists in that folder. Replace it?`) });

$("#dl").onclick = async () => {
  $("#rerr").textContent = "";
  let handle = null;
  try {
    if (!info.bundle && window.showSaveFilePicker) handle = await showSaveFilePicker({ suggestedName: info.name });
    else if (info.bundle && window.showDirectoryPicker) handle = await showDirectoryPicker({ mode: "readwrite" });
  } catch (err) {
    if (err?.name === "AbortError") return;
  }
  $("#dl").hidden = true;
  const start = Date.now();
  on.progress = ({ done, total }) => {
    $("#trail").textContent = trail(done, total, start) + (info.bundle && done >= total ? "\nUnpacking…" : "");
  };
  try {
    const files = await call("download", { handle });
    on.progress = null;
    $("#trail").textContent = trail(1, 1, start);
    if (files) showFiles(files);
    else $("#trail").textContent += "\nSaved ✓";
  } catch (err) {
    $("#rerr").textContent = String(err?.message ?? err);
    $("#dl").hidden = false;
  }
};

function showFiles(files) {
  const links = [];
  for (const f of files) {
    const a = document.createElement("a");
    a.href = URL.createObjectURL(new Blob([f.data]));
    a.download = f.path.split("/").at(-1);
    a.textContent = f.path;
    const li = document.createElement("li");
    li.innerHTML = "<span></span><span></span>";
    li.firstChild.append(a);
    li.lastChild.textContent = size(f.data.length);
    $("#out").append(li);
    links.push(a);
  }
  if (links.length === 1) links[0].click();
  else {
    $("#all").hidden = false;
    $("#all").onclick = () => links.forEach((a, i) => setTimeout(() => a.click(), i * 250));
  }
}

// ---- boot ----
try {
  let timer;
  await Promise.race([
    call("ping"),
    new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error("the transfer engine didn't load")), 15000);
    }),
  ]).finally(() => clearTimeout(timer));
  $("#boot").hidden = true;
  const h = location.hash.slice(1);
  if (TICKET.test(h)) {
    tab("get");
    $("#tin").value = h;
    connect();
  } else tab("send");
} catch (err) {
  $("#boot").textContent = "Couldn't start the app: " + (err?.message ?? err);
}
