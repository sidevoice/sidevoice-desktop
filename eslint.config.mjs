const nodeGlobals = {
  AbortController: "readonly",
  Buffer: "readonly",
  clearTimeout: "readonly",
  console: "readonly",
  process: "readonly",
  setTimeout: "readonly",
};

export default [{
  files: [
    "test/macos/native-pair-update-smoke.mjs",
    "test/macos/committed-pair-release.mjs",
    "test/macos/native-pair-update-preflight.mjs",
    "test/macos/release-root-containment.mjs",
    "scripts/align-r4-native-pair-core-pin.mjs",
  ],
  languageOptions: {
    ecmaVersion: "latest",
    sourceType: "module",
    globals: nodeGlobals,
  },
  rules: {
    "no-undef": "error",
    "no-unused-vars": ["error", { args: "none", caughtErrors: "none" }],
  },
}];
