# Changelog

## [0.2.0](https://github.com/sidevoice/sidevoice-desktop/compare/v0.1.0...v0.2.0) (2026-10-16)


### ⚠ BREAKING CHANGES

* host.engine lists the engine's catalogues and keeps provider keys; host.voice is voice 0.2.0's seam ([#41](https://github.com/sidevoice/sidevoice-desktop/issues/41))

### Features

* bridge smoke test in the real webview, navigation rule, docs ([22c81f8](https://github.com/sidevoice/sidevoice-desktop/commit/22c81f839af28eb4bb2d72cae99b49a253635fa4))
* **bridge:** forward rendered playback level ([#75](https://github.com/sidevoice/sidevoice-desktop/issues/75)) ([a8857ad](https://github.com/sidevoice/sidevoice-desktop/commit/a8857adaf7972928bb9d9b085c53a658a8bf3dc3))
* **bridge:** the mute shortcut and the call controls through the page's settings, and no target address ([#59](https://github.com/sidevoice/sidevoice-desktop/issues/59)) ([fdad3d1](https://github.com/sidevoice/sidevoice-desktop/commit/fdad3d1356a319df69e689a06c7589a7b506d806))
* bundle the web interface from integrate/cursor-on-split@9374111; prove the local-node path in WKWebView ([8859be0](https://github.com/sidevoice/sidevoice-desktop/commit/8859be037acf785e3c005c89457542a1d8ae8972))
* bundle the web interface with device pairing and the native engine device ([9c826ed](https://github.com/sidevoice/sidevoice-desktop/commit/9c826edd918d41b193a06e80cbd07c9ebc0d4f8a))
* bundle the web interface with the review's fixes (split/core-out-of-room@7b677bf) ([dd8d496](https://github.com/sidevoice/sidevoice-desktop/commit/dd8d4968f9942d089546a8cc8a8b2201517d598d))
* **data:** add desktop data reset and shared model store ([#78](https://github.com/sidevoice/sidevoice-desktop/issues/78)) ([a1f51e7](https://github.com/sidevoice/sidevoice-desktop/commit/a1f51e7ccf62defa4a70ed05d18158c9b66fce0d))
* **data:** host.data.uninstall() deletes all data, then trashes the app ([#95](https://github.com/sidevoice/sidevoice-desktop/issues/95)) ([58deace](https://github.com/sidevoice/sidevoice-desktop/commit/58deacef0856c5bd973ab45a35c6f68f70ed0d3d))
* **engine:** host.engine lists when each downloaded model was last used ([#52](https://github.com/sidevoice/sidevoice-desktop/issues/52)) ([74fda77](https://github.com/sidevoice/sidevoice-desktop/commit/74fda77b68775a1b3de76588364033b265d443a1))
* **engine:** host.engine.uninstall takes a downloaded model off the computer ([#48](https://github.com/sidevoice/sidevoice-desktop/issues/48)) ([1a4fd4b](https://github.com/sidevoice/sidevoice-desktop/commit/1a4fd4b55850f11701749a6a3c74a5b90b4a37d8))
* **engine:** run local voice on sidevoice-engine ([#34](https://github.com/sidevoice/sidevoice-desktop/issues/34)) ([fc9a8c4](https://github.com/sidevoice/sidevoice-desktop/commit/fc9a8c4900ee68b77f6f83427cd9919401d855f3))
* **engines:** catalog v2 copied from sidevoice-core; offers() is [#124](https://github.com/sidevoice/sidevoice-desktop/issues/124)'s resolver (phase 1) ([826bd18](https://github.com/sidevoice/sidevoice-desktop/commit/826bd18d999d815d17bf8a0756a39b529d901c0d))
* **engines:** load, unload, loaded and memory on the bridge; the app keeps models by D13 ([#3](https://github.com/sidevoice/sidevoice-desktop/issues/3)) ([eadb8a3](https://github.com/sidevoice/sidevoice-desktop/commit/eadb8a3bd7757c6d72639e20691678f422ce819d))
* **engines:** one catalog, capability detection, and sherpa-onnx loaded at run time ([d5c4325](https://github.com/sidevoice/sidevoice-desktop/commit/d5c4325615af1cb3f9a68132280c701d9bbaa5f4))
* **engines:** the catalog is sidevoice-core's v2, copied; offers() is [#124](https://github.com/sidevoice/sidevoice-desktop/issues/124)'s resolver ([7eddac2](https://github.com/sidevoice/sidevoice-desktop/commit/7eddac256d152831e5a71cb79f8216279bf32fd9))
* **engines:** the interface reaches the native engine through the bridge ([bc9e626](https://github.com/sidevoice/sidevoice-desktop/commit/bc9e626dd5be70522615de1ae7fa3444f27b2750))
* **engine:** upgrade to v0.5.0 and expose accelerator class ([#74](https://github.com/sidevoice/sidevoice-desktop/issues/74)) ([ea5a299](https://github.com/sidevoice/sidevoice-desktop/commit/ea5a299d32faec17a1b0fe1394724c3ceeb016c2))
* headset buttons in a call — media commands for any headset, the AirPods gesture on top ([e124254](https://github.com/sidevoice/sidevoice-desktop/commit/e124254b6782ce2e64fe1ff92e605c4f08451e8d))
* host.engine lists the engine's catalogues and keeps provider keys; host.voice is voice 0.2.0's seam ([#41](https://github.com/sidevoice/sidevoice-desktop/issues/41)) ([ae5fad9](https://github.com/sidevoice/sidevoice-desktop/commit/ae5fad9d5f881de46e65a883791e955255e82231))
* **local-host:** prepare this computer from the app, from the npm package, without Node ([#66](https://github.com/sidevoice/sidevoice-desktop/issues/66)) ([8f7d8f4](https://github.com/sidevoice/sidevoice-desktop/commit/8f7d8f4669496b7c3853f4900005504b2901a8a4))
* **local-host:** setStartAtLogin turns start at sign-in on or off without touching the running service ([#46](https://github.com/sidevoice/sidevoice-desktop/issues/46)) ([76262ce](https://github.com/sidevoice/sidevoice-desktop/commit/76262ce64ab7470d7e7ef4995e81677b67f5313e))
* **local-host:** the status says the installed Sidevoice's version ([#47](https://github.com/sidevoice/sidevoice-desktop/issues/47)) ([ac92370](https://github.com/sidevoice/sidevoice-desktop/commit/ac923705694cfa218438bf3a1a8615aa0cb1a69e))
* **local-host:** this computer's core as a host — socket checks, local pairing, the page's proxy, host.localHost (onboarding R1-c) ([#18](https://github.com/sidevoice/sidevoice-desktop/issues/18)) ([03631bc](https://github.com/sidevoice/sidevoice-desktop/commit/03631bc2648137503f56532da39c63657d5cf8fd))
* **microphone:** host.microphone, the macOS microphone permission for the page's first-run setup ([#93](https://github.com/sidevoice/sidevoice-desktop/issues/93)) ([c6cd6cc](https://github.com/sidevoice/sidevoice-desktop/commit/c6cd6cc459997cf6896fa6b3f6c324f1dcde4bb1))
* point the app at a node directly, with the web interface bundled ([542ddf1](https://github.com/sidevoice/sidevoice-desktop/commit/542ddf1a102720a294097bd89adfc8ee6327436b))
* **settings:** the native window keeps only diagnostics; shortcut and call controls are set in the page ([#62](https://github.com/sidevoice/sidevoice-desktop/issues/62)) ([45b92a0](https://github.com/sidevoice/sidevoice-desktop/commit/45b92a0ef9fa93f20fff40acbff5fe374dddb379))
* Tauri 2 shell for the Sidevoice room, with tray, mute shortcut and CI builds ([203ea01](https://github.com/sidevoice/sidevoice-desktop/commit/203ea013e95fa227e173298cab10934ea5d772c5))
* the app always shows its bundled interface; no remote pages, no room mode ([fa4ed7b](https://github.com/sidevoice/sidevoice-desktop/commit/fa4ed7be7264713913c0f876aa320291a0358089))
* the call controls card, floating over other apps during a call ([#5](https://github.com/sidevoice/sidevoice-desktop/issues/5)) ([1b3e494](https://github.com/sidevoice/sidevoice-desktop/commit/1b3e4946620ea02eb9a1724c19033cfae5af8d40))
* the desktop app wears the Sidevoice brand ([91aa1a8](https://github.com/sidevoice/sidevoice-desktop/commit/91aa1a8c6560fc61425ba81d87919689ac61fb5b))
* **turns:** forward native conversation flow through desktop voice bridge ([#77](https://github.com/sidevoice/sidevoice-desktop/issues/77)) ([8e6104a](https://github.com/sidevoice/sidevoice-desktop/commit/8e6104afc7ae6988d9777d0b28edc58aff0629d7))
* **updater:** a nightly updates to the next nightly, a release to the next release ([#69](https://github.com/sidevoice/sidevoice-desktop/issues/69)) ([d814447](https://github.com/sidevoice/sidevoice-desktop/commit/d814447ada604b548166f39da3a3bbd2e03f1f57))
* **updater:** a release app updates itself in the background; nightly and dev builds don't ([#56](https://github.com/sidevoice/sidevoice-desktop/issues/56)) ([1887446](https://github.com/sidevoice/sidevoice-desktop/commit/1887446589687ca59ac5717e114fbc9b61c81438))
* **updater:** check nightly every 10 minutes and release every hour ([#83](https://github.com/sidevoice/sidevoice-desktop/issues/83)) ([f612537](https://github.com/sidevoice/sidevoice-desktop/commit/f612537bccd8aa467ae298ca3410a0a7bf69baab))
* **updates:** cancel a download in progress ([#92](https://github.com/sidevoice/sidevoice-desktop/issues/92)) ([4f57586](https://github.com/sidevoice/sidevoice-desktop/commit/4f575863becdc38b49528beb70e8e783fa7ab322))
* **voice:** a model changed mid-call takes over without stopping the call ([#54](https://github.com/sidevoice/sidevoice-desktop/issues/54)) ([fe08dab](https://github.com/sidevoice/sidevoice-desktop/commit/fe08dabd19bb7d4b07eda742a3d87c9c13a31aec))
* **voice:** choose the call's microphone and speaker, mid-call too ([#43](https://github.com/sidevoice/sidevoice-desktop/issues/43)) ([ff255df](https://github.com/sidevoice/sidevoice-desktop/commit/ff255dfbd136963520b24009d2af1acad9c6c411))
* **voice:** host.voice hands a try's take to the page: start({take}) and onTake ([#71](https://github.com/sidevoice/sidevoice-desktop/issues/71)) ([3d570de](https://github.com/sidevoice/sidevoice-desktop/commit/3d570de840e7fcef3a653be4570e8853065074bd))
* **voice:** run the voice call natively with sidevoice-voice behind host.voice, keys in the keychain ([#37](https://github.com/sidevoice/sidevoice-desktop/issues/37)) ([11b2573](https://github.com/sidevoice/sidevoice-desktop/commit/11b257382ea030406ee47a88597191c01cd19668))


### Bug Fixes

* **bridge:** the call card changes the audio device through the web's setAudioDevices ([#49](https://github.com/sidevoice/sidevoice-desktop/issues/49)) ([f13c84d](https://github.com/sidevoice/sidevoice-desktop/commit/f13c84d3201ff1450ee4cd541255f753cb825595))
* **ci:** the .dmg's signing keychain goes on the search list, so codesign signs from it ([#70](https://github.com/sidevoice/sidevoice-desktop/issues/70)) ([41165ec](https://github.com/sidevoice/sidevoice-desktop/commit/41165ec285daccf0107a52dce7a449fb91caf05d))
* **copy:** name the connector's commands with npx ([#98](https://github.com/sidevoice/sidevoice-desktop/issues/98)) ([e1b67d9](https://github.com/sidevoice/sidevoice-desktop/commit/e1b67d9b1f3c292e68a28c81942f2188f0eb08b3))
* **copy:** tell people to run npx sidevoice pair-device ([#97](https://github.com/sidevoice/sidevoice-desktop/issues/97)) ([e471583](https://github.com/sidevoice/sidevoice-desktop/commit/e4715830fdb6e1bf6f1c8586648240ec706efbeb))
* **desktop:** the computer's own name as the device name, and the window in front at launch ([#64](https://github.com/sidevoice/sidevoice-desktop/issues/64)) ([eb76002](https://github.com/sidevoice/sidevoice-desktop/commit/eb76002b578e895ba35f86aa9b9aa6b2b574cf95))
* **engine:** host.engine tests a provider key before it is kept, and refreshes a provider ([#53](https://github.com/sidevoice/sidevoice-desktop/issues/53)) ([5c08962](https://github.com/sidevoice/sidevoice-desktop/commit/5c089626849069cf4a6c72a6b0a9e13d09dc7c8d))
* **engine:** host.engine.install takes the web's options (build, progress, signal) ([#51](https://github.com/sidevoice/sidevoice-desktop/issues/51)) ([b4f560c](https://github.com/sidevoice/sidevoice-desktop/commit/b4f560c4ed968d92b709c2b7f3c00fd8b79e689c))
* pre-publication audit — no private hosts or handover, MIT licence, releases-only README, re-vendored web ([#4](https://github.com/sidevoice/sidevoice-desktop/issues/4)) ([d694359](https://github.com/sidevoice/sidevoice-desktop/commit/d694359314fb8edf228e86fc8eac761931cdc036))
* settings window CSP allows the IPC; CI checks the app inside the dmg ([6f8501a](https://github.com/sidevoice/sidevoice-desktop/commit/6f8501a6716eb635836c627886af685316820369))
* **voice:** minimize native Silero VAD hold times ([#73](https://github.com/sidevoice/sidevoice-desktop/issues/73)) ([4d486c5](https://github.com/sidevoice/sidevoice-desktop/commit/4d486c5c06ceb872cfa8116761cf5813ec9295e9))
* **voice:** nothing reaches the page from a lane after dispose resolves ([#99](https://github.com/sidevoice/sidevoice-desktop/issues/99)) ([c8bbfbd](https://github.com/sidevoice/sidevoice-desktop/commit/c8bbfbd810342f1f23239b535b121dfc918f278d))
* **voice:** the call takes the web's chosen build and end-of-turn model ([#50](https://github.com/sidevoice/sidevoice-desktop/issues/50)) ([cf9cdc8](https://github.com/sidevoice/sidevoice-desktop/commit/cf9cdc8bc1eed9ecb92b0aef2e96c97c6b92cc07))
* what the adversarial review confirmed on the app side ([7f7a1bb](https://github.com/sidevoice/sidevoice-desktop/commit/7f7a1bb85058cf49282ee72adcfe138b38283041))
* what the brand review confirmed on the desktop side ([e8c4ed0](https://github.com/sidevoice/sidevoice-desktop/commit/e8c4ed0c4fdb9a9ca03f4e445f6433849fc000e6))
