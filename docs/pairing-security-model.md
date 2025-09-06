# Landropic Device Pairing Security Model

## Overview

The Landropic device pairing system implements a secure passphrase-based authentication protocol that establishes trust between devices without requiring pre-shared keys or a central authority.

## Security Goals

1. **Mutual Authentication**: Both devices prove they know the shared passphrase
2. **Forward Secrecy**: Compromise of long-term keys doesn't reveal past session keys
3. **Resistance to MITM**: Attackers cannot impersonate devices without the passphrase
4. **Protection Against Dictionary Attacks**: Slow key derivation makes offline attacks infeasible
5. **Secure Identity Exchange**: Device identities are verified after establishing secure channel

## Protocol Design

### SPAKE2-like PAKE Implementation

We implement a simplified SPAKE2-inspired protocol that combines:
- **Passphrase-derived keys** using Argon2id
- **X25519 ephemeral key exchange** for forward secrecy
- **Ed25519 signatures** for identity verification
- **ChaCha20-Poly1305** for authenticated encryption

### Protocol Flow

```
Alice (Initiator)                          Bob (Responder)
-----------------                          ----------------
1. Derive K from passphrase                1. Derive K from passphrase
   K = Argon2id(passphrase, salt)             K = Argon2id(passphrase, salt)

2. Generate ephemeral X25519 keypair       
   (a, A = g^a)                            

3. Send Message A:                         
   -> A, nonce_A                           

                                           4. Receive Message A
                                           5. Generate ephemeral keypair
                                              (b, B = g^b)
                                           6. Compute shared secret:
                                              S = A^b
                                              SK = S ⊕ K
                                           7. Create confirmation:
                                              conf_B = MAC(SK, nonce_A || B)
                                           8. Send Message B:
                                              <- B, nonce_B, conf_B

9. Receive Message B                       
10. Compute shared secret:                 
    S = B^a                                
    SK = S ⊕ K                             
11. Verify conf_B                          
12. Exchange identities (encrypted):       12. Exchange identities (encrypted):
    -> Enc(SK, ID_A, Sign_A(A||B))            -> Enc(SK, ID_B, Sign_B(B||A))
13. Verify signatures                      13. Verify signatures
14. Pairing complete                       14. Pairing complete
```

## Cryptographic Primitives

### Key Derivation (Argon2id)
- **Algorithm**: Argon2id (hybrid of Argon2i and Argon2d)
- **Memory**: 64 MB
- **Iterations**: 3
- **Parallelism**: 4 threads
- **Salt**: Fixed protocol salt (should be unique per deployment)
- **Output**: 32 bytes

### Ephemeral Key Exchange (X25519)
- **Curve**: Curve25519
- **Key Size**: 256 bits
- **Purpose**: Provides forward secrecy and fresh randomness

### Identity Signatures (Ed25519)
- **Algorithm**: Ed25519
- **Key Size**: 256 bits
- **Purpose**: Long-term device identity and authentication

### Symmetric Encryption (ChaCha20-Poly1305)
- **Algorithm**: ChaCha20-Poly1305 AEAD
- **Key Size**: 256 bits
- **Nonce Size**: 96 bits
- **Purpose**: Authenticated encryption of identity exchange

### Message Authentication (BLAKE3)
- **Algorithm**: BLAKE3 in keyed MAC mode
- **Key Size**: 256 bits
- **Output Size**: 256 bits
- **Purpose**: Confirmation messages during handshake

## Security Properties

### Resistance to Attacks

1. **Dictionary Attacks**: Argon2id with high memory cost makes offline attacks expensive
2. **Man-in-the-Middle**: Attacker cannot compute correct confirmation without passphrase
3. **Replay Attacks**: Fresh nonces prevent replay of old messages
4. **Forward Secrecy**: Ephemeral X25519 keys are deleted after use
5. **Identity Binding**: Ed25519 signatures bind identities to the ephemeral keys

### Trust Model

- **Initial Trust**: Established through out-of-band passphrase exchange
- **Identity Persistence**: Device Ed25519 keys stored securely on disk
- **Session Keys**: Derived fresh for each pairing session
- **Key Zeroization**: Sensitive data cleared from memory after use

## Implementation Details

### State Machine

The pairing process follows a strict state machine:
1. `Idle` - Initial state
2. `InitiatorSentA` - Message A sent (initiator only)
3. `ResponderSentB` - Message B sent (responder only)
4. `SharedSecretEstablished` - PAKE complete
5. `IdentityExchanged` - Identities verified
6. `Completed` - Pairing successful
7. `Failed` - Pairing failed (with reason)

### Error Handling

- Invalid state transitions result in immediate failure
- Signature verification failures terminate the protocol
- Timeout handling prevents hanging connections
- All errors are logged for debugging

### Memory Safety

- Uses Rust's `zeroize` crate for secure memory clearing
- Ephemeral secrets consumed after use (move semantics)
- Sensitive data wrapped in `Drop` implementations
- No secrets logged or displayed

## Integration Points

### With QUIC Transport
- Pairing happens before QUIC connection establishment
- Shared secret used to derive QUIC session keys
- Device identities used for certificate generation

### With Device Identity (landro-crypto)
- Ed25519 keys from `DeviceIdentity` used for signatures
- Integration with certificate generation for mTLS
- Persistent storage of paired device identities

### With Connection Management
- Pairing state tracked per connection attempt
- Failed pairings trigger connection retry logic
- Successful pairings cached for reconnection

## Testing

The implementation includes comprehensive tests:
- Unit tests for each protocol step
- Integration tests for full pairing flow
- Negative tests for wrong passphrase scenarios
- Timing attack resistance validation

## Future Enhancements

1. **SRP Support**: Add option for SRP (Secure Remote Password) protocol
2. **QR Code Pairing**: Support QR codes for passphrase exchange
3. **Multi-factor Authentication**: Add optional second factor
4. **Pairing History**: Track and audit pairing attempts
5. **Rate Limiting**: Prevent brute force attempts

## References

- [SPAKE2 RFC Draft](https://datatracker.ietf.org/doc/draft-irtf-cfrg-spake2/)
- [Argon2 RFC 9106](https://www.rfc-editor.org/rfc/rfc9106.html)
- [X25519 RFC 7748](https://www.rfc-editor.org/rfc/rfc7748.html)
- [Ed25519 RFC 8032](https://www.rfc-editor.org/rfc/rfc8032.html)
- [ChaCha20-Poly1305 RFC 8439](https://www.rfc-editor.org/rfc/rfc8439.html)