Key flows

1. Pairing

landropic init generates device identity: Ed25519 device_id, X25519 TLS key, folder keys.

landropic pair --qr shows a QR with device public key + short authentication code (SAS).

Peer scans → compare SAS out-of-band → add to trusted peers list (pinned cert).

On connect, mutual TLS + version negotiation on control stream.

2. Sync negotiation

Peers exchange folder manifests: <folder_id, root hash, bloom of chunk_ids, file count, total size>.

Receiver computes missing chunk set via bloom & sparse asks.

Receiver sends WANT list grouped by priority: metadata first, then small files, then large file hot chunks (front-to-back).

3. Transfer

Scheduler opens multiple QUIC streams (N configurable) and pipelines chunks; each chunk frame: {chunk_id, file_id, offset, seq, payload, mac}.

Receiver verifies blake3(payload)==chunk_id and per-frame MAC; writes into CAS; stitches file via hardlinks or copy-on-write to temp file then atomic rename.

Ack stream carries selective ACKs & gap requests; backpressure via QUIC flow control.
