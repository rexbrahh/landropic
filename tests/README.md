# Landropic Integration Tests

This directory contains comprehensive integration tests for the Landropic file synchronization system. These tests verify that all components work together correctly in end-to-end scenarios.

## Test Structure

```
tests/
├── integration/           # Integration test modules
│   ├── cli_daemon_test.rs    # CLI <-> Daemon communication tests
│   ├── sync_test.rs          # File synchronization tests
│   ├── network_test.rs       # QUIC network layer tests
│   └── storage_test.rs       # Storage and indexing tests
├── common/               # Shared test utilities
│   └── mod.rs               # Test helpers and fixtures
├── fixtures/             # Test configuration and data
│   ├── test_config.toml     # Test configuration
│   └── sample_files.json    # Sample file definitions
├── lib.rs               # Test library entry point
├── run_integration_tests.sh # Test runner script
└── README.md            # This file
```

## Test Categories

### 1. CLI-Daemon Integration Tests (`cli_daemon_test.rs`)

Tests the communication between the CLI client and daemon service:

- **`test_cli_start_stop_daemon`**: Verifies CLI can start and stop daemon
- **`test_cli_daemon_communication`**: Tests CLI commands work with running daemon
- **`test_cli_daemon_not_running`**: Tests CLI behavior when daemon is down
- **`test_cli_command_sequence`**: Tests multiple CLI commands in sequence
- **`test_cli_invalid_arguments`**: Tests CLI error handling with invalid inputs

**Key Functionality Tested:**
- Daemon process management via CLI
- PID file handling and process detection
- CLI status reporting
- Error handling and user feedback

### 2. Sync Functionality Tests (`sync_test.rs`)

Tests the core file synchronization logic:

- **`test_basic_file_sync`**: Basic file sync between two daemons
- **`test_sync_engine_integration`**: Sync engine components working together
- **`test_chunking_and_cas_integration`**: Chunking and content-addressed storage
- **`test_manifest_integration`**: Manifest generation and comparison
- **`test_file_watcher_integration`**: File system monitoring
- **`test_conflict_detection`**: Conflict detection and resolution

**Key Functionality Tested:**
- File change detection and indexing
- Content chunking and deduplication  
- Manifest generation and diffing
- Sync orchestration and scheduling
- Conflict detection algorithms

### 3. Network Layer Tests (`network_test.rs`)

Tests the QUIC transport and networking components:

- **`test_quic_server_basic`**: QUIC server startup and shutdown
- **`test_quic_client_connection`**: Client-server connection establishment
- **`test_quic_sync_protocol`**: Sync protocol message handling
- **`test_service_discovery`**: mDNS peer discovery
- **`test_network_error_conditions`**: Network failure scenarios
- **`test_network_performance`**: Performance under load
- **`test_concurrent_connections`**: Multiple simultaneous connections

**Key Functionality Tested:**
- QUIC connection establishment and management
- Encrypted data transmission
- Service discovery via mDNS
- Protocol message serialization/deserialization
- Network error handling and recovery
- Performance characteristics

### 4. Storage Layer Tests (`storage_test.rs`)

Tests the storage, indexing, and data management components:

- **`test_cas_basic_operations`**: Content-addressed storage operations
- **`test_atomic_writer`**: Atomic file writing for concurrent safety
- **`test_chunker_comprehensive`**: File chunking with various patterns
- **`test_indexer_comprehensive`**: File indexing and manifest generation
- **`test_storage_performance`**: Storage performance under load
- **`test_storage_error_handling`**: Storage error conditions
- **`test_manifest_operations`**: Manifest diffing and operations

**Key Functionality Tested:**
- Content-addressed storage (CAS) operations
- File chunking with configurable parameters
- SQLite-based file indexing
- Manifest generation and comparison
- Atomic operations for data integrity
- Performance and error handling

## Running Tests

### Prerequisites

1. **Build binaries first:**
   ```bash
   cargo build -p landro-daemon
   cargo build -p landro-cli
   ```

2. **Install dependencies:**
   ```bash
   cargo build --workspace
   ```

### Running All Integration Tests

Use the provided test runner script:

```bash
./tests/run_integration_tests.sh
```

This script will:
- Build all required binaries
- Set up test environment
- Run all integration tests in sequence
- Clean up test artifacts
- Provide detailed results summary

### Running Individual Test Modules

```bash
# Run specific test module
cargo test --package landropic --test lib integration::cli_daemon_test

# Run specific test function
cargo test --package landropic --test lib test_basic_file_sync -- --nocapture

# Run with debug logging
RUST_LOG=debug cargo test --package landropic --test lib integration::sync_test
```

### Environment Variables

