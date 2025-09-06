CLI:

landropic init # create device + folder keys landropic pair [--qr | --code ABC-123] # trust a peer landropic serve # run daemon (mDNS + QUIC) landropic sync <folder> # one-shot sync landropic watch <folder> # continuous sync (foreground) landropic send <path> --to <peer> # one-off transfer landropic status # sessions, queue, throughput

Daemon control (local HTTP/gRPC)

GET /status → sessions, queue sizes, throughput

POST /folders → add/remove watched folder

POST /peers/pin → add trusted peer (from QR payload)
