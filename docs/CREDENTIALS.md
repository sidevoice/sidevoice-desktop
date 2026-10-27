# Provider credentials

`platform/credentials.rs` owns the provider keys through plain functions: `valid(provider)`, `read(provider)`,
`write(provider, key | None)` and `clear_all()`, which clears the provider namespace. The engine and bridge use the
same store on every supported OS. The page can set a key, remove it, and ask whether it exists; it cannot read it back. Native voice availability is unchanged.

The implementation uses the maintained [keyring ecosystem](https://docs.rs/keyring/latest/keyring/):
`keyring-core` with explicit native stores, as recommended for applications needing namespace search.
There is no global keyring default, plaintext fallback, or production in-memory store.

| OS | Store and naming | Limits / availability |
|---|---|---|
| macOS | `apple-native-keyring-store` as it is: generic passwords in the user's login keychain; service `dev.sidevoice.desktop.providers`, account = provider | No compatibility with keys kept by earlier app versions (there are no users yet). Other keychains in the search list are not read or written. OS access control and Keychain prompts apply. |
| Windows | `windows-native-keyring-store`, Credential Manager generic passwords; target `<provider>/dev.sidevoice.desktop.providers` | Keyring validates its UTF-16 password blob against `CRED_MAX_CREDENTIAL_BLOB_SIZE` (2,560 bytes) before calling `CredWriteW`. At most 1,280 UTF-16 code units (640 supplementary-plane characters). Oversized writes fail without truncation or replacing the old key. Names fit the native limits. Entries explicitly use local-machine persistence (`Local` / `CRED_PERSIST_LOCAL_MACHINE`), so they survive logoff without roaming to other computers. Updating a formerly roaming entry changes it to local persistence. |
| Linux | `zbus-secret-service-keyring-store`, encrypted D-Bus session with the user's Secret Service; exact `service` and `username` attributes | Requires a reachable session bus and accessible Secret Service. Missing/inaccessible/locked stores return `credentials_unavailable`, never absence or a successful fallback. A store may use its normal unlock prompt; callers wait at most 30 seconds, then receive `credentials_unavailable`. |

Provider IDs retain the existing syntax: 1–32 ASCII lowercase letters, digits or dashes. Valid IDs outside
today's catalog can be stored and read. `clear_all()` removes the entire exact service, including unknown or
obsolete accounts, without consulting that catalog. macOS deletes the exact service in the login keychain with one service-scoped `SecItemDelete`. Windows uses an anchored, escaped namespace pattern and a delimiter forbidden in service
names, so similarly named services are excluded. Linux searches the exact service attribute across
collections. Clear reports any deletion failure instead of claiming partial cleanup succeeded.

Backend errors are reduced to fixed categories; their messages, debug values, and malformed secret bytes
never reach logs or the bridge. The bridge retains `provider_invalid`, `credentials_failed`, and
`credentials_unavailable`; an oversized write also carries `reason: "too_long"`. A credential trial uses only its supplied key and does not open the persisted store (the existing macOS behavior). All store work runs on
blocking workers. Windows operations, which never prompt, are serialized within the process. macOS (Keychain access
and unlock prompts) and Linux (Secret Service unlock prompts) use nonblocking admission and a 30-second response
deadline, with no global mutex held across a prompt. The synchronous
keyring backend cannot cancel a pending call: after a timeout the outcome remains unknown, and later
calls return unavailable immediately until that operation ends. Only one worker can remain pending;
retries never accumulate threads or race a late write against a subsequent write/clear. Once it ends,
operations can be retried. An indefinitely unanswered prompt may require dismissal or app restart.

## Reset

The reset transaction (docs/BRIDGE.md, `deleteAll()`) is the one caller of `clear_all()`. Its common `begin_reset` in
`data.rs` clears the provider keys after the macOS connector service and agent-results steps and before the app exits,
journalled as the `keychain` step; a failure is reported as `{step: "keychain", key: "reset.keychain-failed"}`. The step
is the same on every host that runs the transaction. Today that is macOS alone: elsewhere `deleteAll()` refuses
`reset.unavailable` before touching anything, so Windows and Linux keep their keys until the reset transaction runs
there.

## Verification

Portable contract tests live with the interface and run without Tauri/model linkage via
`cargo test -p sidevoice-desktop-test-support --test credentials`. The existing macOS and Windows PR
jobs run them through `cargo xtask test-support`; the Linux workspace tests run them too.
The shared concern holds the public functions and every store-independent body: naming, validation, requests,
the default namespace clear (the store's exact `service` search, then deleting each entry it returns), admission and
error categories. Its private `Os` trait, implemented by `Native` in `credentials/{macos,windows,linux}.rs`, has the
real differences only:

- `store()`: the OS's keyring store. Windows' store applies local persistence to its entries and turns the exact
  `service` search into the anchored target pattern. macOS uses the library store unchanged.
- `clear_namespace(service)`: deletes the whole namespace. The default is the search-and-delete above (Windows, Linux).
  macOS overrides it with one service-scoped `SecItemDelete` for all matches in the login keychain. The library's
  search would rebuild each account from its attribute text and delete by that name, so an account that does not
  round-trip through text could be missed while its `NoEntry` reads as success. The override reads no account.
- `PROMPTS`: whether store calls can wait on an unanswered access or unlock prompt (macOS and Linux). Common code
  selects the bounded admission from it.

Test builds give the public functions a per-process namespace, so the macOS and Windows fixtures of
`read`/`write`/`clear_all` never touch a person's keys.

Native regression fixtures live beside their OS implementation because they must exercise CredMan persistence metadata and byte limits, or Secret Service bus failure and
prompt admission. Each invokes the same portable contract where storage is available; the Linux live-store
contract remains explicitly ignored on headless CI. The native fixtures cannot be expressed solely through
read/write/clear: those operations intentionally hide the OS metadata and setup mechanisms being checked.

- The in-memory keyring sample backend is test-only and non-persistent. It covers missing keys, update,
  idempotent removal, Unicode, unknown and malformed providers, ambiguous entries, malformed stored
  text, namespace cleanup/isolation, and removal of backend error details.
- Native tests use unique test services, never production credentials. macOS runs the portable contract
  and the public functions on the real login keychain. Windows tests
  ASCII and supplementary-character size boundaries, preservation of the prior value after refusal,
  and stored local-machine persistence on both new and formerly roaming entries.
- A fake slow backend (the bounded admission Linux uses, tested on every OS) checks deadline expiry, immediate unavailable on retry, and recovery after
  the pending operation ends. Linux's absent-bus test runs in an isolated child process. The native persistence test is explicitly
  ignored on headless CI: successful Secret Service storage, unlock UI, and real desktop integration
  remain **unverified**. On an unlocked desktop, run the test above with `native_contract -- --ignored`.
- macOS access prompts/code-signing trust and Windows interactive-user behavior need real-machine
  verification beyond runner round trips. This does not establish native call support on either OS.
