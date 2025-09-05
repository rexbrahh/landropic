use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::{debug, info};
use zeroize::Zeroize;

use crate::errors::{CryptoError, Result};

/// A unique device identifier derived from an Ed25519 public key.
///
/// Device IDs are used throughout Landropic to identify and authenticate devices
/// in the network. They are derived from the public portion of a device's long-term
/// Ed25519 identity keypair.
///
/// # Example
///
/// ```rust
/// use landro_crypto::DeviceId;
///
/// // Create from raw bytes
/// let bytes = [0u8; 32]; // In practice, this would be a real public key
/// let device_id = DeviceId::from_bytes(&bytes).unwrap();
///
/// // Display as a human-readable fingerprint
/// println!("Device: {}", device_id); // Shows fingerprint format
/// ```
///
/// Device IDs are displayed as fingerprints - Blake3 hashes of the public key
/// formatted as colon-separated hex groups for easy verification.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct DeviceId(pub [u8; 32]);

impl DeviceId {
    /// Creates a DeviceId from raw bytes.
    ///
    /// # Arguments
    ///
    /// * `bytes` - A 32-byte Ed25519 public key
    ///
    /// # Errors
    ///
    /// Returns `CryptoError::InvalidKeyFormat` if the input is not exactly 32 bytes.
    ///
    /// # Example
    ///
    /// ```rust
    /// use landro_crypto::DeviceId;
    ///
    /// let key_bytes = [0u8; 32]; // Normally from Ed25519 key generation
    /// let device_id = DeviceId::from_bytes(&key_bytes).unwrap();
    /// ```
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != 32 {
            return Err(CryptoError::InvalidKeyFormat(
                "Device ID must be 32 bytes".to_string(),
            ));
        }
        let mut id = [0u8; 32];
        id.copy_from_slice(bytes);
        Ok(DeviceId(id))
    }

    /// Returns the raw 32-byte public key.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Returns the device ID as a hex string (for file names, etc.)
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    /// Generates a human-readable fingerprint for device verification.
    ///
    /// The fingerprint is created by taking a Blake3 hash of the device's public key
    /// and formatting the first 16 bytes as colon-separated hex groups. This provides
    /// a shorter, more manageable string for manual verification during device pairing.
    ///
    /// # Example
    ///
    /// ```rust
    /// use landro_crypto::DeviceId;
    ///
    /// let device_id = DeviceId::from_bytes(&[0u8; 32]).unwrap();
    /// let fingerprint = device_id.fingerprint();
    /// // Output format: "af13:49b5:9011:628c:5e9c:7c5e:7b91:2345"
    /// ```
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

impl std::str::FromStr for DeviceId {
    type Err = CryptoError;
    
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        // Try to parse as hex string (64 characters for 32 bytes)
        if s.len() == 64 {
            let bytes = hex::decode(s)
                .map_err(|e| CryptoError::InvalidKeyFormat(format!("Invalid hex string: {}", e)))?;
            Self::from_bytes(&bytes)
        } else {
            Err(CryptoError::InvalidKeyFormat(format!(
                "Invalid DeviceId string length: {} (expected 64 hex characters)", 
                s.len()
            )))
        }
    }
}

