Demo script (for hiring/showcase)

On laptop & desktop: landropic init && landropic serve (QR pair once).

landropic watch ~/Projects/site on laptop; edit a file → desktop updates in < 2 s.

Drop a 5 GB VM image; first sync saturates link; second sync transfers only changed blocks.

Kill the process mid-transfer; restart → resumes at chunk boundaries.

lanray status shows throughput, queue, acks, and no data ever left the LAN.
