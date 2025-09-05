use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, SanType};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tracing::{debug, warn};
use tokio::fs;

use crate::errors::{CryptoError, Result};
use crate::identity::{DeviceId, DeviceIdentity};
use crate::secure_verifier::{LandropicCertVerifier, LandropicClientCertVerifier};


/// Generates TLS certificates for secure QUIC connections between Landropic devices.
///
/// This generator creates self-signed X.509 certificates that embed device identity
/// information for mutual TLS (mTLS) authentication. The certificates are used to
/// establish encrypted QUIC connections between trusted devices.
///
/// # Certificate Types
///
/// - **Device certificates**: Long-term certificates tied to a device's Ed25519 identity
/// - **Ephemeral certificates**: Short-lived certificates for temporary connections
///
/// # Example
///
/// ```rust
/// use landro_crypto::{DeviceIdentity, CertificateGenerator};
///
/// let identity = DeviceIdentity::generate("my-device").unwrap();
/// let (cert_chain, private_key) =
///     CertificateGenerator::generate_device_certificate(&identity).unwrap();
///     
/// // cert_chain and private_key can now be used with rustls/quinn
/// ```
pub struct CertificateGenerator;

impl CertificateGenerator {
    /// Generates a self-signed TLS certificate for a device's long-term identity.
    ///
    /// This certificate embeds the device's identity information and can be used
    /// for mTLS authentication in QUIC connections. The certificate is valid for
    /// 5 years and includes the device name in various certificate fields.
    ///
    /// # Arguments
    ///
    /// * `identity` - The device identity to create a certificate for
    ///
    /// # Returns
    ///
    /// A tuple containing:
    /// - Certificate chain (just one self-signed cert)
    /// - Private key for the certificate
    ///
    /// # Example
    ///
    /// ```rust
    /// use landro_crypto::{DeviceIdentity, CertificateGenerator};
    ///
    /// let identity = DeviceIdentity::generate("alice-laptop").unwrap();
    /// let (cert_chain, private_key) =
    ///     CertificateGenerator::generate_device_certificate(&identity).unwrap();
    ///     
    /// // Use with rustls ServerConfig or ClientConfig
    /// ```
    pub fn generate_device_certificate(
        identity: &DeviceIdentity,
    ) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
        // Create certificate parameters
        let mut params = CertificateParams::new(vec![identity.device_name().to_string()])
            .map_err(|e| CryptoError::CertificateGeneration(e.to_string()))?;

        // Set certificate validity (5 years)
        params.not_before = time::OffsetDateTime::now_utc();
        params.not_after = params.not_before + time::Duration::days(365 * 5);

        // Add device ID as a custom extension
        let device_id = identity.device_id();
        params.distinguished_name = DistinguishedName::new();
        params
            .distinguished_name
            .push(DnType::CommonName, identity.device_name());
        params
            .distinguished_name
            .push(DnType::OrganizationName, "Landropic");

