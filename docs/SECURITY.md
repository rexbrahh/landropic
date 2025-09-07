# Landropic Security Guide

## Overview

Landropic uses state-of-the-art cryptography to secure your file synchronization. This document explains our security model, what is protected, and current limitations.

## What's Protected ✅

### Device Authentication
- Each device has a unique Ed25519 identity
- Devices must be explicitly paired before syncing
- All connections use mutual TLS authentication

### Network Security
- All transfers use QUIC with TLS 1.3
- No data is ever sent unencrypted
- Perfect forward secrecy for all connections

### Data Integrity
- All chunks are verified with Blake3 hashes
- Corrupted data is automatically detected
- Tampered files won't sync

## Current Limitations ⚠️ (Alpha)

### Not Yet Protected
1. **Files on disk** - Currently stored unencrypted (coming in beta)
2. **Memory** - Sensitive data not protected in RAM 
3. **Audit trail** - No logging of who accessed what
4. **Rate limiting** - No protection against resource exhaustion

### Security Model
- **Trust model**: Trust on first use (TOFU)
- **Authentication**: Device-level only (no users)
- **Authorization**: All-or-nothing (no per-file permissions)

## Best Practices

### Safe Usage
```bash
# DO: Sync non-sensitive test data
landro watch ~/test-docs

# DON'T: Sync sensitive data (yet)
landro watch ~/.ssh  # Bad idea!
```

### Device Pairing
Only pair devices you control:
1. Run pairing on trusted network (your home WiFi)
2. Verify device fingerprints match
3. Don't pair over public WiFi

### Secure Setup
```bash
# 1. Create dedicated sync folder
mkdir ~/LandropicSync
chmod 700 ~/LandropicSync

# 2. Start daemon with that folder
landro watch ~/LandropicSync

# 3. Only put test files there
```

## Security Features

### Ed25519 Device Identity
- Military-grade elliptic curve cryptography
- Keys generated with secure random source
- Private keys never leave your device

### TLS 1.3 Encryption
- Latest TLS standard
- No vulnerable legacy algorithms
- Certificate pinning for paired devices

### Content Addressing
- Files identified by cryptographic hash
- Deduplication without exposing content
- Tamper-evident storage

## Known Vulnerabilities

### Alpha Version Disclaimers
This is **ALPHA** software for testing only:

1. **No encryption at rest** - Files stored in plaintext
2. **No audit logging** - Cannot track access
3. **Self-signed certificates** - No certificate authority
4. **No rate limiting** - Vulnerable to DoS
5. **Basic access control** - Device-level only

## Reporting Security Issues

Found a security issue? Please report it:

1. **Critical issues**: Email security@landropic.dev (coming soon)
2. **Non-critical**: Open GitHub issue with [SECURITY] tag
3. **Questions**: Discussions forum

## Security Roadmap

### Beta (Q2 2024)
- ✅ Encryption at rest (ChaCha20-Poly1305)
- ✅ Security audit logging
- ✅ Rate limiting
- ✅ Memory protection

### v1.0 (Q3 2024)
- ✅ User access control
- ✅ File permissions sync
- ✅ Third-party audit
- ✅ Bug bounty program

## FAQ

### Is my data encrypted?
- **In transit**: Yes, always (TLS 1.3)
- **At rest**: Not yet (coming in beta)

### Can others read my files?
- **Network**: No, all transfers encrypted
- **Local**: Yes, if they have system access

### Is it safe for sensitive data?
- **Alpha**: No, testing only
- **Beta**: Better, but wait for v1.0
- **v1.0**: Yes, production ready

### How are devices authenticated?
- Ed25519 signatures
- Certificate pinning
- No passwords needed

### What about quantum computers?
- Current crypto is not quantum-resistant
- Post-quantum algorithms planned for v2.0

## Technical Details

### Cryptographic Primitives
- **Identity**: Ed25519 (EdDSA)
- **TLS**: X25519, ChaCha20-Poly1305, AES-256-GCM
- **Hashing**: Blake3 (faster than SHA-256)
- **KDF**: Argon2id (when implemented)

### Attack Surface
- **Network**: QUIC/TLS endpoints
- **Filesystem**: Watched directories
- **IPC**: None (single process)
- **Dependencies**: Minimal, audited

### Threat Model
We protect against:
- Network eavesdropping ✅
- Man-in-the-middle attacks ✅
- Data corruption ✅
- Device impersonation ✅

We don't yet protect against:
- Local system compromise ❌
- Memory dumps ❌
- Sophisticated targeted attacks ❌
- Supply chain attacks ❌

## Compliance

### Current Status
- No compliance certifications yet
- Not suitable for regulated data
- No warranty or liability

### Future Plans
- SOC 2 Type II (planned)
- GDPR compliance (planned)
- HIPAA compatibility (evaluating)

---

**Remember**: This is ALPHA software for testing only. Do not use for sensitive data yet!

For the latest security updates, see: https://github.com/landropic/landropic/security