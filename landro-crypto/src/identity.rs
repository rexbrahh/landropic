use ed25519_dalek::{SigningKey, VerifyingKey, Signature, Signer, Verifier};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::fs;
use zeroize::Zeroize;
use tracing::{debug, info};

use crate::errors::{CryptoError, Result};

/// Device identifier (Ed25519 public key)
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct DeviceId(pub [u8; 32]);

impl DeviceId {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != 32 {
            return Err(CryptoError::InvalidKeyFormat(
                "Device ID must be 32 bytes".to_string()
            ));
        }
        let mut id = [0u8; 32];
        id.copy_from_slice(bytes);
        Ok(DeviceId(id))
    }
    
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
    
    /// Generate a fingerprint for display/verification
    pub fn fingerprint(&self) -> String {
        let hash = blake3::hash(&self.0);
        let bytes = hash.as_bytes();
        
        // Format as groups of 4 hex chars separated by colons
        bytes[..16]
            .chunks(2)
            .map(|chunk| format!("{:02x}{:02x}", chunk[0], chunk[1]))
            .collect::<Vec<_>>()
            .join(":")
    }
}

impl std::fmt::Display for DeviceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.fingerprint())
    }
}

/// Device identity with Ed25519 keypair
pub struct DeviceIdentity {
    signing_key: SigningKey,
    verifying_key: VerifyingKey,
    device_name: String,
    storage_path: Option<PathBuf>,
}

impl Drop for DeviceIdentity {
    fn drop(&mut self) {
        // SigningKey implements Zeroize automatically
    }
}

impl DeviceIdentity {
    /// Generate a new device identity
    pub fn generate(device_name: impl Into<String>) -> Result<Self> {
        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key = signing_key.verifying_key();
        
        Ok(Self {
            signing_key,
            verifying_key,
            device_name: device_name.into(),
            storage_path: None,
        })
    }
    
    /// Get the device ID (public key)
    pub fn device_id(&self) -> DeviceId {
        DeviceId(self.verifying_key.to_bytes())
    }
    
    /// Get the device name
    pub fn device_name(&self) -> &str {
        &self.device_name
    }
    
    /// Sign a message
    pub fn sign(&self, message: &[u8]) -> Signature {
        self.signing_key.sign(message)
    }
    
    /// Verify a signature from another device
    pub fn verify_signature(
        device_id: &DeviceId,
        message: &[u8],
        signature: &Signature,
    ) -> Result<()> {
        let verifying_key = VerifyingKey::from_bytes(&device_id.0)
            .map_err(|e| CryptoError::InvalidKeyFormat(e.to_string()))?;
        
        verifying_key
            .verify(message, signature)
            .map_err(|_| CryptoError::SignatureVerification)?;
        
        Ok(())
    }
    
    /// Get the default storage directory for keys
    pub fn default_storage_dir() -> Result<PathBuf> {
        let home = dirs::home_dir()
            .ok_or_else(|| CryptoError::Io(
                std::io::Error::new(std::io::ErrorKind::NotFound, "Home directory not found")
            ))?;
        Ok(home.join(".landropic").join("keys"))
    }
    
    /// Save identity to disk
    pub async fn save(&mut self, path: Option<&Path>) -> Result<()> {
        let storage_path = if let Some(p) = path {
            p.to_path_buf()
        } else {
            Self::default_storage_dir()?.join("device_identity.key")
        };
        
        // Ensure directory exists
        if let Some(parent) = storage_path.parent() {
            fs::create_dir_all(parent).await?;
        }
        
        // Serialize identity (private key + metadata)
        #[derive(Serialize)]
        struct StoredIdentity<'a> {
            version: u8,
            device_name: &'a str,
            private_key: [u8; 32],
        }
        
        let stored = StoredIdentity {
            version: 1,
            device_name: &self.device_name,
            private_key: self.signing_key.to_bytes(),
        };
        
        let data = bincode::serialize(&stored)
            .map_err(|e| CryptoError::Serialization(e.to_string()))?;
        
