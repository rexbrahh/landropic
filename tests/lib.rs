//! Integration tests for Landropic
//! 
//! This module contains comprehensive end-to-end integration tests that verify
//! the full functionality of the Landropic file synchronization system.

pub mod common;

// Integration test modules
pub mod integration {
    pub mod cli_daemon_test;
    pub mod sync_test;
    pub mod network_test;
    pub mod storage_test;
}

// Re-export common utilities for use in tests
pub use common::*;

#[cfg(test)]
mod tests {
    use super::*;
    
    /// Test that all test modules can be loaded
    #[test]
    fn test_modules_load() {
        // This test ensures all modules compile correctly
        assert!(true, "All test modules loaded successfully");
    }
}