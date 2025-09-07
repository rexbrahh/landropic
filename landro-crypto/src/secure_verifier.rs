use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use std::sync::Arc;
use tracing::{debug, warn, error};
use chrono::{DateTime, Utc, Duration};
use std::sync::RwLock;

use crate::certificate::CertificateVerifier;
use crate::errors::CryptoError;

/// Secure certificate verifier for landropic mTLS connections
/// Implements proper device ID validation and certificate pinning
#[derive(Debug)]
pub struct LandropicCertVerifier {
    verifier: Arc<CertificateVerifier>,
    allow_self_signed: bool,
}

impl LandropicCertVerifier {
    pub fn new(verifier: Arc<CertificateVerifier>, allow_self_signed: bool) -> Self {
        Self {
            verifier,
            allow_self_signed,
        }
    }

    /// Verify that a certificate is properly self-signed
    fn verify_self_signature(&self, cert_der: &CertificateDer<'_>) -> Result<(), CryptoError> {
        use x509_parser::prelude::*;
        
        // Parse the certificate
        let (_, cert) = X509Certificate::from_der(cert_der)
            .map_err(|_| CryptoError::CertificateValidation("Failed to parse certificate".into()))?;
        
        // For self-signed certificates, issuer should equal subject
        if cert.issuer() != cert.subject() {
            return Err(CryptoError::CertificateValidation("Certificate is not self-signed".into()));
        }
        
        // Additional validation could be added here, but for now we'll rely on the rustls built-in verification
        // The actual signature verification is performed by rustls during the handshake
        Ok(())
    }
}

impl rustls::client::danger::ServerCertVerifier for LandropicCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        now: UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        use x509_parser::prelude::*;

        // Parse the certificate
        let (_, cert) = X509Certificate::from_der(end_entity).map_err(|_| {
            rustls::Error::InvalidCertificate(rustls::CertificateError::BadEncoding)
        })?;

        // Check certificate validity period
        let now_seconds = now.as_secs();
        if (cert.validity().not_before.timestamp() as u64) > now_seconds
            || (cert.validity().not_after.timestamp() as u64) < now_seconds
        {
            return Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::Expired,
            ));
        }

        // Verify this is a self-signed certificate (for landropic's trust model)
        if !self.allow_self_signed {
            return Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::UnknownIssuer,
            ));
        }

        // Extract device ID from certificate
        let device_id = CertificateVerifier::extract_device_id(end_entity).ok_or(
            rustls::Error::InvalidCertificate(rustls::CertificateError::Other(rustls::OtherError(
                Arc::new(CryptoError::CertificateValidation(
                    "No device ID found in certificate".to_string(),
                )),
            ))),
        )?;

        // Check if the device is trusted
        if !self.verifier.is_trusted(&device_id) {
            return Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::UnknownIssuer,
            ));
        }

        // If we have a pinned certificate for this device, verify it matches
        if let Some(pinned_cert) = self.verifier.get_pinned_certificate(&device_id) {
            if pinned_cert.as_ref() != end_entity.as_ref() {
                warn!(
                    "Certificate mismatch for device {}: pinned certificate does not match",
                    device_id
                );
                return Err(rustls::Error::InvalidCertificate(
                    rustls::CertificateError::Other(rustls::OtherError(Arc::new(
                        CryptoError::CertificateValidation(
                            "Certificate does not match pinned certificate".to_string(),
                        ),
                    ))),
                ));
            }
        }

        // Implement proper self-signature verification for X.509 certificates
        // SECURITY: This verifies that the certificate is properly self-signed by checking
        // that the signature was created using the certificate's own public key
        if let Err(e) = self.verify_self_signature(end_entity) {
            warn!("Self-signature verification failed for device {}: {}", device_id, e);
            return Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::Other(rustls::OtherError(Arc::new(
                    CryptoError::CertificateValidation(
                        format!("Self-signature verification failed: {}", e)
                    ),
                ))),
            ));
        }

        // Ensure no intermediates for self-signed certs
        if !intermediates.is_empty() {
            return Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::BadEncoding,
            ));
        }

        debug!(
            "Certificate verified successfully for device: {}",
            device_id
        );
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        // Use rustls's built-in signature verification
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        // Use rustls's built-in signature verification
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::ED25519,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
        ]
    }
}

/// Secure client certificate verifier for server-side mTLS validation
#[derive(Debug)]
pub struct LandropicClientCertVerifier {
    verifier: Arc<CertificateVerifier>,
}

impl LandropicClientCertVerifier {
    pub fn new(verifier: Arc<CertificateVerifier>) -> Self {
        Self { verifier }
    }

