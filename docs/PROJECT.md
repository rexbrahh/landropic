# Landropic Project Charter

**Goal:** Build a cross-platform, encrypted, LAN-based file sync & transfer tool. Think “AirDrop for everyone,” built on QUIC with end-to-end encryption.

## Roadmap Epics

1. Transport & Discovery
2. Sync Engine & Storage
3. Security & Identity
4. CLI / Daemon & UX
5. Perf & QA

## Success Criteria

- Fast: saturate 1Gb LAN on large transfers.
- Correct: resumable, byte-exact mirroring.
- Secure: mTLS, forward secrecy, integrity-checked chunks.
- Usable: simple CLI, clear logs, minimal setup.

## Workflow

- Issues tracked in GitHub Issues.
- Progress tracked in “LANdrop Roadmap” GitHub Project.
- PRs must link to issues and include tests.
