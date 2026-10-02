# Rejection-only ZIP fixtures

These tiny synthetic archives exercise the consumer's extraction boundary. None contains a valid signature
or represents a genuine connector artifact. Entries use a fixed ZIP timestamp; `oversized.zip` expands to
1 MiB + 1 byte. `invalid-bundle.zip` has the correct root filename but deliberately invalid bundle JSON.
The other fixtures contain traversal, duplicate/extra entries, or an empty bundle.
