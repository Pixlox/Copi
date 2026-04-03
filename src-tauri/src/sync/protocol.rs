//! Wire protocol for clipboard sync.
//!
//! All messages are newline-delimited JSON (NDJSON).
//! One message per line, terminated by '\n'.

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u8 = 2;

/// Top-level wire message (tagged enum for serde).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Msg {
    /// First message sent after TCP + Noise handshake.
    Hello {
        device_id: String,
        device_name: String,
        protocol_version: u8,
        last_sync_version: i64,
    },
    HelloAck {
        device_id: String,
        device_name: String,
        protocol_version: u8,
        last_sync_version: i64,
    },
    /// Pairing request (sent over Noise XX during initial pairing).
    PairRequest {
        device_id: String,
        device_name: String,
        platform: String,
        public_key: String, // base64 X25519 public key
        pin: String,
    },
    PairAccept {
        device_id: String,
        device_name: String,
        platform: String,
        public_key: String, // base64 X25519 public key
    },
    PairReject {
        reason: String,
    },
    /// A single clip (used for both delta sync and real-time push).
    Clip {
        clip: WireClip,
    },
    /// A single collection.
    Collection {
        collection: WireCollection,
    },
    /// Move clip to collection.
    MoveClipToCollection {
        clip_sync_id: String,
        collection_sync_id: Option<String>,
    },
    /// Pin/unpin clip.
    SetClipPinned {
        sync_id: String,
        pinned: bool,
    },
    /// Delete clip (soft delete).
    DeleteClip {
        sync_id: String,
    },
    /// Delete collection (soft delete).
    DeleteCollection {
        sync_id: String,
    },
    Ping,
    Pong,
}

/// Serializable clip for the wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireClip {
    pub sync_id: String,
    pub sync_version: i64,
    pub content_hash: String,
    pub content: String,
    pub content_type: String, // "text" | "url" | "code" | "image"
    pub source_app: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_app_icon: Option<String>, // base64 PNG
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_highlighted: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ocr_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_data: Option<String>, // base64 PNG (only for images, may be omitted in phase 1)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_thumbnail: Option<String>, // base64 PNG
    pub image_width: i32,
    pub image_height: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    pub created_at: i64,
    pub pinned: bool,
    pub copy_count: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection_sync_id: Option<String>,
    pub origin_device_id: String,
    pub deleted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding: Option<String>, // base64 f32[384] (1536 bytes)
}

/// Serializable collection for the wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireCollection {
    pub sync_id: String,
    pub sync_version: i64,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    pub created_at: i64,
    pub origin_device_id: String,
    pub deleted: bool,
}

impl WireClip {
    /// Encode embedding vector to base64.
    pub fn set_embedding(&mut self, embedding: Option<&[f32]>) {
        self.embedding = embedding.map(|e| {
            let bytes: Vec<u8> = e.iter().flat_map(|f| f.to_le_bytes()).collect();
            B64.encode(&bytes)
        });
    }

    /// Decode embedding vector from base64.
    pub fn get_embedding(&self) -> Option<Vec<f32>> {
        let bytes = B64.decode(self.embedding.as_ref()?).ok()?;
        if bytes.len() % 4 != 0 {
            return None;
        }
        Some(
            bytes
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect(),
        )
    }
}

impl Msg {
    /// Serialize to a line of JSON + '\n'.
    pub fn to_line(&self) -> Result<String, serde_json::Error> {
        Ok(serde_json::to_string(self)? + "\n")
    }

    /// Parse from a line of JSON.
    pub fn from_line(line: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_roundtrip() {
        let msg = Msg::Hello {
            device_id: "abc".into(),
            device_name: "my-mac".into(),
            protocol_version: PROTOCOL_VERSION,
            last_sync_version: 42,
        };
        let line = msg.to_line().unwrap();
        let parsed = Msg::from_line(&line).unwrap();
        match parsed {
            Msg::Hello {
                device_id,
                last_sync_version,
                ..
            } => {
                assert_eq!(device_id, "abc");
                assert_eq!(last_sync_version, 42);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn clip_roundtrip() {
        let clip = WireClip {
            sync_id: "s1".into(),
            sync_version: 1,
            content_hash: "h1".into(),
            content: "hello".into(),
            content_type: "text".into(),
            source_app: "Chrome".into(),
            source_app_icon: None,
            content_highlighted: None,
            ocr_text: None,
            image_data: None,
            image_thumbnail: None,
            image_width: 0,
            image_height: 0,
            language: Some("en".into()),
            created_at: 1000,
            pinned: false,
            copy_count: 0,
            collection_sync_id: None,
            origin_device_id: "dev1".into(),
            deleted: false,
            embedding: None,
        };
        let msg = Msg::Clip { clip };
        let line = msg.to_line().unwrap();
        let parsed = Msg::from_line(&line).unwrap();
        match parsed {
            Msg::Clip { clip } => {
                assert_eq!(clip.sync_id, "s1");
                assert_eq!(clip.content, "hello");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn embedding_roundtrip() {
        let mut clip = WireClip {
            sync_id: "s1".into(),
            sync_version: 1,
            content_hash: "h1".into(),
            content: "test".into(),
            content_type: "text".into(),
            source_app: "".into(),
            source_app_icon: None,
            content_highlighted: None,
            ocr_text: None,
            image_data: None,
            image_thumbnail: None,
            image_width: 0,
            image_height: 0,
            language: None,
            created_at: 0,
            pinned: false,
            copy_count: 0,
            collection_sync_id: None,
            origin_device_id: "".into(),
            deleted: false,
            embedding: None,
        };
        let vec: Vec<f32> = (0..384).map(|i| i as f32 * 0.01).collect();
        clip.set_embedding(Some(&vec));
        let decoded = clip.get_embedding().unwrap();
        assert_eq!(decoded.len(), 384);
        assert!((decoded[0] - 0.0).abs() < 0.0001);
        assert!((decoded[383] - 3.83).abs() < 0.0001);
    }

    #[test]
    fn all_variants_serialize() {
        let msgs: Vec<Msg> = vec![
            Msg::Hello {
                device_id: "a".into(),
                device_name: "b".into(),
                protocol_version: 1,
                last_sync_version: 0,
            },
            Msg::HelloAck {
                device_id: "a".into(),
                device_name: "b".into(),
                protocol_version: 1,
                last_sync_version: 0,
            },
            Msg::PairRequest {
                device_id: "a".into(),
                device_name: "b".into(),
                platform: "macos".into(),
                public_key: "key".into(),
                pin: "123456".into(),
            },
            Msg::PairAccept {
                device_id: "a".into(),
                device_name: "b".into(),
                platform: "macos".into(),
                public_key: "key".into(),
            },
            Msg::PairReject {
                reason: "bad".into(),
            },
            Msg::Ping,
            Msg::Pong,
            Msg::DeleteClip {
                sync_id: "s1".into(),
            },
            Msg::SetClipPinned {
                sync_id: "s1".into(),
                pinned: true,
            },
        ];
        for msg in msgs {
            let line = msg.to_line().unwrap();
            let parsed = Msg::from_line(&line).unwrap();
            assert!(matches!(
                (std::mem::discriminant(&msg), std::mem::discriminant(&parsed)),
                (a, b) if a == b
            ));
        }
    }
}
