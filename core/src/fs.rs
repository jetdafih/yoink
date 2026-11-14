use std::{
    io::IsTerminal,
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::Instant,
};

use anyhow::{anyhow, Result};
use iroh::{endpoint::presets, protocol::Router, Endpoint};

use crate::{fetch_root, leaf, Root, Ticket, BUNDLE, ROOT_ALPN};
use iroh_blobs::api::downloader::DownloadProgressItem;
use iroh_blobs::{
    format::collection::Collection, store::fs::FsStore, BlobsProtocol, HashAndFormat,
};
use n0_future::StreamExt;
use tokio::sync::Notify;

pub struct Share {
    pub ticket: Ticket,
    router: Router,
    dir: PathBuf,
    done: Arc<Notify>,
}

impl Share {
    pub async fn finished(&self) {
        self.done.notified().await
    }

    pub async fn close(self) -> Result<()> {
        let r = self.router.shutdown().await;
        let _ = std::fs::remove_dir_all(&self.dir);
        Ok(r?)
    }
}

fn scratch() -> PathBuf {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("yoink-{}-{n}", std::process::id()))
}

/// names are sent relative to the output folder, so they have to be unique
fn file_names(paths: &[PathBuf]) -> Result<Vec<(String, PathBuf)>> {
    let mut named = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for p in paths {
        let p = std::path::absolute(p)?;
        let name = p
            .file_name()
            .ok_or_else(|| anyhow!("no file name: {}", p.display()))?
            .to_string_lossy()
            .into_owned();
        if !seen.insert(name.clone()) {
            return Err(anyhow!(
                "two paths are named {name:?}; rename one or send them separately"
            ));
        }
        named.push((name, p));
    }
    Ok(named)
}

pub async fn share(paths: &[PathBuf]) -> Result<Share> {
    let dir = scratch();
    match share_in(&dir, paths).await {
        Ok(share) => Ok(share),
        Err(e) => {
            let _ = std::fs::remove_dir_all(&dir);
            Err(e)
        }
    }
}

async fn share_in(dir: &Path, paths: &[PathBuf]) -> Result<Share> {
    let store = FsStore::load(dir).await?;
    let mut named = file_names(paths)?;
    if named.is_empty() {
        return Err(anyhow!("nothing to send"));
    }
    // one plain file goes as-is; anything else is bundled into a single archive
    let single = matches!(named.as_slice(), [(_, p)] if !p.is_dir());
    let (name, path, tmp) = if single {
        let (n, p) = named.remove(0);
        (n, p, false)
    } else {
        let arc = scratch().with_extension("tar.gz");
        pack(&named, &arc)?;
        (BUNDLE.to_string(), arc, true)
    };
    let total = std::fs::metadata(&path)?.len();
    let tag = store.blobs().add_path(&path).await;
    if tmp {
        std::fs::remove_file(&path)?;
    }
    let tt = [(name, tag?.hash)]
        .into_iter()
        .collect::<Collection>()
        .store(&store)
        .await?;
    store.tags().create(tt.hash_and_format()).await?;

    let endpoint = Endpoint::bind(presets::N0).await?;
    endpoint.online().await;
    let ticket = Ticket(endpoint.id());
    let done = Arc::new(Notify::new());
    let router = Router::builder(endpoint)
        .accept(iroh_blobs::ALPN, BlobsProtocol::new(&store, None))
        .accept(ROOT_ALPN, Root(tt.hash(), total, done.clone()))
        .spawn();
    Ok(Share {
        ticket,
        router,
        dir: dir.to_path_buf(),
        done,
    })
}

pub async fn get(
    ticket: &Ticket,
    out: &Path,
    ask: &mut dyn FnMut(&Path) -> bool,
) -> Result<Vec<PathBuf>> {
    let dir = scratch();
    let res = get_in(&dir, ticket, out, ask).await;
    let _ = std::fs::remove_dir_all(&dir);
    res
}

async fn get_in(
    dir: &Path,
    ticket: &Ticket,
    out: &Path,
    ask: &mut dyn FnMut(&Path) -> bool,
) -> Result<Vec<PathBuf>> {
    let store = FsStore::load(dir).await?;
    let endpoint = Endpoint::bind(presets::N0).await?;
    let res = get_with(&store, &endpoint, dir, ticket, out, ask).await;
    endpoint.close().await;
    res
}

