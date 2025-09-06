use blake3;
use std::fmt;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Content hash wrapper for blake3::Hash
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ContentHash(#[cfg_attr(feature = "serde", serde(with = "hash_serde"))] blake3::Hash);

impl ContentHash {
    /// Create from blake3 hash
    pub fn from_blake3(hash: blake3::Hash) -> Self {
        Self(hash)
    }

    /// Get as bytes
    pub fn as_bytes(&self) -> &[u8; 32] {
        self.0.as_bytes()
    }

    /// Create from bytes
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(blake3::Hash::from_bytes(bytes))
    }

    /// Convert to hex string
    pub fn to_hex(&self) -> String {
        self.0.to_hex().to_string()
    }

    /// Parse from hex string
    pub fn from_hex(hex: &str) -> Result<Self, hex::FromHexError> {
        let bytes = hex::decode(hex)?;
        if bytes.len() != 32 {
            return Err(hex::FromHexError::InvalidStringLength);
        }
        let mut array = [0u8; 32];
        array.copy_from_slice(&bytes);
        Ok(Self::from_bytes(array))
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ContentHash({}...)", &self.to_hex()[..8])
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

/// Chunk hash (same as content hash but semantically different)
pub type ChunkHash = ContentHash;

#[cfg(feature = "serde")]
mod hash_serde {
    use super::*;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S>(hash: &blake3::Hash, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        hash.as_bytes().serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<blake3::Hash, D::Error>
    where
        D: Deserializer<'de>,
    {
        let bytes: [u8; 32] = Deserialize::deserialize(deserializer)?;
        Ok(blake3::Hash::from_bytes(bytes))
    }
}
