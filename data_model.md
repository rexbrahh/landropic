Data model

On disk

.landropic/index.sqlite

files(path TEXT PK, inode, size, mtime, mode, uid, gid, manifest_hash BLOB, folder_id TEXT)

manifests(manifest_hash BLOB PK, json BLOB) // array of {chunk_id, len, offset}

chunks(chunk_id BLOB PK, size, refcount, location, at_rest_key_id)

peers(device_id TEXT PK, cert_pin BLOB, first_seen, last_seen, trust_state)

.landropic/objects/xx/<chunk_id>

Optional: .landropic/config.toml (ports, peers, keys, folders)

On wire (control stream protobuf)

Hello{ device_id, build, caps, folders[] }

FolderSummary{ folder_id, root_hash, bloom, counts }

Want{ folder_id, [chunk_id...] }

Ack{ [chunk_id: bitset-seq] }

Error{ code, msg }