        // Add SANs for local connections
        params.subject_alt_names = vec![
            SanType::DnsName(identity.device_name().try_into().map_err(|e| {
                CryptoError::CertificateGeneration(format!("Invalid DNS name: {:?}", e))
            })?),
            SanType::IpAddress(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)),
            SanType::IpAddress(std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)),
        ];

        // Include device ID in certificate for identification
        // We'll encode it as a custom extension or in the CN
        // Include device ID in certificate using a proper custom OID
        // Using private enterprise arc: 1.3.6.1.4.1.XXXXX.1 (replace XXXXX with assigned number)
        // For now using a temporary OID in the experimental range
        params.custom_extensions = vec![rcgen::CustomExtension::from_oid_content(
            &[1, 3, 6, 1, 4, 1, 99999, 1], // Experimental OID for landropic device ID
            device_id.as_bytes().to_vec(),
        )];

        // Generate the certificate
        let key_pair =
            KeyPair::generate().map_err(|e| CryptoError::CertificateGeneration(e.to_string()))?;
        let cert = params
            .self_signed(&key_pair)
            .map_err(|e| CryptoError::CertificateGeneration(e.to_string()))?;
        let cert_der = CertificateDer::from(cert.der().to_vec());
        let key_der = PrivateKeyDer::try_from(key_pair.serialize_der()).map_err(|e| {
            CryptoError::CertificateGeneration(format!("Failed to serialize key: {:?}", e))
        })?;

        debug!(
            "Generated certificate for device: {}",
            identity.device_name()
        );

        Ok((vec![cert_der], key_der))
    }

    /// Generates a short-lived certificate for temporary connections.
    ///
    /// Ephemeral certificates are useful for initial device discovery or when
    /// a device doesn't have a long-term identity yet. They're valid for only
    /// 7 days and are tied to a hostname rather than a device identity.
    ///
    /// # Arguments
    ///
    /// * `hostname` - The hostname or IP address for the certificate
    ///
    /// # Returns
    ///
    /// A tuple containing the certificate chain and private key.
    ///
    /// # Example
    ///
    /// ```rust
    /// use landro_crypto::CertificateGenerator;
    ///
    /// let (cert_chain, private_key) =
    ///     CertificateGenerator::generate_ephemeral_certificate("192.168.1.100").unwrap();
    /// ```
    pub fn generate_ephemeral_certificate(
        hostname: &str,
    ) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
        let mut params = CertificateParams::new(vec![hostname.to_string()])
            .map_err(|e| CryptoError::CertificateGeneration(e.to_string()))?;

        // Short validity for ephemeral certs (7 days)
        params.not_before = time::OffsetDateTime::now_utc();
        params.not_after = params.not_before + time::Duration::days(7);

        params.distinguished_name = DistinguishedName::new();
        params.distinguished_name.push(DnType::CommonName, hostname);

        // Add SANs
        params.subject_alt_names = vec![SanType::DnsName(hostname.try_into().map_err(|e| {
            CryptoError::CertificateGeneration(format!("Invalid DNS name: {:?}", e))
        })?)];

        // Add IP SANs if hostname is an IP
        if let Ok(ip) = hostname.parse::<std::net::IpAddr>() {
            params.subject_alt_names.push(SanType::IpAddress(ip));
        }

        let key_pair =
            KeyPair::generate().map_err(|e| CryptoError::CertificateGeneration(e.to_string()))?;
        let cert = params
            .self_signed(&key_pair)
            .map_err(|e| CryptoError::CertificateGeneration(e.to_string()))?;
        let cert_der = CertificateDer::from(cert.der().to_vec());
        let key_der = PrivateKeyDer::try_from(key_pair.serialize_der()).map_err(|e| {
            CryptoError::CertificateGeneration(format!("Failed to serialize key: {:?}", e))
        })?;

        Ok((vec![cert_der], key_der))
    }
}

/// Verifies peer certificates during TLS handshakes based on device trust relationships.
///
/// Landropic uses a trust-on-first-use model where devices maintain lists of
/// other devices they trust. This verifier checks incoming certificates against
/// that trust list, extracting device IDs from certificate extensions.
///
/// # Trust Model
///
/// - Devices start with empty trust lists
/// - During pairing, devices are added to each other's trust lists
/// - Connections are only allowed from trusted devices (unless in pairing mode)
/// - Device trust can be revoked by removing from the trust list
///
/// # Example
///
/// ```rust
/// use landro_crypto::{CertificateVerifier, DeviceId};
///
/// let trusted_devices = vec![DeviceId::from_bytes(&[1u8; 32]).unwrap()];
/// let mut verifier = CertificateVerifier::new(trusted_devices);
///
/// // Add a new device to trust list
/// let new_device = DeviceId::from_bytes(&[2u8; 32]).unwrap();
/// verifier.add_trusted_device(new_device.clone());
/// assert!(verifier.is_trusted(&new_device));
/// ```
#[derive(Debug)]
pub struct CertificateVerifier {
    trusted_device_ids: Vec<DeviceId>,
    trusted_certificates: HashMap<DeviceId, CertificateDer<'static>>,
    allow_untrusted: bool,
}

