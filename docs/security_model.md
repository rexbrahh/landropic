Security model

Identity: Each device has long-term Ed25519 identity; TLS uses X25519 ephemeral keys; cert pinned to device_id.

Mutual auth: mTLS; pairing pins peer cert. Optional PSK mode for headless environments.

Forward secrecy: Provided by TLS 1.3 over QUIC (ephemeral KEX).

Integrity: Chunk hash + per-frame MAC; manifest hashes; end-to-end object verification independent of transport.

Replay/rollback protection: Monotonic folder epochs; receiver rejects manifests older than its view unless --force.

Privacy: mDNS advertises only (device_id, version, port). No filenames/path leakage.
