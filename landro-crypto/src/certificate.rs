use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, SanType};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use std::sync::Arc;
use tracing::{debug, warn};

use crate::errors::{CryptoError, Result};
use crate::identity::{DeviceIdentity, DeviceId};

/// Generate TLS certificates for mTLS connections
pub struct CertificateGenerator;

impl CertificateGenerator {
    /// Generate a self-signed certificate for a device
    pub fn generate_device_certificate(
        identity: &DeviceIdentity,
    ) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
        // Create certificate parameters
        let mut params = CertificateParams::new(vec![
            identity.device_name().to_string(),
        ]).map_err(|e| CryptoError::CertificateGeneration(e.to_string()))?;
        
        // Set certificate validity (5 years)
        params.not_before = time::OffsetDateTime::now_utc();
        params.not_after = params.not_before + time::Duration::days(365 * 5);
        
        // Add device ID as a custom extension
        let device_id = identity.device_id();
        params.distinguished_name = DistinguishedName::new();
        params.distinguished_name.push(
            DnType::CommonName,
            identity.device_name(),
        );
        params.distinguished_name.push(
            DnType::OrganizationName,
            "Landropic",
        );
        
        // Add SANs for local connections
        params.subject_alt_names = vec![
            SanType::DnsName(identity.device_name().try_into()
                .map_err(|e| CryptoError::CertificateGeneration(format!("Invalid DNS name: {:?}", e)))?),
            SanType::IpAddress(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)),
            SanType::IpAddress(std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)),
        ];
        
        // Include device ID in certificate for identification
        // We'll encode it as a custom extension or in the CN
        params.custom_extensions = vec![
            rcgen::CustomExtension::from_oid_content(
                &[2, 5, 29, 17], // Subject Alternative Name OID
                device_id.as_bytes().to_vec(),
            ),
        ];
        
        // Generate the certificate
        let key_pair = KeyPair::generate()
            .map_err(|e| CryptoError::CertificateGeneration(e.to_string()))?;
        let cert = params.self_signed(&key_pair)
            .map_err(|e| CryptoError::CertificateGeneration(e.to_string()))?;
        let cert_der = CertificateDer::from(cert.der().to_vec());
        let key_der = PrivateKeyDer::try_from(key_pair.serialize_der())
            .map_err(|e| CryptoError::CertificateGeneration(format!("Failed to serialize key: {:?}", e)))?;
        
        debug!("Generated certificate for device: {}", identity.device_name());
        
        Ok((vec![cert_der], key_der))
    }
    
    /// Generate ephemeral session certificate for QUIC connection
    pub fn generate_ephemeral_certificate(
        hostname: &str,
    ) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
        let mut params = CertificateParams::new(vec![hostname.to_string()])
            .map_err(|e| CryptoError::CertificateGeneration(e.to_string()))?;
        
        // Short validity for ephemeral certs (7 days)
        params.not_before = time::OffsetDateTime::now_utc();
        params.not_after = params.not_before + time::Duration::days(7);
        
        params.distinguished_name = DistinguishedName::new();
        params.distinguished_name.push(
            DnType::CommonName,
            hostname,
        );
        
        // Add SANs
        params.subject_alt_names = vec![
            SanType::DnsName(hostname.try_into()
                .map_err(|e| CryptoError::CertificateGeneration(format!("Invalid DNS name: {:?}", e)))?),
        ];
        
        // Add IP SANs if hostname is an IP
        if let Ok(ip) = hostname.parse::<std::net::IpAddr>() {
            params.subject_alt_names.push(SanType::IpAddress(ip));
        }
        
        let key_pair = KeyPair::generate()
            .map_err(|e| CryptoError::CertificateGeneration(e.to_string()))?;
        let cert = params.self_signed(&key_pair)
            .map_err(|e| CryptoError::CertificateGeneration(e.to_string()))?;
        let cert_der = CertificateDer::from(cert.der().to_vec());
        let key_der = PrivateKeyDer::try_from(key_pair.serialize_der())
            .map_err(|e| CryptoError::CertificateGeneration(format!("Failed to serialize key: {:?}", e)))?;
        
        Ok((vec![cert_der], key_der))
    }
}

/// Certificate verification for mTLS
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
        if let Some(pos) = self.trusted_device_ids.iter().position(|id| id == device_id) {
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
    pub fn extract_device_id(_cert: &CertificateDer) -> Option<DeviceId> {
        // In a real implementation, we'd parse the certificate and extract
        // the device ID from the custom extension we added
        // For now, this is a placeholder
        warn!("Certificate device ID extraction not yet implemented");
        None
    }
}

/// TLS configuration builder for Landropic
pub struct TlsConfig;

impl TlsConfig {
    /// Create server TLS configuration
    pub fn server_config(
        cert_chain: Vec<CertificateDer<'static>>,
        private_key: PrivateKeyDer<'static>,
        _verifier: Arc<CertificateVerifier>,
    ) -> Result<rustls::ServerConfig> {
        let config = rustls::ServerConfig::builder()
            .with_no_client_auth() // We'll implement custom verification
            .with_single_cert(cert_chain, private_key)
            .map_err(|e| CryptoError::CertificateGeneration(e.to_string()))?;
        
        Ok(config)
    }
    
    /// Create client TLS configuration
    pub fn client_config(
        cert_chain: Vec<CertificateDer<'static>>,
        private_key: PrivateKeyDer<'static>,
        _verifier: Arc<CertificateVerifier>,
    ) -> Result<rustls::ClientConfig> {
        let config = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(DangerousAcceptAnyServerCert))
            .with_client_auth_cert(cert_chain, private_key)
            .map_err(|e| CryptoError::CertificateGeneration(e.to_string()))?;
        
        Ok(config)
    }
}

/// Temporary certificate verifier that accepts any certificate
/// TODO: Implement proper verification with device ID checking
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
        warn!("Accepting any server certificate - proper verification not yet implemented");
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
        let (cert_chain, private_key) = 
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
}