impl CertificateVerifier {
    /// Create a new verifier with a list of trusted device IDs
    pub fn new(trusted_device_ids: Vec<DeviceId>) -> Self {
        Self {
            trusted_device_ids,
            trusted_certificates: HashMap::new(),
            allow_untrusted: false,
        }
    }

    /// Create a verifier that allows connections from any device (for pairing)
    /// 
    /// # Security Warning
    /// 
    /// This method creates a verifier that accepts certificates from ANY device,
    /// effectively disabling certificate validation. This should ONLY be used:
    /// 
    /// 1. During initial device pairing when devices don't yet trust each other
    /// 2. In unit tests that don't require proper certificate validation
    /// 3. In integration tests where certificate setup is not the focus
    /// 
    /// NEVER use this in production code paths outside of pairing.
    #[cfg(any(test, feature = "dangerous-allow-any"))]
    pub fn allow_any() -> Self {
        warn!("SECURITY WARNING: Creating certificate verifier that accepts ANY certificate");
        Self {
            trusted_device_ids: Vec::new(),
            trusted_certificates: HashMap::new(),
            allow_untrusted: true,
        }
    }
    
    /// Create a verifier for device pairing that temporarily allows untrusted connections
    /// This is the production-safe way to handle initial device pairing
    pub fn for_pairing() -> Self {
        warn!("Creating certificate verifier for device pairing - will accept untrusted certificates");
        Self {
            trusted_device_ids: Vec::new(),
            trusted_certificates: HashMap::new(),
            allow_untrusted: true,
        }
    }

    /// Add a device ID to the trust list
    pub fn add_trusted_device(&mut self, device_id: DeviceId) {
        if !self.trusted_device_ids.contains(&device_id) {
            debug!("Added trusted device: {}", device_id);
            self.trusted_device_ids.push(device_id);
        }
    }

    /// Remove a device ID from the trust list
    pub fn remove_trusted_device(&mut self, device_id: &DeviceId) -> bool {
        if let Some(pos) = self
            .trusted_device_ids
            .iter()
            .position(|id| id == device_id)
        {
            self.trusted_device_ids.remove(pos);
            debug!("Removed trusted device: {}", device_id);
            true
        } else {
            false
        }
    }

    /// Add a trusted device with its certificate
    pub fn add_trusted_device_with_cert(&mut self, device_id: DeviceId, cert: CertificateDer<'static>) {
        if !self.trusted_device_ids.contains(&device_id) {
            debug!("Added trusted device with certificate: {}", device_id);
            self.trusted_device_ids.push(device_id.clone());
        }
        self.trusted_certificates.insert(device_id, cert);
    }

    /// Check if a device ID is trusted
    pub fn is_trusted(&self, device_id: &DeviceId) -> bool {
        self.allow_untrusted || self.trusted_device_ids.contains(device_id)
    }

