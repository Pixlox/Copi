//! Encrypted Transport Layer
//!
//! Provides secure TCP communication using the Noise Protocol Framework.
//! Uses the Noise_XX_25519_ChaChaPoly_BLAKE2s pattern for mutual authentication.

use snow::{Builder, TransportState};
use std::net::{Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

use super::{SyncError, SyncResult};

/// Timeout for establishing a TCP connection to a peer
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// Noise protocol pattern for mutual authentication
/// XX pattern: Both parties authenticate each other
const NOISE_PATTERN: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";

/// Maximum message size (16 KB payload + noise overhead)
const MAX_MESSAGE_SIZE: usize = 16 * 1024 + 64;

/// Message framing: 2-byte length prefix (big-endian)
const LENGTH_PREFIX_SIZE: usize = 2;

/// Encrypted transport for a single connection
pub struct SecureTransport {
    reader: Arc<Mutex<ReadHalf<TcpStream>>>,
    writer: Arc<Mutex<WriteHalf<TcpStream>>>,
    noise: Arc<Mutex<TransportState>>,
    remote_public_key: Vec<u8>,
}

impl SecureTransport {
    /// Connect to a remote peer and establish encrypted channel (initiator)
    pub async fn connect(
        addr: SocketAddr,
        our_private_key: &[u8; 32],
        expected_public_key: Option<&[u8]>,
    ) -> SyncResult<Self> {
        let stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr))
            .await
            .map_err(|_| SyncError::ConnectionFailed(format!("Connection to {} timed out", addr)))?
            .map_err(|e| SyncError::ConnectionFailed(e.to_string()))?;

        let (noise, remote_public_key, stream) =
            Self::handshake_initiator(stream, our_private_key).await?;

        // Verify remote public key if expected
        if let Some(expected) = expected_public_key {
            if remote_public_key != expected {
                return Err(SyncError::EncryptionError(
                    "Remote public key mismatch".into(),
                ));
            }
        }

        let (reader, writer) = tokio::io::split(stream);

        Ok(Self {
            reader: Arc::new(Mutex::new(reader)),
            writer: Arc::new(Mutex::new(writer)),
            noise: Arc::new(Mutex::new(noise)),
            remote_public_key,
        })
    }

    /// Accept a connection and establish encrypted channel (responder)
    pub async fn accept(stream: TcpStream, our_private_key: &[u8; 32]) -> SyncResult<Self> {
        let (noise, remote_public_key, stream) =
            Self::handshake_responder(stream, our_private_key).await?;

        let (reader, writer) = tokio::io::split(stream);

        Ok(Self {
            reader: Arc::new(Mutex::new(reader)),
            writer: Arc::new(Mutex::new(writer)),
            noise: Arc::new(Mutex::new(noise)),
            remote_public_key,
        })
    }

    /// Perform Noise XX handshake as initiator
    async fn handshake_initiator(
        mut stream: TcpStream,
        our_private_key: &[u8; 32],
    ) -> SyncResult<(TransportState, Vec<u8>, TcpStream)> {
        let builder = Builder::new(NOISE_PATTERN.parse().unwrap());
        let mut noise = builder
            .local_private_key(our_private_key)
            .build_initiator()
            .map_err(|e| SyncError::EncryptionError(e.to_string()))?;

        let mut buf = vec![0u8; MAX_MESSAGE_SIZE];
        let mut read_buf = vec![0u8; MAX_MESSAGE_SIZE];

        // -> e
        let len = noise
            .write_message(&[], &mut buf)
            .map_err(|e| SyncError::EncryptionError(e.to_string()))?;
        Self::send_raw(&mut stream, &buf[..len]).await?;

        // <- e, ee, s, es
        let msg = Self::recv_raw(&mut stream, &mut read_buf).await?;
        noise
            .read_message(msg, &mut buf)
            .map_err(|e| SyncError::EncryptionError(e.to_string()))?;

        // -> s, se
        let len = noise
            .write_message(&[], &mut buf)
            .map_err(|e| SyncError::EncryptionError(e.to_string()))?;
        Self::send_raw(&mut stream, &buf[..len]).await?;

        // Get remote public key before converting to transport mode
        let remote_public_key = noise
            .get_remote_static()
            .ok_or_else(|| SyncError::EncryptionError("No remote public key".into()))?
            .to_vec();

        // Convert to transport mode
        let noise = noise
            .into_transport_mode()
            .map_err(|e| SyncError::EncryptionError(e.to_string()))?;

        Ok((noise, remote_public_key, stream))
    }

    /// Perform Noise XX handshake as responder
    async fn handshake_responder(
        mut stream: TcpStream,
        our_private_key: &[u8; 32],
    ) -> SyncResult<(TransportState, Vec<u8>, TcpStream)> {
        let builder = Builder::new(NOISE_PATTERN.parse().unwrap());
        let mut noise = builder
            .local_private_key(our_private_key)
            .build_responder()
            .map_err(|e| SyncError::EncryptionError(e.to_string()))?;

        let mut buf = vec![0u8; MAX_MESSAGE_SIZE];
        let mut read_buf = vec![0u8; MAX_MESSAGE_SIZE];

        // <- e
        let msg = Self::recv_raw(&mut stream, &mut read_buf).await?;
        noise
            .read_message(msg, &mut buf)
            .map_err(|e| SyncError::EncryptionError(e.to_string()))?;

        // -> e, ee, s, es
        let len = noise
            .write_message(&[], &mut buf)
            .map_err(|e| SyncError::EncryptionError(e.to_string()))?;
        Self::send_raw(&mut stream, &buf[..len]).await?;

        // <- s, se
        let msg = Self::recv_raw(&mut stream, &mut read_buf).await?;
        noise
            .read_message(msg, &mut buf)
            .map_err(|e| SyncError::EncryptionError(e.to_string()))?;

        // Get remote public key before converting to transport mode
        let remote_public_key = noise
            .get_remote_static()
            .ok_or_else(|| SyncError::EncryptionError("No remote public key".into()))?
            .to_vec();

        // Convert to transport mode
        let noise = noise
            .into_transport_mode()
            .map_err(|e| SyncError::EncryptionError(e.to_string()))?;

        Ok((noise, remote_public_key, stream))
    }

    /// Send a message through the encrypted channel
    pub async fn send(&self, data: &[u8]) -> SyncResult<()> {
        if data.len() > MAX_MESSAGE_SIZE - 64 {
            return Err(SyncError::TransportError("Message too large".into()));
        }

        let mut noise = self.noise.lock().await;
        let mut buf = vec![0u8; MAX_MESSAGE_SIZE];

        let len = noise
            .write_message(data, &mut buf)
            .map_err(|e| SyncError::EncryptionError(e.to_string()))?;

        let mut writer = self.writer.lock().await;
        Self::send_raw_writer(&mut *writer, &buf[..len]).await
    }

    /// Receive a message from the encrypted channel
    pub async fn recv(&self) -> SyncResult<Vec<u8>> {
        let mut noise = self.noise.lock().await;
        let mut read_buf = vec![0u8; MAX_MESSAGE_SIZE];
        let mut out_buf = vec![0u8; MAX_MESSAGE_SIZE];

        let mut reader = self.reader.lock().await;
        let msg = Self::recv_raw_reader(&mut *reader, &mut read_buf).await?;

        let len = noise
            .read_message(msg, &mut out_buf)
            .map_err(|e| SyncError::EncryptionError(e.to_string()))?;

        Ok(out_buf[..len].to_vec())
    }

    /// Send raw bytes with length prefix (for handshake)
    async fn send_raw(stream: &mut TcpStream, data: &[u8]) -> SyncResult<()> {
        let len = data.len() as u16;
        stream.write_all(&len.to_be_bytes()).await?;
        stream.write_all(data).await?;
        stream.flush().await?;
        Ok(())
    }

    /// Send raw bytes with length prefix (for established connection)
    async fn send_raw_writer(writer: &mut WriteHalf<TcpStream>, data: &[u8]) -> SyncResult<()> {
        let len = data.len() as u16;
        writer.write_all(&len.to_be_bytes()).await?;
        writer.write_all(data).await?;
        writer.flush().await?;
        Ok(())
    }

    /// Receive raw bytes with length prefix (for handshake)
    async fn recv_raw<'a>(stream: &mut TcpStream, buf: &'a mut [u8]) -> SyncResult<&'a [u8]> {
        let mut len_buf = [0u8; LENGTH_PREFIX_SIZE];
        stream.read_exact(&mut len_buf).await?;
        let len = u16::from_be_bytes(len_buf) as usize;

        if len > buf.len() {
            return Err(SyncError::TransportError("Message too large".into()));
        }

        stream.read_exact(&mut buf[..len]).await?;
        Ok(&buf[..len])
    }

    /// Receive raw bytes with length prefix (for established connection)
    async fn recv_raw_reader<'a>(
        reader: &mut ReadHalf<TcpStream>,
        buf: &'a mut [u8],
    ) -> SyncResult<&'a [u8]> {
        let mut len_buf = [0u8; LENGTH_PREFIX_SIZE];
        reader.read_exact(&mut len_buf).await?;
        let len = u16::from_be_bytes(len_buf) as usize;

        if len > buf.len() {
            return Err(SyncError::TransportError("Message too large".into()));
        }

        reader.read_exact(&mut buf[..len]).await?;
        Ok(&buf[..len])
    }

    /// Get the remote peer's public key
    pub fn remote_public_key(&self) -> &[u8] {
        &self.remote_public_key
    }

}

