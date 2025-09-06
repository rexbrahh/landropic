Key design choices & trade-offs

QUIC+mTLS vs Noise over UDP: We choose QUIC+mTLS for built-in congestion control, recovery, multiplexing, and OS/hardware acceleration. Application-level MACs guard objects regardless of transport and enable CAS.

FastCDC chunking: Better delta for shifted edits than fixed-size blocks; chunk size tuned by profile.

SQLite for metadata: Simple, robust, ubiquitous. Pack files later for very small chunks to reduce inode pressure.

Conflict policy: LWW keeps UX simple; preserve both versions within a short collision window to prevent surprise overwrites.

Discovery via mDNS: LAN-only in v1 keeps the threat model tight and the UX simple.
