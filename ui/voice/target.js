/* Where this page talks to (docs/RENDEZVOUS.md, docs/DEVICE_PAIRING.md).
 *
 * A no-op on purpose. The page talks to the node this device is paired with; the target only adds one more
 * place to look for it. Whoever serves this build may say where it points:
 *
 *   - the desktop app injects it before the page loads, and this file leaves it alone;
 *   - a standalone deployment (a static image serving this build) replaces this file with one line:
 *
 *       window.__SIDEVOICE_TARGET__ = "<url of a node or a rendezvous room>";
 *
 * Loaded as a classic script before the application's module, so the value is in place before anything reads
 * it. The page's own address never sets it. */