/// A device's long-term cryptographic identity using Ed25519 keys.
///
/// Each device in a Landropic network has a unique identity consisting of:
/// - An Ed25519 keypair for signing and authentication
/// - A human-readable device name
/// - Optional persistent storage for the private key
///
/// The identity is used for:
/// - Device authentication during network discovery
/// - Message signing for integrity verification
/// - Certificate generation for TLS connections
/// - Device pairing and trust establishment
///
/// # Security
///
/// - Private keys are stored with restricted file permissions (0600 on Unix)
/// - Keys are zeroized in memory when the identity is dropped
/// - Cryptographically secure random number generation (OsRng)
///
/// # Example
///
/// ```rust
/// use landro_crypto::DeviceIdentity;
///
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// // Generate a new identity
/// let identity = DeviceIdentity::generate("my-laptop")?;
///
/// // Sign a message
/// let message = b"Hello, Landropic!";
/// let signature = identity.sign(message);
///
/// // Verify the signature
/// let device_id = identity.device_id();
/// DeviceIdentity::verify_signature(&device_id, message, &signature)?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
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
    /// Generates a new device identity with a cryptographically secure random Ed25519 keypair.
    ///
    /// # Arguments
    ///
    /// * `device_name` - A human-readable name for this device (e.g., "John's MacBook")
    ///
    /// # Example
    ///
    /// ```rust
    /// use landro_crypto::DeviceIdentity;
    ///
    /// let identity = DeviceIdentity::generate("my-device").unwrap();
    /// println!("Generated device: {} ({})",
    ///          identity.device_name(),
    ///          identity.device_id());
    /// ```
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

    /// Signs a message using this device's private key.
    ///
    /// This is used for authenticating messages during the sync protocol,
    /// ensuring that other devices can verify the message came from this device.
    ///
    /// # Arguments
    ///
    /// * `message` - The message bytes to sign
    ///
    /// # Returns
    ///
    /// An Ed25519 signature that can be verified using this device's public key.
    ///
    /// # Example
    ///
    /// ```rust
    /// use landro_crypto::DeviceIdentity;
    ///
    /// let identity = DeviceIdentity::generate("test-device").unwrap();
    /// let message = b"sync request";
    /// let signature = identity.sign(message);
    ///
    /// // Other devices can verify this signature
    /// DeviceIdentity::verify_signature(
    ///     &identity.device_id(),
    ///     message,
    ///     &signature
    /// ).unwrap();
    /// ```
    pub fn sign(&self, message: &[u8]) -> Signature {
        self.signing_key.sign(message)
    }

    /// Verifies a signature against a device ID (static method).
    ///
    /// This is used to authenticate messages from remote devices during sync operations.
    /// The signature must have been created by the device corresponding to the given device_id.
    ///
    /// # Arguments
    ///
    /// * `device_id` - The ID of the device that allegedly signed the message
    /// * `message` - The original message that was signed
    /// * `signature` - The signature to verify
    ///
    /// # Errors
    ///
    /// Returns `CryptoError::SignatureVerification` if the signature is invalid,
    /// or `CryptoError::InvalidKeyFormat` if the device ID is malformed.
    ///
    /// # Example
    ///
    /// ```rust
    /// use landro_crypto::DeviceIdentity;
    ///
    /// let alice = DeviceIdentity::generate("Alice").unwrap();
    /// let message = b"Hello Bob!";
    /// let signature = alice.sign(message);
    ///
    /// // Bob can verify Alice's signature
    /// DeviceIdentity::verify_signature(
    ///     &alice.device_id(),
    ///     message,
    ///     &signature
    /// ).unwrap();
    /// ```
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
        let home = dirs::home_dir().ok_or_else(|| {
            CryptoError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Home directory not found",
            ))
        })?;
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

        // Serialize identity (private key + metadata) with secure handling
        #[derive(Serialize, Zeroize)]
        #[zeroize(drop)]
        struct StoredIdentity {
            version: u8,
            device_name: String,
            private_key: [u8; 32],
        }

        let mut stored = StoredIdentity {
            version: 1,
            device_name: self.device_name.clone(),
            private_key: self.signing_key.to_bytes(),
        };

        let data =
            bincode::serialize(&stored).map_err(|e| CryptoError::Serialization(e.to_string()))?;

        // Zeroize the temporary struct
        stored.zeroize();

        // Write to a temporary file first, then atomically rename
        let temp_path = storage_path.with_extension("tmp");
        fs::write(&temp_path, &data).await?;

        // Set secure permissions before atomically moving
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let metadata = fs::metadata(&temp_path).await?;
            let mut permissions = metadata.permissions();
            permissions.set_mode(0o600);
            fs::set_permissions(&temp_path, permissions).await?;
        }

        #[cfg(windows)]
        {
            // On Windows, set file as hidden and restrict access
            use std::os::windows::fs::MetadataExt;
            use std::process::Command;

            // Attempt to set file as hidden and restrict permissions
            let _ = Command::new("icacls")
                .args(&[
                    temp_path.to_string_lossy().as_ref(),
                    "/inheritance:r",
                    "/grant:r",
                    &format!("{}:F", whoami::username()),
                ])
                .status();
        }

        // Atomically move temp file to final location
        fs::rename(&temp_path, &storage_path).await?;

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
            return Err(CryptoError::KeyNotFound(storage_path.display().to_string()));
        }

        let data = fs::read(&storage_path).await?;

        #[derive(Deserialize)]
        struct StoredIdentity {
            version: u8,
            device_name: String,
            private_key: [u8; 32],
        }

        let mut stored: StoredIdentity =
            bincode::deserialize(&data).map_err(|e| CryptoError::Serialization(e.to_string()))?;

        if stored.version != 1 {
            return Err(CryptoError::InvalidKeyFormat(format!(
                "Unsupported key version: {}",
                stored.version
            )));
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

    /// Loads an existing identity from the default location, or generates a new one if none exists.
    ///
    /// This is the recommended way to obtain a device identity for long-running applications.
    /// It first attempts to load an existing identity from `~/.landropic/keys/device_identity.key`.
    /// If no identity file exists, it generates a new one and saves it.
    ///
    /// # Arguments
    ///
    /// * `device_name` - Name for the device (only used if generating a new identity)
    ///
    /// # Errors
    ///
    /// Returns errors for I/O failures, permission issues, or key format problems.
    /// Does not return an error if no existing key is found - it will generate a new one.
    ///
    /// # Example
    ///
    /// ```rust
    /// use landro_crypto::DeviceIdentity;
    ///
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// // First run: generates and saves a new identity
    /// let identity1 = DeviceIdentity::load_or_generate("my-device").await?;
    ///
    /// // Subsequent runs: loads the existing identity
    /// let identity2 = DeviceIdentity::load_or_generate("my-device").await?;
    ///
    /// assert_eq!(identity1.device_id(), identity2.device_id());
    /// # Ok(())
    /// # }
    /// ```
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

    // === ERROR PATH AND EDGE CASE TESTS ===

    #[test]
    fn test_device_id_from_bytes_errors() {
        // Test invalid lengths
        let too_short = [0u8; 31];
        let result = DeviceId::from_bytes(&too_short);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            CryptoError::InvalidKeyFormat(_)
        ));

        let too_long = [0u8; 33];
        let result = DeviceId::from_bytes(&too_long);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            CryptoError::InvalidKeyFormat(_)
        ));

        // Empty slice
        let empty = [];
        let result = DeviceId::from_bytes(&empty);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            CryptoError::InvalidKeyFormat(_)
        ));
    }

    #[tokio::test]
    async fn test_load_nonexistent_key() {
        let dir = tempdir().unwrap();
        let nonexistent_path = dir.path().join("nonexistent.key");

        let result = DeviceIdentity::load(Some(&nonexistent_path)).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), CryptoError::KeyNotFound(_)));
    }

    #[tokio::test]
    async fn test_load_corrupted_key_file() {
        let dir = tempdir().unwrap();
        let corrupted_path = dir.path().join("corrupted.key");

        // Write invalid data
        tokio::fs::write(&corrupted_path, b"not a valid key file")
            .await
            .unwrap();

        let result = DeviceIdentity::load(Some(&corrupted_path)).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), CryptoError::Serialization(_)));
    }

    #[tokio::test]
    async fn test_load_unsupported_version() {
        let dir = tempdir().unwrap();
        let version_path = dir.path().join("version.key");

        // Create a key file with unsupported version
        #[derive(serde::Serialize)]
        struct BadVersionKey {
            version: u8,
            device_name: String,
            private_key: [u8; 32],
        }

        let bad_key = BadVersionKey {
            version: 99, // Unsupported version
            device_name: "test".to_string(),
            private_key: [0u8; 32],
        };

        let data = bincode::serialize(&bad_key).unwrap();
        tokio::fs::write(&version_path, &data).await.unwrap();

        let result = DeviceIdentity::load(Some(&version_path)).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            CryptoError::InvalidKeyFormat(_)
        ));
    }

    #[tokio::test]
    async fn test_verify_signature_with_wrong_key() {
        let identity1 = DeviceIdentity::generate("device1").unwrap();
        let identity2 = DeviceIdentity::generate("device2").unwrap();

        let message = b"test message";
        let signature = identity1.sign(message);

        // Try to verify with different device's ID
        let device_id2 = identity2.device_id();
        let result = DeviceIdentity::verify_signature(&device_id2, message, &signature);

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            CryptoError::SignatureVerification
        ));
    }

    #[tokio::test]
    async fn test_save_to_readonly_directory() {
        let dir = tempdir().unwrap();
        let readonly_dir = dir.path().join("readonly");
        tokio::fs::create_dir(&readonly_dir).await.unwrap();

        // Set directory as read-only (Unix only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = tokio::fs::metadata(&readonly_dir)
                .await
                .unwrap()
                .permissions();
            perms.set_mode(0o444); // Read-only
            tokio::fs::set_permissions(&readonly_dir, perms)
                .await
                .unwrap();

            let readonly_key_path = readonly_dir.join("test.key");
            let mut identity = DeviceIdentity::generate("test-device").unwrap();

            let result = identity.save(Some(&readonly_key_path)).await;
            assert!(result.is_err());
            assert!(matches!(result.unwrap_err(), CryptoError::Io(_)));

            // Restore permissions for cleanup
            let mut perms = tokio::fs::metadata(&readonly_dir)
                .await
                .unwrap()
                .permissions();
            perms.set_mode(0o755);
            tokio::fs::set_permissions(&readonly_dir, perms)
                .await
                .unwrap();
        }
    }

    #[test]
    fn test_device_id_display_format() {
        let bytes = [0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0];
        let mut full_bytes = [0u8; 32];
        full_bytes[..8].copy_from_slice(&bytes);

        let device_id = DeviceId::from_bytes(&full_bytes).unwrap();
        let display = format!("{}", device_id);
        let fingerprint = device_id.fingerprint();

        // Display and fingerprint should be the same
        assert_eq!(display, fingerprint);

        // Should contain colons
        assert!(display.contains(':'));
    }

    #[test]
    fn test_edge_case_device_names() {
        // Empty string device name
        let identity = DeviceIdentity::generate("").unwrap();
        assert_eq!(identity.device_name(), "");

        // Very long device name
        let long_name = "a".repeat(1000);
        let identity = DeviceIdentity::generate(&long_name).unwrap();
        assert_eq!(identity.device_name(), &long_name);
    }
}