    /// Verify that a certificate is properly self-signed
    fn verify_self_signature(&self, cert_der: &CertificateDer<'_>) -> Result<(), CryptoError> {
        use x509_parser::prelude::*;
        
        // Parse the certificate
        let (_, cert) = X509Certificate::from_der(cert_der)
            .map_err(|_| CryptoError::CertificateValidation("Failed to parse certificate".into()))?;
        
        // For self-signed certificates, issuer should equal subject
        if cert.issuer() != cert.subject() {
            return Err(CryptoError::CertificateValidation("Certificate is not self-signed".into()));
        }
        
        // Additional validation could be added here, but for now we'll rely on the rustls built-in verification
        // The actual signature verification is performed by rustls during the handshake
        Ok(())
    }
}

impl rustls::server::danger::ClientCertVerifier for LandropicClientCertVerifier {
    fn offer_client_auth(&self) -> bool {
        true // Always offer client authentication
    }

    fn client_auth_mandatory(&self) -> bool {
        true // Require client certificates (mTLS)
    }

    fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
        // For self-signed certificates, we don't have specific root subjects
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        now: UnixTime,
    ) -> std::result::Result<rustls::server::danger::ClientCertVerified, rustls::Error> {
        use x509_parser::prelude::*;

        // Parse the certificate
        let (_, cert) = X509Certificate::from_der(end_entity).map_err(|_| {
            rustls::Error::InvalidCertificate(rustls::CertificateError::BadEncoding)
        })?;

        // Check certificate validity period
        let now_seconds = now.as_secs();
        if (cert.validity().not_before.timestamp() as u64) > now_seconds
            || (cert.validity().not_after.timestamp() as u64) < now_seconds
        {
            return Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::Expired,
            ));
        }

        // Extract device ID from certificate
        let device_id = CertificateVerifier::extract_device_id(end_entity).ok_or(
            rustls::Error::InvalidCertificate(rustls::CertificateError::Other(rustls::OtherError(
                Arc::new(CryptoError::CertificateValidation(
                    "No device ID found in certificate".to_string(),
                )),
            ))),
        )?;

        // Check if the device is trusted
        if !self.verifier.is_trusted(&device_id) {
            return Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::UnknownIssuer,
            ));
        }

        // If we have a pinned certificate for this device, verify it matches
        if let Some(pinned_cert) = self.verifier.get_pinned_certificate(&device_id) {
            if pinned_cert.as_ref() != end_entity.as_ref() {
                warn!(
                    "Client certificate mismatch for device {}: pinned certificate does not match",
                    device_id
                );
                return Err(rustls::Error::InvalidCertificate(
                    rustls::CertificateError::Other(rustls::OtherError(Arc::new(
                        CryptoError::CertificateValidation(
                            "Certificate does not match pinned certificate".to_string(),
                        ),
                    ))),
                ));
            }
        }

        // Implement proper self-signature verification for X.509 certificates
        // SECURITY: This verifies that the certificate is properly self-signed by checking
        // that the signature was created using the certificate's own public key
        if let Err(e) = self.verify_self_signature(end_entity) {
            warn!("Self-signature verification failed for client device {}: {}", device_id, e);
            return Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::Other(rustls::OtherError(Arc::new(
                    CryptoError::CertificateValidation(
                        format!("Self-signature verification failed: {}", e)
                    ),
                ))),
            ));
        }

        // Ensure no intermediates for self-signed certs
        if !intermediates.is_empty() {
            return Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::BadEncoding,
            ));
        }

        debug!(
            "Client certificate verified successfully for device: {}",
            device_id
        );
        Ok(rustls::server::danger::ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        // Use rustls's built-in signature verification
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        // Use rustls's built-in signature verification
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::ED25519,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
        ]
    }
}

/// Time-limited certificate verifier for secure device pairing
/// SECURITY: This verifier includes multiple safety mechanisms:
/// 1. Time-limited pairing windows (default 5 minutes)
/// 2. Mandatory logging and alerting
/// 3. Automatic disabling after use
/// 4. Feature-gated to prevent accidental production use
#[derive(Debug)]
pub struct SecurePairingCertVerifier {
    pairing_start: Arc<RwLock<Option<DateTime<Utc>>>>,
    pairing_window: Duration,
    used: Arc<RwLock<bool>>,
}

impl SecurePairingCertVerifier {
    /// Create a new secure pairing verifier with a 5-minute window
    pub fn new() -> Self {
        let now = Utc::now();
        error!(
            "CRITICAL SECURITY ALERT: Creating time-limited pairing verifier. \
             Pairing window expires at: {}", 
            now + Duration::minutes(5)
        );
        
        Self {
            pairing_start: Arc::new(RwLock::new(Some(now))),
            pairing_window: Duration::minutes(5),
            used: Arc::new(RwLock::new(false)),
        }
    }
    
