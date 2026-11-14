// all wasm work happens here so the page never freezes
import init, { send, receive, qr } from "./pkg/yoink_web.js";

const ready = init();
let sender = null;
let rx = null;

const ok = (id, result, transfer = []) => postMessage({ id, result }, transfer);

const asks = new Map();
let nextAsk = 1;
const ask = (path) =>
  new Promise((resolve) => {
    const id = nextAsk++;
    asks.set(id, resolve);
    postMessage({ event: "ask", id, path });
  });

async function exists(dir, name) {
  for (const open of ["getDirectoryHandle", "getFileHandle"]) {
    try {
      await dir[open](name);
      return true;
    } catch {}
  }
  return false;
}

async function writeTree(dir, files) {
  const seen = new Set();
  const skip = new Set();
  for (const f of files) {
    const parts = f.path.split("/");
    const top = parts[0];
    if (!seen.has(top)) {
      seen.add(top);
      if ((await exists(dir, top)) && !(await ask(top))) skip.add(top);
    }
    if (skip.has(top)) continue;
    let d = dir;
    for (const p of parts.slice(0, -1)) d = await d.getDirectoryHandle(p, { create: true });
    const w = await (await d.getFileHandle(parts.at(-1), { create: true })).createWritable();
    await w.write(f.data);
    await w.close();
  }
}

const ops = {
  async ping() {},

  async send({ files, bundle }) {
    const data = [];
    for (const f of files) data.push(new Uint8Array(await f.file.arrayBuffer()));
    sender = await send(files.map((f) => f.name), data, bundle);
    data.length = 0;
    sender.finished().then(() => postMessage({ event: "finished" }));
    return { result: sender.ticket() };
  },

  async close() {
    await sender?.close();
    sender = null;
  },

  async qr({ text }) {
    return { result: qr(text) };
  },

  async receive({ ticket }) {
    rx = await receive(ticket);
    return { result: { name: rx.name(), size: rx.size(), bundle: rx.is_bundle() } };
  },

  async download({ handle }) {
    const bundle = rx.is_bundle();
    const writable = handle && !bundle ? await handle.createWritable() : null;
    let last = 0;
    const progress = (done, total) => {
      const now = Date.now();
      if (now - last < 100 && done < total) return;
      last = now;
      postMessage({ event: "progress", done, total });
    };
    const files = await rx.download(progress, writable ? (c) => writable.write(c) : undefined);
    if (writable) await writable.close();
    if (handle && bundle) {
      await writeTree(handle, files);
      return {};
    }
    if (!files) return {};
    const out = [...files].map((f) => ({ path: f.path, data: f.data }));
    return { result: out, transfer: out.map((f) => f.data.buffer) };
  },
};

onmessage = async ({ data }) => {
  if (data.askReply !== undefined) {
    asks.get(data.askReply)?.(data.yes);
    asks.delete(data.askReply);
    return;
  }
  const { id, op, ...args } = data;
  try {
    await ready;
    const r = await ops[op](args);
    ok(id, r?.result, r?.transfer);
  } catch (err) {
    postMessage({ id, error: String(err?.message ?? err) });
  }
};