        // Write with restricted permissions (owner read/write only)
        fs::write(&storage_path, data).await?;
        
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let metadata = fs::metadata(&storage_path).await?;
            let mut permissions = metadata.permissions();
            permissions.set_mode(0o600);
            fs::set_permissions(&storage_path, permissions).await?;
        }
        
        self.storage_path = Some(storage_path);
        info!("Device identity saved");
        
        Ok(())
    }
    
    /// Load identity from disk
    pub async fn load(path: Option<&Path>) -> Result<Self> {
        let storage_path = if let Some(p) = path {
            p.to_path_buf()
        } else {
            Self::default_storage_dir()?.join("device_identity.key")
        };
        
        if !storage_path.exists() {
            return Err(CryptoError::KeyNotFound(
                storage_path.display().to_string()
            ));
        }
        
        let data = fs::read(&storage_path).await?;
        
        #[derive(Deserialize)]
        struct StoredIdentity {
            version: u8,
            device_name: String,
            private_key: [u8; 32],
        }
        
        let mut stored: StoredIdentity = bincode::deserialize(&data)
            .map_err(|e| CryptoError::Serialization(e.to_string()))?;
        
        if stored.version != 1 {
            return Err(CryptoError::InvalidKeyFormat(
                format!("Unsupported key version: {}", stored.version)
            ));
        }
        
        let signing_key = SigningKey::from_bytes(&stored.private_key);
        let verifying_key = signing_key.verifying_key();
        
        // Clear the temporary private key from memory
        stored.private_key.zeroize();
        
        debug!("Device identity loaded from {:?}", storage_path);
        
        Ok(Self {
            signing_key,
            verifying_key,
            device_name: stored.device_name,
            storage_path: Some(storage_path),
        })
    }
    
    /// Load existing identity or generate a new one
    pub async fn load_or_generate(device_name: impl Into<String>) -> Result<Self> {
        match Self::load(None).await {
            Ok(identity) => {
                info!("Loaded existing device identity: {}", identity.device_id());
                Ok(identity)
            }
            Err(CryptoError::KeyNotFound(_)) => {
                info!("No existing identity found, generating new one");
                let mut identity = Self::generate(device_name)?;
                identity.save(None).await?;
                info!("Generated new device identity: {}", identity.device_id());
                Ok(identity)
            }
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    
    #[tokio::test]
    async fn test_device_identity_generation() {
        let identity = DeviceIdentity::generate("test-device").unwrap();
        assert_eq!(identity.device_name(), "test-device");
        
        let device_id = identity.device_id();
        assert_eq!(device_id.as_bytes().len(), 32);
    }
    
    #[tokio::test]
    async fn test_sign_and_verify() {
        let identity = DeviceIdentity::generate("test-device").unwrap();
        let message = b"Hello, Landropic!";
        
        let signature = identity.sign(message);
        let device_id = identity.device_id();
        
        // Should verify correctly
        DeviceIdentity::verify_signature(&device_id, message, &signature).unwrap();
        
        // Should fail with wrong message
        let wrong_message = b"Wrong message";
        let result = DeviceIdentity::verify_signature(&device_id, wrong_message, &signature);
        assert!(result.is_err());
    }
    
    #[tokio::test]
    async fn test_save_and_load() {
        let dir = tempdir().unwrap();
        let key_path = dir.path().join("test.key");
        
        // Generate and save
        let mut identity = DeviceIdentity::generate("test-device").unwrap();
        let original_id = identity.device_id();
        identity.save(Some(&key_path)).await.unwrap();
        
        // Load and verify
        let loaded = DeviceIdentity::load(Some(&key_path)).await.unwrap();
        assert_eq!(loaded.device_name(), "test-device");
        assert_eq!(loaded.device_id(), original_id);
    }
    
    #[test]
    fn test_device_id_fingerprint() {
        let bytes = [0u8; 32];
        let device_id = DeviceId(bytes);
        let fingerprint = device_id.fingerprint();
        
        // Should be formatted as groups of 4 hex chars with colons
        assert!(fingerprint.contains(':'));
        assert_eq!(fingerprint.len(), 39); // 8 groups of 4 chars + 7 colons
    }
}