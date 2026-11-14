use std::{fmt::Display, io::Read, sync::Arc};

use bao_tree::io::BaoContentItem;
use bytes::Bytes;
use iroh::{
    endpoint::{presets, Connection},
    protocol::Router,
    Endpoint,
};
use iroh_blobs::{
    format::collection::Collection,
    get::request::{get_blob, get_hash_seq_and_sizes, GetBlobItem},
    store::mem::MemStore,
    BlobsProtocol, Hash,
};
use js_sys::{Array, Function, Object, Promise, Reflect, Uint8Array};
use n0_future::StreamExt;
use tokio::sync::Notify;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use yoink_core::{fetch_root, leaf, Root, Ticket, BUNDLE, ROOT_ALPN};

const FLUSH: usize = 4 << 20;
/// whole-file-in-memory paths refuse anything bigger
const MEM_LIMIT: u64 = 512 << 20;

fn e(x: impl Display) -> JsError {
    JsError::new(&x.to_string())
}

fn bundle(items: &[(String, Vec<u8>)]) -> Result<Vec<u8>, JsError> {
    let enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut b = tar::Builder::new(enc);
    for (name, data) in items {
        let mut h = tar::Header::new_gnu();
        h.set_size(data.len() as u64);
        h.set_mode(0o644);
        b.append_data(&mut h, name, &data[..]).map_err(e)?;
    }
    b.into_inner().map_err(e)?.finish().map_err(e)
}

#[wasm_bindgen]
pub struct Sender {
    ticket: String,
    router: Router,
    done: Arc<Notify>,
}

#[wasm_bindgen]
impl Sender {
    pub fn ticket(&self) -> String {
        self.ticket.clone()
    }

    /// resolves once a receiver finished downloading
    pub async fn finished(&self) {
        self.done.notified().await
    }

    pub async fn close(&self) -> Result<(), JsError> {
        self.router.shutdown().await.map_err(e)
    }
}

/// `names[i]` is the path of `files[i]`; with `as_bundle` everything goes as one archive
#[wasm_bindgen]
pub async fn send(names: Array, files: Array, as_bundle: bool) -> Result<Sender, JsError> {
    let mut items = Vec::new();
    let mut size = 0u64;
    for i in 0..files.length() {
        let name = names.get(i).as_string().ok_or_else(|| e("bad name"))?;
        let data = Uint8Array::new(&files.get(i)).to_vec();
        size += data.len() as u64;
        if size > MEM_LIMIT {
            return Err(e("too big for the browser, use the yoink CLI"));
        }
        items.push((name, data));
    }
    if items.is_empty() {
        return Err(e("nothing to send"));
    }
    let (name, data) = if as_bundle || items.len() > 1 {
        (BUNDLE.to_string(), bundle(&items)?)
    } else {
        items.remove(0)
    };
    let total = data.len() as u64;
    let store = MemStore::new();
    let tag = store.add_bytes(Bytes::from(data)).await.map_err(e)?;
    let tt = [(name, tag.hash)]
        .into_iter()
        .collect::<Collection>()
        .store(&store)
        .await
        .map_err(e)?;
    store.tags().create(tt.hash_and_format()).await.map_err(e)?;

    let endpoint = Endpoint::bind(presets::N0).await.map_err(e)?;
    endpoint.online().await;
    let ticket = Ticket(endpoint.id()).to_string();
    let done = Arc::new(Notify::new());
    let router = Router::builder(endpoint)
        .accept(iroh_blobs::ALPN, BlobsProtocol::new(&store, None))
        .accept(ROOT_ALPN, Root(tt.hash(), total, done.clone()))
        .spawn();
    Ok(Sender {
        ticket,
        router,
        done,
    })
}

#[wasm_bindgen]
pub struct Receiver {
    endpoint: Endpoint,
    root_conn: Connection,
    blobs_conn: Connection,
    item: Hash,
    name: String,
    size: u64,
}

#[wasm_bindgen]
impl Receiver {
    pub fn name(&self) -> String {
        self.name.clone()
    }

    pub fn size(&self) -> f64 {
        self.size as f64
    }

    pub fn is_bundle(&self) -> bool {
        self.name.ends_with('/')
    }

