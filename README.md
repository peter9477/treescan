Treescan
========
Utility to produce deterministic filesystem scan summaries.
The original goal was to scan an entire filesystem, outputting metadata
(e.g. owner, permissions, mtime, hash and path) for each file,
in a consistent order (depth first, alphabetical) so that the
output could be diffed in order to quickly identify differences
between various hosts, usually where one had begun as a clone
of the other's disk image.

This was first written in Python 2, ported to Python 3, then
rewritten in Rust in mid-2022.
