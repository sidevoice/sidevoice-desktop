# Provider credentials

`platform/credentials.rs` owns the desktop `Credentials` interface: read, write/remove, and clear the
provider namespace. The engine and bridge use the same store on every supported OS. The page can set a
key, remove it, and ask whether it exists; it cannot read it back. Native voice availability is unchanged.

The implementation uses the maintained [keyring ecosystem](https://docs.rs/keyring/latest/keyring/):
`keyring-core` with explicit native stores, as recommended for applications needing namespace search.
There is no global keyring default, plaintext fallback, or production in-memory store.

| OS | Store and naming | Limits / availability |
|---|---|---|
| macOS | `apple-native-keyring-store` legacy Keychain Services; service `dev.sidevoice.desktop.providers`, account = provider | Same UTF-8 secrets and identifiers as the old `security_framework::passwords` calls; no migration or migration prompt. Existing user-keychain credentials remain readable. OS access control still applies. |
| Windows | `windows-native-keyring-store`, Credential Manager generic passwords; target `<provider>/dev.sidevoice.desktop.providers` | Keyring validates its UTF-16 password blob against `CRED_MAX_CREDENTIAL_BLOB_SIZE` (2,560 bytes) before calling `CredWriteW`. At most 1,280 UTF-16 code units (640 supplementary-plane characters). Oversized writes fail without truncation or replacing the old key. Names fit the native limits. |
| Linux | `zbus-secret-service-keyring-store`, encrypted D-Bus session with the user's Secret Service; exact `service` and `username` attributes | Requires a reachable session bus and accessible Secret Service. Missing/inaccessible/locked stores return `credentials_unavailable`, never absence or a successful fallback. A store may use its normal unlock prompt. |

Provider IDs retain the existing syntax: 1–32 ASCII lowercase letters, digits or dashes. Valid IDs outside
today's catalog can be stored and read. `clear()` removes the entire exact service, including unknown or
obsolete accounts, without consulting that catalog. macOS retains the former `SecItemDelete` search-list
scope through `security-framework`'s maintained safe API (keyring's search alone only covers its selected
keychain). Windows uses an anchored, escaped namespace pattern and a delimiter forbidden in service
names, so similarly named services are excluded. Linux searches the exact service attribute across
collections. Clear reports any deletion failure instead of claiming partial cleanup succeeded.

Backend errors are reduced to fixed categories; their messages, debug values, and malformed secret bytes
never reach logs or the bridge. The bridge retains `provider_invalid`, `credentials_failed`, and
`credentials_unavailable`; an oversized write also carries `reason: "too_long"`. All store work runs on
blocking workers, with credential operations serialized within the process. No additional native-reset
availability is enabled by this change; its existing caller now invokes `clear()`.

## Verification

Tests live with the interface and run without Tauri/model linkage via
`cargo test -p sidevoice-desktop-test-support --test credentials`. The existing macOS and Windows PR
jobs run them through `cargo xtask test-support`; the Linux workspace tests run them too.

- The in-memory keyring sample backend is test-only and non-persistent. It covers missing keys, update,
  idempotent removal, Unicode, unknown and malformed providers, ambiguous entries, malformed stored
  text, namespace cleanup/isolation, and removal of backend error details.
- Native tests use unique test services, never production credentials. macOS also checks old-API
  writes read through the new interface and new writes read through the old API. Windows tests ASCII
  and supplementary-character size boundaries and preservation of the prior value after refusal.
- Linux's absent-bus test runs in an isolated child process. The native persistence test is explicitly
  ignored on headless CI: successful Secret Service storage, unlock UI, and real desktop integration
  remain **unverified**. On an unlocked desktop, run the test above with `native_contract -- --ignored`.
- macOS access prompts/code-signing trust and Windows interactive-user behavior need real-machine
  verification beyond runner round trips. This does not establish native call support on either OS.
