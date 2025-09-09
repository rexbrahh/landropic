use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::errors::{QuicError, Result};
use landro_crypto::DeviceIdentity;

/// Simplified pairing state for v1.0
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairingState {
    /// Waiting for passphrase
    WaitingForPassphrase,
    /// Pairing completed
    Completed,
    /// Pairing failed
    Failed(String),
}

/// Simplified pairing context for v1.0 - basic passphrase verification
pub struct PairingContext {
    state: Arc<RwLock<PairingState>>,
    expected_passphrase: Option<String>,
    device_identity: Option<DeviceIdentity>,
}

impl PairingContext {
    /// Create new pairing context
    pub fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(PairingState::WaitingForPassphrase)),
            expected_passphrase: None,
            device_identity: None,
        }
    }

    /// Set expected passphrase for pairing
    pub async fn set_passphrase(&mut self, passphrase: String) {
        self.expected_passphrase = Some(passphrase);
    }

    /// Verify passphrase from peer
    pub async fn verify_passphrase(&mut self, received_passphrase: &str) -> Result<bool> {
        match &self.expected_passphrase {
            Some(expected) => {
                if expected == received_passphrase {
                    let mut state = self.state.write().await;
                    *state = PairingState::Completed;
                    Ok(true)
                } else {
                    let mut state = self.state.write().await;
                    *state = PairingState::Failed("Passphrase mismatch".to_string());
                    Ok(false)
                }
            }
            None => {
                let mut state = self.state.write().await;
                *state = PairingState::Failed("No passphrase set".to_string());
                Ok(false)
            }
        }
    }

    /// Get current state
    pub async fn state(&self) -> PairingState {
        self.state.read().await.clone()
    }

    /// Check if pairing is complete
    pub async fn is_complete(&self) -> bool {
        matches!(*self.state.read().await, PairingState::Completed)
    }
}

/// Simple pairing manager for basic passphrase-based pairing
pub struct PairingManager {
    contexts: Arc<RwLock<HashMap<String, PairingContext>>>,
}

impl PairingManager {
    pub fn new() -> Self {
        Self {
            contexts: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Start pairing with a device using passphrase
    pub async fn start_pairing(&self, device_id: String, passphrase: String) -> Result<()> {
        let mut context = PairingContext::new();
        context.set_passphrase(passphrase).await;

        let mut contexts = self.contexts.write().await;
        contexts.insert(device_id, context);

        Ok(())
    }

    /// Verify pairing attempt
    pub async fn verify_pairing(&self, device_id: &str, passphrase: &str) -> Result<bool> {
        let mut contexts = self.contexts.write().await;

        match contexts.get_mut(device_id) {
            Some(context) => context.verify_passphrase(passphrase).await,
            None => Err(QuicError::pairing_failed("No pairing context found")),
        }
    }

    /// Check if device is paired
    pub async fn is_paired(&self, device_id: &str) -> bool {
        let contexts = self.contexts.read().await;
        match contexts.get(device_id) {
            Some(context) => context.is_complete().await,
            None => false,
        }
    }
}

impl Default for PairingManager {
    fn default() -> Self {
        Self::new()
    }
}
