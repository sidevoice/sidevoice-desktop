# Connector release metadata and manifest fixtures

`core-manifest-core34.json` is byte-for-byte the R4-b fixture generated with the producer in core PR #34. Its bundle and
wheel checksums and sizes use synthetic test bytes; it covers the real manifest shape and is not signed-asset evidence.

`connector-install-r1.json` and `connector-install-r4.json` reflect the service installer record shape. Connector and
core version ordering metadata lives in the selected release's `release.json`, represented by the paired fixtures.
