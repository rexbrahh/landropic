Testing & validation

Deterministic tests: Chunker golden vectors; manifest equality across OSes; round-trip encode/decode of messages.

Integration harness: Two-node and three-node clusters inside a CI job with tmpfs; scripted edits & power-kill.

Network emulation: netem profiles (0–5% loss, 1–20 ms latency) to verify resumption & backpressure.

Filesystem matrix: APFS/HFS+, ext4, NTFS; perms propagation where supported.

Security: TLS audits (certificate pinning), TOFU disabled by default; fuzz control parser with libFuzzer.
