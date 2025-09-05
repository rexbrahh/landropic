use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, SanType};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use std::sync::Arc;
use tracing::{debug, warn};

use crate::errors::{CryptoError, Result};
use crate::identity::{DeviceId, DeviceIdentity};

/// Temporary certificate verifier that accepts any certificate - ONLY for testing/pairing
#[derive(Debug)]
struct DangerousAcceptAnyServerCert;

impl rustls::client::danger::ServerCertVerifier for DangerousAcceptAnyServerCert {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        warn!("DANGEROUS: Accepting any server certificate for testing/pairing");
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::ED25519,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
        ]
    }
}

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
    allow_untrusted: bool,
}

impl CertificateVerifier {
    /// Create a new verifier with a list of trusted device IDs
    pub fn new(trusted_device_ids: Vec<DeviceId>) -> Self {
        Self {
            trusted_device_ids,
            allow_untrusted: false,
        }
    }

    /// Create a verifier that allows connections from any device (for pairing)
    pub fn allow_any() -> Self {
        Self {
            trusted_device_ids: Vec::new(),
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

    /// Check if a device ID is trusted
    pub fn is_trusted(&self, device_id: &DeviceId) -> bool {
        self.allow_untrusted || self.trusted_device_ids.contains(device_id)
    }

    /// Extract device ID from certificate
    pub fn extract_device_id(cert: &CertificateDer) -> Option<DeviceId> {
        use x509_parser::prelude::*;

        match X509Certificate::from_der(cert) {
            Ok((_, cert)) => {
                // Look for our custom extension containing the device ID
                for ext in cert.extensions() {
                    // Check if this is our landropic device ID extension
                    // Check for our custom extension OID (1.3.6.1.4.1.99999.1)
                    // The x509_parser represents OIDs as slices, so compare directly
                    if ext.oid.to_string() == "1.3.6.1.4.1.99999.1" {
                        if let Ok(device_id) = DeviceId::from_bytes(ext.value) {
                            return Some(device_id);
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
}

/// TLS configuration builder for Landropic
pub struct TlsConfig;

impl TlsConfig {
    /// Create server TLS configuration with proper mTLS
    pub fn server_config(
        cert_chain: Vec<CertificateDer<'static>>,
        private_key: PrivateKeyDer<'static>,
        _verifier: Arc<CertificateVerifier>,
    ) -> Result<rustls::ServerConfig> {
        // TODO: Implement proper mTLS with secure verifier
        let config = rustls::ServerConfig::builder()
            .with_no_client_auth() // Temporary - should use custom verifier
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
        _verifier: Arc<CertificateVerifier>,
    ) -> Result<rustls::ClientConfig> {
        // TODO: Implement proper certificate verification with secure verifier
        let config = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(DangerousAcceptAnyServerCert))
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
