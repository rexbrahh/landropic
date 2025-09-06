#![doc = include_str!("../README.md")]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::uninlined_format_args)]

pub mod generated {
    #![allow(clippy::doc_markdown)]
    #![allow(clippy::must_use_candidate)]
    #![allow(clippy::missing_const_for_fn)]
    include!("generated/landro.proto.rs");
}

pub mod sync_handler;
pub mod validation;

pub use generated::*;
pub use sync_handler::{FolderSyncProgress, PeerInfo, SyncProtocolHandler, SyncState};

// Re-export commonly used types
pub use generated::{
    Ack, ChunkData, Error as ProtoError, FileEntry, FolderSummary, Hello, Manifest, Want,
};

// Protocol version for compatibility checking
pub const PROTOCOL_VERSION: &str = "0.1.0";
pub const PROTOCOL_VERSION_MAJOR: u32 = 0;
pub const PROTOCOL_VERSION_MINOR: u32 = 1;
pub const PROTOCOL_VERSION_PATCH: u32 = 0;

/// Version compatibility checking for protocol negotiation
pub struct VersionNegotiator;

impl VersionNegotiator {
    /// Check if a peer version is compatible with our version
    /// Returns true if versions are compatible (same major version)
    pub fn is_compatible(peer_version: &str) -> bool {
        match Self::parse_version(peer_version) {
            Some((peer_major, _peer_minor, _peer_patch)) => {
                // Compatible if same major version (semantic versioning rules)
                peer_major == PROTOCOL_VERSION_MAJOR
            }
            None => false, // Invalid version string
        }
    }

    /// Parse a version string like "0.1.0" into (major, minor, patch)
    fn parse_version(version: &str) -> Option<(u32, u32, u32)> {
        let parts: Vec<&str> = version.split('.').collect();
        if parts.len() != 3 {
            return None;
        }

        let major = parts[0].parse().ok()?;
        let minor = parts[1].parse().ok()?;
        let patch = parts[2].parse().ok()?;

        Some((major, minor, patch))
    }

    /// Get a user-friendly compatibility error message
    pub fn compatibility_error(peer_version: &str) -> String {
        match Self::parse_version(peer_version) {
            Some((peer_major, peer_minor, peer_patch)) => {
                format!(
                    "Protocol version incompatible: peer {}.{}.{}, we support {}.{}.{} (major version must match)",
                    peer_major, peer_minor, peer_patch,
                    PROTOCOL_VERSION_MAJOR, PROTOCOL_VERSION_MINOR, PROTOCOL_VERSION_PATCH
                )
            }
            None => {
                format!(
                    "Invalid protocol version format '{}', expected format like '{}'",
                    peer_version, PROTOCOL_VERSION
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_protocol_version() {
        assert_eq!(PROTOCOL_VERSION, "0.1.0");
    }

    #[test]
    fn test_version_compatibility() {
        // Same version should be compatible
        assert!(VersionNegotiator::is_compatible("0.1.0"));

        // Same major version but different minor should be compatible
        assert!(VersionNegotiator::is_compatible("0.2.0"));
        assert!(VersionNegotiator::is_compatible("0.1.5"));

        // Different major version should be incompatible
        assert!(!VersionNegotiator::is_compatible("1.0.0"));
        assert!(!VersionNegotiator::is_compatible("2.1.0"));

        // Invalid formats should be incompatible
        assert!(!VersionNegotiator::is_compatible("0.1"));
        assert!(!VersionNegotiator::is_compatible("0.1.0.1"));
        assert!(!VersionNegotiator::is_compatible("invalid"));
        assert!(!VersionNegotiator::is_compatible(""));
    }

    #[test]
    fn test_version_parsing() {
        assert_eq!(VersionNegotiator::parse_version("0.1.0"), Some((0, 1, 0)));
        assert_eq!(VersionNegotiator::parse_version("1.2.3"), Some((1, 2, 3)));
        assert_eq!(
            VersionNegotiator::parse_version("10.20.30"),
            Some((10, 20, 30))
        );

        // Invalid formats
        assert_eq!(VersionNegotiator::parse_version("0.1"), None);
        assert_eq!(VersionNegotiator::parse_version("0.1.0.1"), None);
        assert_eq!(VersionNegotiator::parse_version("invalid"), None);
        assert_eq!(VersionNegotiator::parse_version("1.a.0"), None);
    }

    #[test]
    fn test_compatibility_error_messages() {
        let error = VersionNegotiator::compatibility_error("1.0.0");
        assert!(error.contains("Protocol version incompatible"));
        assert!(error.contains("1.0.0"));
        assert!(error.contains("0.1.0"));

        let error = VersionNegotiator::compatibility_error("invalid");
        assert!(error.contains("Invalid protocol version format"));
        assert!(error.contains("invalid"));
    }
}