- `RUST_LOG`: Set logging level (debug, info, warn, error)
- `LANDROPIC_TEST_MODE`: Enable test mode (set by test runner)
- `BUILD_MODE`: Build mode for binaries (debug/release)

## Test Environment

### Temporary Directories

Tests use isolated temporary directories:
- `/tmp/landropic-test*`: Test storage and data
- Individual test temp dirs via `tempfile` crate

### Test Daemons

Tests spawn real daemon processes with:
- Ephemeral ports to avoid conflicts
- Isolated storage directories  
- Test-specific device identities
- Controlled cleanup on test completion

### Network Testing

Network tests use:
- Localhost-only connections
- Dynamic port allocation
- Simulated network conditions
- Certificate validation bypassed for testing

## Common Test Utilities

### `TestDaemon` Helper

The `TestDaemon` struct provides:
- Automated daemon process management
- Port allocation and address tracking
- Storage directory management
- Graceful startup/shutdown
- Process monitoring

### File System Helpers

- `create_test_files()`: Generate test file hierarchies
- `generate_test_data()`: Create test data of specified sizes
- `compare_directories()`: Verify sync results
- `find_free_port()`: Allocate test ports

### Crypto Initialization

All tests call `init_crypto()` to set up the rustls crypto provider.

## Test Data and Fixtures

### Configuration Files

- `test_config.toml`: Test-specific configuration parameters
- `sample_files.json`: Predefined test file sets and scenarios

### Test Scenarios

Defined test scenarios include:
- Basic two-device sync
- Complex directory structures
- Large file transfers
- Conflict resolution
- Network failure recovery

## Expected Test Results

### Success Criteria

✅ **All integration tests should pass** in a clean environment with:
- No conflicting processes on test ports
- Sufficient disk space for test files
- Network connectivity for localhost
- Required system permissions

### Known Limitations

⚠️ **Current Alpha Limitations:**
- Some network tests may timeout due to certificate validation
- Service discovery may not find peers in isolated environments
- Large file transfers are simplified in alpha version
- Conflict resolution is basic (manual resolution)

### Performance Expectations

Tests include performance validation:
- Storage operations: >1000 ops/sec for small objects
- Network serialization: >10 MB/s for message processing
- Indexing: <100ms for small file hierarchies
- Startup time: <5 seconds for daemon initialization

## Troubleshooting

### Common Issues

1. **Port conflicts:**
   ```bash
   # Kill existing test processes
   pkill -f "landro-daemon.*test"
   ```

2. **Permission denied:**
   ```bash
   # Ensure test script is executable
   chmod +x tests/run_integration_tests.sh
   ```

3. **Missing binaries:**
   ```bash
   # Rebuild required components
   cargo build -p landro-daemon -p landro-cli
   ```

4. **Test timeouts:**
   ```bash
   # Run with increased timeout and debug logging
   TEST_TIMEOUT=600 RUST_LOG=debug ./tests/run_integration_tests.sh
   ```

### Debug Information

For debugging failing tests:

1. **Check test logs:**
   ```bash
   tail -f /tmp/landropic_test_*.log
   ```

2. **Run single test with full output:**
   ```bash
   cargo test test_name -- --nocapture
   ```

3. **Enable verbose daemon logging:**
   ```bash
   RUST_LOG=landro_daemon=debug cargo test ...
   ```

## Test Coverage

Current integration test coverage includes:

| Component | Coverage | Tests |
|-----------|----------|-------|
| CLI-Daemon Communication | ✅ Full | 5 tests |
| File Sync Logic | ✅ Full | 6 tests |
| QUIC Network Layer | ✅ Full | 7 tests |
| Storage & Indexing | ✅ Full | 6 tests |
| Error Handling | ✅ Good | Across all tests |
| Performance | ✅ Basic | 3 tests |

### Missing Coverage (Future)

Areas for future test expansion:
- Multi-peer sync scenarios (>2 devices)
- Network partition recovery
- Large-scale performance testing
- Cross-platform compatibility
- Security attack scenarios

## Contributing

When adding new integration tests:

1. **Follow naming convention:** `test_<functionality>_<scenario>`
2. **Use test utilities:** Leverage `TestDaemon` and helper functions
3. **Include cleanup:** Ensure tests clean up resources
4. **Add documentation:** Document test purpose and expected behavior
5. **Consider performance:** Include timing assertions where relevant

### Test Template

```rust
#[tokio::test]
async fn test_new_functionality() -> Result<()> {
    init_crypto();
    info!("Testing new functionality");
    
    // Setup
    let mut daemon = TestDaemon::new("test-daemon")?;
    let port = daemon.start(None).await?;
    
    // Test logic
    // ... test implementation ...
    
    // Cleanup
    daemon.stop().await?;
    
    Ok(())
}
```

For questions or issues with integration tests, check the [main project documentation](../README.md) or create an issue.