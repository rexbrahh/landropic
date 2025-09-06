Delivery roadmap (8 weeks → v1 GA) Week 1 — Foundations & contracts

Repos & workspaces; CI (fmt, clippy, tests).

Choose libs: quinn, rustls, blake3, notify, zeroconf, rusqlite.

Define wire messages (protobuf) & DB schema; freeze for MVP.

Spike: QUIC echo between two hosts; mDNS advert/resolve POC.

Exit: “hello” handshake on QUIC with mTLS and version negotiation.

Week 2 — Indexing & chunk store

Implement FastCDC with configurable avg chunk size (e.g., 64 KiB).

CAS layout + atomic writes + fsync boundaries; refcounting.

File manifest builder; index sqlite; integrity scanner.

Exit: lanray index <folder> produces stable manifests & CAS.

Week 3 — Diff & control protocol

Bloom filter summary + WANT negotiation; edge-cases (renames, truncation).

Control stream handlers, timeouts, retries, telemetry.

Exit: lanray diff shows precise missing chunk counts between two peers.

Week 4 — Data plane & resumption

Data stream framing; per-stream acks; resend windows; backpressure.

Crash-safe receive path (temp files + atomic rename).

Exit: Transfer large file end-to-end; resume mid-file after simulated crash.

Week 5 — Watchers & continuous sync

Cross-platform watchers (macOS FSEvents, Linux inotify, Win USN/FSEvents fallback).

Debounce & coalesce events; schedule priorities (small files first).

Exit: Background daemon mirrors edits within seconds on same LAN.

Week 6 — Security hardening & pairing UX

QR pairing (SAS code), pinned peers list, trust states.

Optional at-rest encryption (folder key) with streaming XChaCha20.

Fuzz parsers (control messages), prop-tests for chunker.

Exit: Pairing story polished; audit log of pair events.

Week 7 — Scale & perf

Million-small-files test; pack-on-the-fly for tiny chunks; ziplist batching.

Parallelism tuning (streams per conn, IO queue depths).

Benchmarks vs rsync/Syncthing on LAN.

Exit: Hit perf targets on 1 Gb and 10 Gb LAN; CPU profile stable.

Week 8 — Polish & GA

CLI ergonomics, status, progress bars; installers/services; docs.

Failure matrix: disk full, permission errors, clock skew, network flap.

Exit: GA build with reproducible install & a scripted demo.
