//! the wire bits shared by the CLI and the browser build
use std::{sync::Arc, time::Duration};

use anyhow::{anyhow, Result};
use iroh::{
    endpoint::{Connection, ConnectionError},
    protocol::{AcceptError, ProtocolHandler},
    Endpoint, EndpointId,
};
use iroh_blobs::Hash;
use n0_future::time::timeout;
use tokio::sync::Notify;

pub const ROOT_ALPN: &[u8] = b"yoink/root/0";
/// how long to wait for the sender to answer with its root hash
pub const ROOT_TIMEOUT: Duration = Duration::from_secs(30);

/// collection name of a bundled archive; a plain file keeps its own name
pub const BUNDLE: &str = "yoink/";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ticket(pub EndpointId);

impl std::fmt::Display for Ticket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            &data_encoding::BASE32_NOPAD
                .encode(self.0.as_bytes())
                .to_lowercase(),
        )
    }
}

impl std::str::FromStr for Ticket {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let b = data_encoding::BASE32_NOPAD.decode(s.trim().to_uppercase().as_bytes())?;
        let b: [u8; 32] = b.try_into().map_err(|_| anyhow!("bad ticket length"))?;
        Ok(Self(EndpointId::from_bytes(&b)?))
    }
}

/// last path segment of a name from a peer, so it can't escape the output folder
pub fn leaf(name: &str) -> Result<&str> {
    name.rsplit(['/', '\\'])
        .next()
        .filter(|n| !n.is_empty() && *n != "..")
        .ok_or_else(|| anyhow!("bad name from peer: {name}"))
}

/// answers a root request with the share's root hash and payload size, and
/// notifies once the receiver closed the connection cleanly
#[derive(Debug, Clone)]
pub struct Root(pub Hash, pub u64, pub Arc<Notify>);

impl ProtocolHandler for Root {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        let (mut send, mut recv) = conn.accept_bi().await?;
        timeout(ROOT_TIMEOUT, recv.read_exact(&mut [0u8; 1]))
            .await
            .map_err(|_| {
                AcceptError::from_err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "root request timed out",
                ))
            })?
            .map_err(AcceptError::from_err)?;
        send.write_all(self.0.as_bytes())
            .await
            .map_err(AcceptError::from_err)?;
        send.write_all(&self.1.to_be_bytes())
            .await
            .map_err(AcceptError::from_err)?;
        send.finish()?;
        if let ConnectionError::ApplicationClosed(c) = conn.closed().await {
            if c.error_code == 0u32.into() {
                self.2.notify_one();
            }
        }
        Ok(())
    }
}

/// asks the sender for its root hash and payload size; keep the connection
/// open until the download is done and close it with code 0
pub async fn fetch_root(endpoint: &Endpoint, ticket: &Ticket) -> Result<(Connection, Hash, u64)> {
    timeout(ROOT_TIMEOUT, async {
        let conn = endpoint.connect(ticket.0, ROOT_ALPN).await?;
        let (mut send, mut recv) = conn.open_bi().await?;
        send.write_all(&[0]).await?;
        send.finish()?;
        let mut head = [0u8; 40];
        recv.read_exact(&mut head).await?;
        Ok((
            conn,
            Hash::from_bytes(head[..32].try_into()?),
            u64::from_be_bytes(head[32..].try_into()?),
        ))
    })
    .await
    .map_err(|_| anyhow!("sender did not respond in time"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticket_roundtrip_is_short_and_lowercase() {
        let t = Ticket(iroh::SecretKey::generate().public());
        let s = t.to_string();
        assert_eq!(s.len(), 52);
        assert!(s
            .chars()
            .all(|c| c.is_ascii_lowercase() || ('2'..='7').contains(&c)));
        assert_eq!(s.parse::<Ticket>().unwrap(), t);
        assert_eq!(s.to_uppercase().parse::<Ticket>().unwrap(), t);
    }

    #[test]
    fn ticket_rejects_bad_input() {
        assert!("".parse::<Ticket>().is_err());
        assert!("not-a-ticket".parse::<Ticket>().is_err());
        assert!("a".repeat(51).parse::<Ticket>().is_err());
        assert!("a".repeat(53).parse::<Ticket>().is_err());
    }

    #[test]
    fn leaf_strips_directories_and_rejects_escapes() {
        assert_eq!(leaf("photos/cat.jpg").unwrap(), "cat.jpg");
        assert_eq!(leaf(r"photos\cat.jpg").unwrap(), "cat.jpg");
        assert_eq!(leaf("cat.jpg").unwrap(), "cat.jpg");
        assert!(leaf("").is_err());
        assert!(leaf("/").is_err());
        assert!(leaf("..").is_err());
        assert!(leaf("photos/..").is_err());
        assert!(leaf("photos/").is_err());
    }
}