async fn get_with(
    store: &FsStore,
    endpoint: &Endpoint,
    dir: &Path,
    ticket: &Ticket,
    out: &Path,
    ask: &mut dyn FnMut(&Path) -> bool,
) -> Result<Vec<PathBuf>> {
    let (conn, root, total) = fetch_root(endpoint, ticket).await?;
    let mut dl = store
        .downloader(endpoint)
        .download(HashAndFormat::hash_seq(root), vec![ticket.0])
        .stream()
        .await?;
    let tty = std::io::stderr().is_terminal();
    let start = Instant::now();
    while let Some(item) = dl.next().await {
        match item {
            DownloadProgressItem::Progress(n) if tty => trail(n, total, start),
            DownloadProgressItem::Error(e) => return Err(anyhow!("{e}")),
            DownloadProgressItem::DownloadError => return Err(anyhow!("download failed")),
            _ => {}
        }
    }
    if tty {
        trail(total, total, start);
        eprintln!();
    }
    let coll = Collection::load(root, store.as_ref()).await?;
    let out = &std::path::absolute(out)?;
    std::fs::create_dir_all(out)?;
    let mut done = Vec::new();
    for (name, hash) in coll.iter() {
        if name.ends_with('/') {
            let arc = dir.join("export.tar.gz");
            store.blobs().export(*hash, &arc).await?;
            let r = unpack(&arc, out, ask);
            let _ = std::fs::remove_file(&arc);
            done.extend(r?);
        } else {
            let safe = leaf(name)?;
            let dest = out.join(safe);
            if dest.exists() && !ask(&dest) {
                continue;
            }
            store.blobs().export(*hash, &dest).await?;
            done.push(dest);
        }
    }
    conn.close(0u32.into(), b"done");
    Ok(done)
}

fn trail(done: u64, total: u64, start: Instant) {
    let pct = (done * 100 / total.max(1)).min(100);
    let n = (pct / 10) as usize;
    let secs = start.elapsed().as_secs_f64();
    let left = if done == 0 || done >= total || secs < 1.0 {
        String::new()
    } else {
        let s = ((total - done) as f64 * secs / done as f64).round() as u64;
        match s {
            0..=59 => format!("  {s}s left"),
            _ => format!("  {}m {}s left", s / 60, s % 60),
        }
    };
    eprint!(
        "\r{}{} {pct:>3}%{left}\x1b[K",
        "o-".repeat(n),
        ". ".repeat(10 - n)
    );
}

fn pack(items: &[(String, PathBuf)], out: &Path) -> Result<()> {
    let enc =
        flate2::write::GzEncoder::new(std::fs::File::create(out)?, flate2::Compression::fast());
    let mut b = tar::Builder::new(enc);
    for (name, p) in items {
        if p.is_dir() {
            b.append_dir_all(name, p)?;
        } else {
            b.append_path_with_name(p, name)?;
        }
    }
    b.into_inner()?.finish()?;
    Ok(())
}

fn unpack(arc: &Path, dest: &Path, ask: &mut dyn FnMut(&Path) -> bool) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(dest)?;
    let mut tops = Vec::new();
    let mut skipped = Vec::new();
    let mut a = tar::Archive::new(flate2::read::GzDecoder::new(std::fs::File::open(arc)?));
    for e in a.entries()? {
        let mut e = e?;
        let top = e.path()?.components().find_map(|c| match c {
            Component::Normal(s) => Some(dest.join(s)),
            _ => None,
        });
        if let Some(t) = &top {
            if skipped.contains(t) {
                continue;
            }
            if !tops.contains(t) && t.exists() && !ask(t) {
                skipped.push(t.clone());
                continue;
            }
        }
        if e.unpack_in(dest)? {
            if let Some(t) = top.filter(|t| !tops.contains(t)) {
                tops.push(t);
            }
        }
    }
    Ok(tops)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (PathBuf, PathBuf) {
        let base = scratch();
        let src = base.join("src");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("a.txt"), b"hello").unwrap();
        std::fs::write(src.join("sub/b.txt"), b"world").unwrap();
        let arc = base.join("bundle.tar.gz");
        pack(&[("pkg".to_string(), src)], &arc).unwrap();
        (base, arc)
    }

    #[test]
    fn pack_unpack_roundtrip() {
        let (base, arc) = fixture();
        let out = base.join("out");
        let mut asked = 0;
        let got = unpack(&arc, &out, &mut |_| {
            asked += 1;
            true
        })
        .unwrap();
        assert_eq!(asked, 0);
        assert_eq!(got, vec![out.join("pkg")]);
        assert_eq!(std::fs::read(out.join("pkg/a.txt")).unwrap(), b"hello");
        assert_eq!(std::fs::read(out.join("pkg/sub/b.txt")).unwrap(), b"world");
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn unpack_skips_when_overwrite_is_declined() {
        let (base, arc) = fixture();
        let out = base.join("out");
        unpack(&arc, &out, &mut |_| true).unwrap();
        std::fs::write(out.join("pkg/a.txt"), b"keep").unwrap();

        let mut asked = Vec::new();
        let got = unpack(&arc, &out, &mut |p| {
            asked.push(p.to_path_buf());
            false
        })
        .unwrap();

        assert_eq!(asked, vec![out.join("pkg")]);
        assert!(got.is_empty());
        assert_eq!(std::fs::read(out.join("pkg/a.txt")).unwrap(), b"keep");
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn duplicate_names_are_rejected() {
        let dups = [PathBuf::from("a/notes.txt"), PathBuf::from("b/notes.txt")];
        assert!(file_names(&dups).is_err());
        assert!(file_names(&[PathBuf::from("a/notes.txt"), PathBuf::from("b/photo.jpg")]).is_ok());
    }
}
