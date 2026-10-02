# Connector release metadata and manifest fixtures

`core-manifest-core34.json` is byte-for-byte the R4-b fixture generated with the producer in core PR #34. Its bundle and
wheel checksums and sizes use synthetic test bytes; it covers the real manifest shape and is not signed-asset evidence.

`connector-install-r1.json` and `connector-install-r4.json` reflect the service installer record shape. Connector and
core version ordering metadata lives in the selected release's `release.json`, represented by the paired fixtures.

`connector-release-r4-build19.json` records the connector identity and build ordering from the reviewed R4 pin at
`86a0ab1a5363af303b25f77fc7254aef5db1fea3`; it is used to check eligibility against the bundled R2 build 25.
It is metadata-only and does not represent a retained executable or core bundle.
