Risks & mitigations

Clock skew → bad LWW decisions: include device monotonic timestamps + drift estimation; warn when skew > 2 s.

Small-file storms: batch into pack-chunks; prioritize directories with fewer pending items first; compress headers.

Filesystem quirks: normalize permissions/mtime where not supported; provide --preserve-perms=false escape hatch.

mDNS collisions/noisy LANs: exponential backoff advert; view filters by known peers; allow manual IP override.
