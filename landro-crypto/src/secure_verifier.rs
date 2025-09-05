use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use std::sync::Arc;
use tracing::{debug, warn};

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

        // TODO: Implement proper self-signature verification with x509-parser
        // For now, we rely on rustls's built-in TLS signature verification
        // Self-signature verification would require more complex X.509 parsing

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

        // TODO: Implement proper self-signature verification with x509-parser
        // For now, we rely on rustls's built-in TLS signature verification
        // Self-signature verification would require more complex X.509 parsing

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

/// Temporary certificate verifier that accepts any certificate
/// ONLY for initial device pairing - DO NOT USE in production
#[derive(Debug)]
pub struct DangerousAcceptAnyServerCert;

impl rustls::client::danger::ServerCertVerifier for DangerousAcceptAnyServerCert {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        warn!("SECURITY WARNING: Accepting any server certificate for pairing mode");
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
        ]
    }
}