/// TCP listener for accepting encrypted connections
pub struct SecureListener {
    listener: TcpListener,
    our_private_key: [u8; 32],
}

impl SecureListener {
    /// Create a new listener on the specified port.
    /// Attempts dual-stack binding ([::]) first — this accepts both IPv4 and
    /// IPv6 connections on every major OS. Falls back to IPv4-only (0.0.0.0)
    /// when dual-stack is unavailable.
    pub async fn bind(port: u16, our_private_key: [u8; 32]) -> SyncResult<Self> {
        let listener = match TcpListener::bind(SocketAddr::from((Ipv6Addr::UNSPECIFIED, port))).await {
            Ok(l) => {
                eprintln!("[Sync] Listening on port {} (dual-stack)", port);
                l
            }
            Err(_) => {
                let l = TcpListener::bind(("0.0.0.0", port)).await?;
                eprintln!("[Sync] Listening on port {} (IPv4 only)", port);
                l
            }
        };
        Ok(Self {
            listener,
            our_private_key,
        })
    }

    /// Accept a raw TCP connection (no Noise handshake yet).
    /// Returns the stream and peer address so the caller can spawn
    /// the handshake in a separate task with its own timeout.
    pub async fn accept_tcp(&self) -> SyncResult<(TcpStream, std::net::SocketAddr)> {
        let (stream, addr) = self.listener.accept().await?;
        Ok((stream, addr))
    }

    /// Get a copy of our private key (for spawning handshake tasks)
    pub fn private_key(&self) -> [u8; 32] {
        self.our_private_key
    }
}