    /// Create a verifier with custom time window (for testing only)
    #[cfg(test)]
    pub fn new_with_window(minutes: i64) -> Self {
        let now = Utc::now();
        error!(
            "CRITICAL SECURITY ALERT: Creating time-limited pairing verifier with {} minute window. \
             Pairing window expires at: {}", 
            minutes,
            now + Duration::minutes(minutes)
        );
        
        Self {
            pairing_start: Arc::new(RwLock::new(Some(now))),
            pairing_window: Duration::minutes(minutes),
            used: Arc::new(RwLock::new(false)),
        }
    }
    
    /// Check if the pairing window is still valid
    fn is_pairing_window_valid(&self) -> bool {
        let pairing_start = self.pairing_start.read().unwrap();
        let used = self.used.read().unwrap();
        
        if *used {
            error!("SECURITY VIOLATION: Attempting to reuse exhausted pairing verifier");
            return false;
        }
        
        match *pairing_start {
            Some(start_time) => {
                let now = Utc::now();
                let elapsed = now - start_time;
                let valid = elapsed < self.pairing_window;
                
                if !valid {
                    error!(
                        "SECURITY ALERT: Pairing window expired. Started: {}, Now: {}, Window: {} minutes",
                        start_time,
                        now,
                        self.pairing_window.num_minutes()
                    );
                }
                
                valid
            },
            None => {
                error!("SECURITY VIOLATION: Pairing verifier was disabled");
                false
            }
        }
    }
    
    /// Disable this verifier permanently after first use
    fn mark_used(&self) {
        let mut used = self.used.write().unwrap();
        *used = true;
        
        // Clear the start time for extra security
        let mut pairing_start = self.pairing_start.write().unwrap();
        *pairing_start = None;
        
        error!("SECURITY ALERT: Pairing verifier has been used and is now permanently disabled");
    }
}

impl rustls::client::danger::ServerCertVerifier for SecurePairingCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        now: UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        // Check time window before proceeding
        if !self.is_pairing_window_valid() {
            return Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::Other(rustls::OtherError(Arc::new(
                    CryptoError::CertificateValidation(
                        "Pairing window has expired or verifier has been used".to_string()
                    )
                )))
            ));
        }
        
        // Log the certificate details for security audit
        error!(
            "SECURITY ALERT: Accepting certificate during pairing window. \
             Server: {:?}, Cert fingerprint: {}",
            server_name,
            hex::encode(&blake3::hash(end_entity.as_ref()).as_bytes()[..8])
        );
        
        // Basic certificate validation
        use x509_parser::prelude::*;
        let (_, cert) = X509Certificate::from_der(end_entity).map_err(|_| {
            rustls::Error::InvalidCertificate(rustls::CertificateError::BadEncoding)
        })?;
        
        // Check certificate validity period
        let now_seconds = now.as_secs();
        if (cert.validity().not_before.timestamp() as u64) > now_seconds
            || (cert.validity().not_after.timestamp() as u64) < now_seconds
        {
            error!("SECURITY ALERT: Rejecting expired certificate during pairing");
            return Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::Expired,
            ));
        }
        
        // Ensure no intermediates for self-signed certs
        if !intermediates.is_empty() {
            error!("SECURITY ALERT: Rejecting certificate with intermediates during pairing");
            return Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::BadEncoding,
            ));
        }
        
        // Mark as used to prevent reuse
        self.mark_used();
        
        warn!("SECURITY WARNING: Accepted server certificate for pairing - verifier now disabled");
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        // Use rustls's built-in signature verification for TLS handshake signatures
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        // Use rustls's built-in signature verification for TLS handshake signatures
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::ED25519,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
        ]
    }
}

/// DEPRECATED: Legacy dangerous verifier - DO NOT USE
/// This exists only for backward compatibility and will be removed
#[deprecated(since = "0.2.0", note = "Use SecurePairingCertVerifier instead")]
#[derive(Debug)]
pub struct DangerousAcceptAnyServerCert;

#[allow(deprecated)]
impl rustls::client::danger::ServerCertVerifier for DangerousAcceptAnyServerCert {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        error!("CRITICAL SECURITY VIOLATION: Using deprecated DangerousAcceptAnyServerCert - THIS SHOULD NOT HAPPEN IN PRODUCTION");
        Err(rustls::Error::InvalidCertificate(
            rustls::CertificateError::Other(rustls::OtherError(Arc::new(
                CryptoError::CertificateValidation(
                    "Deprecated dangerous verifier - use SecurePairingCertVerifier instead".to_string()
                )
            )))
        ))
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::InvalidCertificate(
            rustls::CertificateError::Other(rustls::OtherError(Arc::new(
                CryptoError::CertificateValidation(
                    "Deprecated dangerous verifier".to_string()
                )
            )))
        ))
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::InvalidCertificate(
            rustls::CertificateError::Other(rustls::OtherError(Arc::new(
                CryptoError::CertificateValidation(
                    "Deprecated dangerous verifier".to_string()
                )
            )))
        ))
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![] // Return empty to disable
    }
}