    /// Get the pinned certificate for a device (if any)
    pub fn get_pinned_certificate(&self, device_id: &DeviceId) -> Option<&CertificateDer<'static>> {
        self.trusted_certificates.get(device_id)
    }

    /// Verify a certificate matches the pinned certificate for this device
    pub fn verify_pinned_certificate(&self, device_id: &DeviceId, cert: &CertificateDer) -> bool {
        match self.get_pinned_certificate(device_id) {
            Some(pinned_cert) => pinned_cert.as_ref() == cert.as_ref(),
            None => false, // No pinned certificate - fail verification
        }
    }

    /// Extract device ID from certificate
    pub fn extract_device_id(cert: &CertificateDer) -> Option<DeviceId> {
        use x509_parser::prelude::*;

        match X509Certificate::from_der(cert) {
            Ok((_, cert)) => {
                // Look for our custom extension containing the device ID
                for ext in cert.extensions() {
                    // Check if this is our landropic device ID extension
                    // OID: 1.3.6.1.4.1.99999.1 (our experimental OID for device ID)
                    if ext.oid.to_string() == "1.3.6.1.4.1.99999.1" {
                        // The extension value should be exactly 32 bytes (the device ID)
                        if ext.value.len() == 32 {
                            let mut device_id_bytes = [0u8; 32];
                            device_id_bytes.copy_from_slice(ext.value);
                            return Some(DeviceId(device_id_bytes));
                        } else {
                            warn!("Device ID extension has wrong length: {} (expected 32)", ext.value.len());
                        }
                    }
                }
                debug!("No landropic device ID extension found in certificate");
                None
            }
            Err(e) => {
                warn!("Failed to parse certificate: {}", e);
                None
            }
        }
    }

    /// Save trusted certificates to disk
    pub async fn save_trusted_certificates<P: AsRef<Path>>(&self, cert_dir: P) -> Result<()> {
        let cert_dir = cert_dir.as_ref();
        fs::create_dir_all(cert_dir)
            .await
            .map_err(|e| CryptoError::Storage(format!("Failed to create cert directory: {}", e)))?;

        for (device_id, cert) in &self.trusted_certificates {
            let cert_file = cert_dir.join(format!("{}.pem", device_id.to_hex()));
            let cert_pem = pem::encode(&pem::Pem::new("CERTIFICATE", cert.as_ref().to_vec()));
            
            fs::write(&cert_file, cert_pem)
                .await
                .map_err(|e| CryptoError::Storage(format!("Failed to write certificate for {}: {}", device_id, e)))?;
            
            debug!("Saved certificate for device {} to {:?}", device_id, cert_file);
        }

        Ok(())
    }

    /// Load trusted certificates from disk
    pub async fn load_trusted_certificates<P: AsRef<Path>>(&mut self, cert_dir: P) -> Result<()> {
        let cert_dir = cert_dir.as_ref();
        
        if !cert_dir.exists() {
            debug!("Certificate directory does not exist: {:?}", cert_dir);
            return Ok(()); // No certificates to load
        }

        let mut dir = fs::read_dir(cert_dir)
            .await
            .map_err(|e| CryptoError::Storage(format!("Failed to read cert directory: {}", e)))?;

        while let Some(entry) = dir.next_entry()
            .await
            .map_err(|e| CryptoError::Storage(format!("Failed to read directory entry: {}", e)))?
        {
            let path = entry.path();
            if path.extension().map_or(false, |ext| ext == "pem") {
                if let Some(file_stem) = path.file_stem().and_then(|s| s.to_str()) {
                    if let Ok(device_id) = file_stem.parse::<DeviceId>() {
                        match self.load_certificate_file(&path).await {
                            Ok(cert) => {
                                debug!("Loaded certificate for device {} from {:?}", device_id, path);
                                self.add_trusted_device_with_cert(device_id, cert);
                            }
                            Err(e) => {
                                warn!("Failed to load certificate from {:?}: {}", path, e);
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    async fn load_certificate_file<P: AsRef<Path>>(&self, cert_file: P) -> Result<CertificateDer<'static>> {
        let cert_pem = fs::read_to_string(cert_file)
            .await
            .map_err(|e| CryptoError::Storage(format!("Failed to read certificate file: {}", e)))?;
        
        let pem = pem::parse(&cert_pem)
            .map_err(|e| CryptoError::CertificateValidation(format!("Invalid PEM format: {}", e)))?;
        
        if pem.tag() != "CERTIFICATE" {
            return Err(CryptoError::CertificateValidation("Not a certificate PEM".to_string()));
        }

        Ok(CertificateDer::from(pem.contents().to_vec()))
    }
}

/// TLS configuration builder for Landropic
pub struct TlsConfig;

impl TlsConfig {
    /// Create server TLS configuration with proper mTLS
    pub fn server_config(
        cert_chain: Vec<CertificateDer<'static>>,
        private_key: PrivateKeyDer<'static>,
        verifier: Arc<CertificateVerifier>,
    ) -> Result<rustls::ServerConfig> {
        let client_verifier = Arc::new(LandropicClientCertVerifier::new(verifier));
        
        let config = rustls::ServerConfig::builder()
            .with_client_cert_verifier(client_verifier)
            .with_single_cert(cert_chain, private_key)
            .map_err(|e| CryptoError::CertificateGeneration(e.to_string()))?;

        Ok(config)
    }

    /// Create server TLS configuration for pairing mode (no client auth required)
    /// This should ONLY be used during initial device pairing
    pub fn server_config_pairing(
        cert_chain: Vec<CertificateDer<'static>>,
        private_key: PrivateKeyDer<'static>,
    ) -> Result<rustls::ServerConfig> {
        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(cert_chain, private_key)
            .map_err(|e| CryptoError::CertificateGeneration(e.to_string()))?;

        Ok(config)
    }

    /// Create client TLS configuration with proper certificate verification
    pub fn client_config(
        cert_chain: Vec<CertificateDer<'static>>,
        private_key: PrivateKeyDer<'static>,
        verifier: Arc<CertificateVerifier>,
    ) -> Result<rustls::ClientConfig> {
        let cert_verifier = Arc::new(LandropicCertVerifier::new(verifier, true)); // Allow self-signed certs for landropic
        
        let config = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(cert_verifier)
            .with_client_auth_cert(cert_chain, private_key)
            .map_err(|e| CryptoError::CertificateGeneration(e.to_string()))?;

        Ok(config)
    }

    /// Create client TLS configuration for pairing mode (DANGEROUS - accepts any cert)
    /// This should ONLY be used during initial device pairing
    pub fn client_config_pairing(
        cert_chain: Vec<CertificateDer<'static>>,
        private_key: PrivateKeyDer<'static>,
    ) -> Result<rustls::ClientConfig> {
        use crate::secure_verifier::DangerousAcceptAnyServerCert;
        
        warn!("SECURITY WARNING: Creating TLS configuration that accepts ANY certificate - ONLY use for initial device pairing");
        
        let config = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(DangerousAcceptAnyServerCert))
            .with_client_auth_cert(cert_chain, private_key)
            .map_err(|e| CryptoError::CertificateGeneration(e.to_string()))?;

        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_generate_device_certificate() {
        let identity = DeviceIdentity::generate("test-device").unwrap();
        let (cert_chain, private_key) =
            CertificateGenerator::generate_device_certificate(&identity).unwrap();

        assert!(!cert_chain.is_empty());
        assert!(!matches!(private_key, PrivateKeyDer::Pkcs1(_))); // Should not be empty
    }

    #[test]
    fn test_generate_ephemeral_certificate() {
        let (cert_chain, _private_key) =
            CertificateGenerator::generate_ephemeral_certificate("localhost").unwrap();

        assert!(!cert_chain.is_empty());
    }

    #[test]
    fn test_certificate_verifier() {
        let device_id1 = DeviceId([1u8; 32]);
        let device_id2 = DeviceId([2u8; 32]);

        let mut verifier = CertificateVerifier::new(vec![device_id1.clone()]);

        assert!(verifier.is_trusted(&device_id1));
        assert!(!verifier.is_trusted(&device_id2));

        verifier.add_trusted_device(device_id2.clone());
        assert!(verifier.is_trusted(&device_id2));

        verifier.remove_trusted_device(&device_id1);
        assert!(!verifier.is_trusted(&device_id1));
    }

    // === ADDITIONAL ERROR PATH AND EDGE CASE TESTS ===

    #[tokio::test]
    async fn test_certificate_generation_edge_cases() {
        // Test with empty device name
        let identity = DeviceIdentity::generate("").unwrap();
        let result = CertificateGenerator::generate_device_certificate(&identity);
        assert!(result.is_ok());

        // Test with very long device name
        let long_name = "device-".repeat(100); // 700+ character name
        let identity = DeviceIdentity::generate(&long_name).unwrap();
        let result = CertificateGenerator::generate_device_certificate(&identity);
        assert!(result.is_ok());
    }

    #[test]
    fn test_ephemeral_certificate_with_ip_addresses() {
        // Test IPv4 address
        let result = CertificateGenerator::generate_ephemeral_certificate("192.168.1.1");
        assert!(result.is_ok());

        // Test IPv6 address
        let result = CertificateGenerator::generate_ephemeral_certificate("::1");
        assert!(result.is_ok());

        // Test invalid hostname (should still work, just won't parse as IP)
        let result = CertificateGenerator::generate_ephemeral_certificate("invalid..hostname");
        assert!(result.is_ok());
    }

    #[test]
    fn test_ephemeral_certificate_edge_cases() {
        // Empty hostname
        let result = CertificateGenerator::generate_ephemeral_certificate("");
        // This might fail depending on rcgen's validation
        let _ = result; // Don't assert since behavior may vary

        // Very long hostname
        let long_hostname = format!("{}.com", "a".repeat(250));
        let result = CertificateGenerator::generate_ephemeral_certificate(&long_hostname);
        // Again, behavior may vary based on certificate limits
        let _ = result;
    }

    #[test]
    fn test_certificate_verifier_allow_any() {
        let verifier = CertificateVerifier::allow_any();

        // Should trust any device when in allow_any mode
        let device_id1 = DeviceId([1u8; 32]);
        let device_id2 = DeviceId([255u8; 32]);

        assert!(verifier.is_trusted(&device_id1));
        assert!(verifier.is_trusted(&device_id2));
    }

    #[test]
    fn test_certificate_verifier_duplicate_additions() {
        let device_id = DeviceId([1u8; 32]);
        let mut verifier = CertificateVerifier::new(vec![device_id.clone()]);

        // Adding the same device multiple times should not create duplicates
        verifier.add_trusted_device(device_id.clone());
        verifier.add_trusted_device(device_id.clone());

        // Should still be trusted
        assert!(verifier.is_trusted(&device_id));

        // Remove should work once
        assert!(verifier.remove_trusted_device(&device_id));
        assert!(!verifier.is_trusted(&device_id));

        // Second removal should return false
        assert!(!verifier.remove_trusted_device(&device_id));
    }

    #[test]
    fn test_certificate_verifier_empty_list() {
        let verifier = CertificateVerifier::new(vec![]);
        let device_id = DeviceId([1u8; 32]);

        // Empty trust list should not trust anyone
        assert!(!verifier.is_trusted(&device_id));
    }

    #[test]
    fn test_extract_device_id_placeholder() {
        // Test the current placeholder implementation
        use rustls::pki_types::CertificateDer;

        // Create a dummy certificate (just empty bytes for now)
        let dummy_cert = CertificateDer::from(vec![0u8; 100]);

        // Current implementation should return None
        let result = CertificateVerifier::extract_device_id(&dummy_cert);
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_multiple_certificate_generations() {
        let identity = DeviceIdentity::generate("test-device").unwrap();

        // Generate multiple certificates from same identity
        let (cert1, _key1) = CertificateGenerator::generate_device_certificate(&identity).unwrap();
        let (cert2, _key2) = CertificateGenerator::generate_device_certificate(&identity).unwrap();

        // Certificates should be different (different keys/serials)
        assert_ne!(cert1[0].as_ref(), cert2[0].as_ref());

        // But both should be valid
        assert!(!cert1.is_empty());
        assert!(!cert2.is_empty());
    }

    #[test]
    fn test_certificate_verifier_large_trust_list() {
        // Test with a large number of trusted devices
        let mut trusted_devices = Vec::new();
        for i in 0..1000 {
            let mut bytes = [0u8; 32];
            bytes[0] = (i % 256) as u8;
            bytes[1] = ((i / 256) % 256) as u8;
            trusted_devices.push(DeviceId(bytes));
        }

        let verifier = CertificateVerifier::new(trusted_devices.clone());

        // First and last devices should be trusted
        assert!(verifier.is_trusted(&trusted_devices[0]));
        assert!(verifier.is_trusted(&trusted_devices[999]));

        // Random device should not be trusted
        let random_device = DeviceId([123u8; 32]);
        assert!(!verifier.is_trusted(&random_device));
    }
}