    /// `progress(done, total)`; with a `write(Uint8Array)` callback a plain file is streamed
    /// out instead of held in memory. Returns `[{path, data}]`, or nothing when streamed.
    pub async fn download(&self, progress: Function, write: JsValue) -> Result<JsValue, JsError> {
        let sink = write
            .dyn_into::<Function>()
            .ok()
            .filter(|_| !self.is_bundle());
        if sink.is_none() && self.size > MEM_LIMIT {
            return Err(e(
                "too big for this browser, use Chrome/Edge or the yoink CLI",
            ));
        }
        let mut s = get_blob(self.blobs_conn.clone(), self.item);
        let mut buf: Vec<u8> = Vec::new();
        let mut got = 0u64;
        while let Some(it) = s.next().await {
            match it {
                GetBlobItem::Item(BaoContentItem::Leaf(l)) => {
                    got += l.data.len() as u64;
                    buf.extend_from_slice(&l.data);
                    let _ = progress.call2(
                        &JsValue::NULL,
                        &(got as f64).into(),
                        &(self.size as f64).into(),
                    );
                    if let Some(f) = &sink {
                        if buf.len() >= FLUSH {
                            flush(f, &mut buf).await?;
                        }
                    }
                }
                GetBlobItem::Error(err) => return Err(e(err)),
                _ => {}
            }
        }
        self.root_conn.close(0u32.into(), b"done");
        self.endpoint.close().await;
        if let Some(f) = &sink {
            flush(f, &mut buf).await?;
            return Ok(JsValue::UNDEFINED);
        }
        let out = Array::new();
        if self.is_bundle() {
            let mut a = tar::Archive::new(flate2::read::GzDecoder::new(&buf[..]));
            let mut unpacked = 0u64;
            for en in a.entries().map_err(e)? {
                let mut en = en.map_err(e)?;
                if !en.header().entry_type().is_file() {
                    continue;
                }
                let path = en
                    .path()
                    .map_err(e)?
                    .components()
                    .filter_map(|c| match c {
                        std::path::Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("/");
                if path.is_empty() {
                    continue;
                }
                let mut data = Vec::new();
                let room = MEM_LIMIT - unpacked;
                en.by_ref()
                    .take(room + 1)
                    .read_to_end(&mut data)
                    .map_err(e)?;
                unpacked += data.len() as u64;
                if unpacked > MEM_LIMIT {
                    return Err(e("bundle is too big to unpack here, use the yoink CLI"));
                }
                out.push(&entry(&path, &data));
            }
        } else {
            out.push(&entry(&self.name, &buf));
        }
        Ok(out.into())
    }
}

async fn flush(f: &Function, buf: &mut Vec<u8>) -> Result<(), JsError> {
    if buf.is_empty() {
        return Ok(());
    }
    let r = f
        .call1(&JsValue::NULL, &Uint8Array::from(&buf[..]))
        .map_err(|_| e("write failed"))?;
    JsFuture::from(Promise::resolve(&r))
        .await
        .map_err(|_| e("write failed"))?;
    buf.clear();
    Ok(())
}

fn entry(path: &str, data: &[u8]) -> Object {
    let o = Object::new();
    let _ = Reflect::set(&o, &"path".into(), &path.into());
    let _ = Reflect::set(&o, &"data".into(), &Uint8Array::from(data));
    o
}

#[wasm_bindgen]
pub async fn receive(ticket: String) -> Result<Receiver, JsError> {
    let ticket: Ticket = ticket
        .parse()
        .map_err(|_| e("that doesn't look like a ticket"))?;
    let endpoint = Endpoint::bind(presets::N0).await.map_err(e)?;
    let (root_conn, root, _) = fetch_root(&endpoint, &ticket).await.map_err(e)?;

    let blobs_conn = endpoint
        .connect(ticket.0, iroh_blobs::ALPN)
        .await
        .map_err(e)?;
    let (seq, sizes) = get_hash_seq_and_sizes(&blobs_conn, &root, 1 << 20, None)
        .await
        .map_err(e)?;
    let meta = get_blob(
        blobs_conn.clone(),
        seq.get(0).ok_or_else(|| e("empty share"))?,
    )
    .await
    .map_err(e)?;
    let (_, names): ([u8; 13], Vec<String>) = postcard::from_bytes(&meta).map_err(e)?;
    let item = seq.get(1).ok_or_else(|| e("empty share"))?;
    let raw = names.first().ok_or_else(|| e("empty share"))?;
    let name = if raw.ends_with('/') {
        raw.clone()
    } else {
        leaf(raw).map_err(e)?.to_string()
    };
    Ok(Receiver {
        endpoint,
        root_conn,
        blobs_conn,
        item,
        name,
        size: sizes[1],
    })
}

/// QR modules row by row, 1 = dark; the side length is the square root of the length
#[wasm_bindgen]
pub fn qr(text: &str) -> Result<Uint8Array, JsError> {
    let c = qrcode::QrCode::new(text).map_err(e)?;
    let v: Vec<u8> = c
        .to_colors()
        .iter()
        .map(|c| (*c == qrcode::Color::Dark) as u8)
        .collect();
    Ok(Uint8Array::from(&v[..]))
}
