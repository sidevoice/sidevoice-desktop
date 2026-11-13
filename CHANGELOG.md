# Changelog

## [0.5.0](https://github.com/sidevoice/sidevoice-desktop/compare/v0.4.1...v0.5.0) (2026-11-12)


### Features

* **engine:** the engine's recommendation and builds that stop in the background, for the page ([#497](https://github.com/sidevoice/sidevoice-desktop/issues/497)) ([7582211](https://github.com/sidevoice/sidevoice-desktop/commit/7582211f681c1c381f8e3933158c9ed8e0b5242e))
* **ios:** choose the microphone from the audio session and open the system output picker ([#498](https://github.com/sidevoice/sidevoice-desktop/issues/498)) ([2ea1523](https://github.com/sidevoice/sidevoice-desktop/commit/2ea1523cdb002f8e9eec5141e076699c6f43b4e3))
* **ios:** ring the stacked faces of other conversations in the Live Activity ([#500](https://github.com/sidevoice/sidevoice-desktop/issues/500)) ([5448471](https://github.com/sidevoice/sidevoice-desktop/commit/54484711104e442361e46fc2425ef6c02dfe6403))
* **local-host:** update the service in the background within the pinned minor ([#503](https://github.com/sidevoice/sidevoice-desktop/issues/503)) ([e6ac0a1](https://github.com/sidevoice/sidevoice-desktop/commit/e6ac0a1fe6809fe19cf908d9b7fcada57c9867f8))


### Bug Fixes

* **ios:** do not report a false failure when the output picker opens ([#506](https://github.com/sidevoice/sidevoice-desktop/issues/506)) ([db79723](https://github.com/sidevoice/sidevoice-desktop/commit/db79723e9ee1fbe0892679799b42f9dcb567abb1))
* **ios:** end the call's Live Activity when the app is swiped away ([#493](https://github.com/sidevoice/sidevoice-desktop/issues/493)) ([0078a6c](https://github.com/sidevoice/sidevoice-desktop/commit/0078a6cd0e3e37b64a3a4cf527d1d8672096b8a3))
* **ios:** log every thinking-sound refusal and failure at warn, in public ([#511](https://github.com/sidevoice/sidevoice-desktop/issues/511)) ([4e1f3e5](https://github.com/sidevoice/sidevoice-desktop/commit/4e1f3e531b790fc3d8865d609134d1d40ad01ce7))
* **ios:** the output button names the system's route in every case ([#507](https://github.com/sidevoice/sidevoice-desktop/issues/507)) ([54e0f5d](https://github.com/sidevoice/sidevoice-desktop/commit/54e0f5dd17d55562fe769b133590500d49a31b38))
* **local-host:** the state names the core's own port, not the proxy's ([#504](https://github.com/sidevoice/sidevoice-desktop/issues/504)) ([f5afed8](https://github.com/sidevoice/sidevoice-desktop/commit/f5afed830b471d5a6312072eaaf3a3c7a1f167a9))
* **mobile:** keep the creature stills' transparency in the Live Activity stack ([#510](https://github.com/sidevoice/sidevoice-desktop/issues/510)) ([bd4fe57](https://github.com/sidevoice/sidevoice-desktop/commit/bd4fe577ba28dda28c46a1b499431896c9cd06b7))

## [0.4.1](https://github.com/sidevoice/sidevoice-desktop/compare/v0.4.0...v0.4.1) (2026-11-10)


### Features

* **mobile:** host.screen says whether the screen is locked, and the iOS cue plays "only with the screen off" while locked ([#482](https://github.com/sidevoice/sidevoice-desktop/issues/482)) ([853281f](https://github.com/sidevoice/sidevoice-desktop/commit/853281fca1f6832929ec9c2d36452a4b1e7618ec))


### Bug Fixes

* **local-host:** stage installs on Windows in the app's folder, not %TEMP%, and say what failed ([#485](https://github.com/sidevoice/sidevoice-desktop/issues/485)) ([8d1e93a](https://github.com/sidevoice/sidevoice-desktop/commit/8d1e93ab39f2d14b60e7ba81ef0517e70ae38fb8))
* **media-controls:** AirPods Press to Mute on iPhone and Mac ([#487](https://github.com/sidevoice/sidevoice-desktop/issues/487)) ([f9a13ba](https://github.com/sidevoice/sidevoice-desktop/commit/f9a13baaf5d0fda5dbc29d0b35b009ed8ab6e095))
* **packaging:** the brand icon on the Windows installer and uninstaller, and every hicolor size on Linux ([#484](https://github.com/sidevoice/sidevoice-desktop/issues/484)) ([e4e9623](https://github.com/sidevoice/sidevoice-desktop/commit/e4e96237bd2695c7bbee036ecd169720c78cd717))


### Miscellaneous Chores

* release desktop as 0.4.1 ([#491](https://github.com/sidevoice/sidevoice-desktop/issues/491)) ([c68262b](https://github.com/sidevoice/sidevoice-desktop/commit/c68262bf263e3b2ac584d629cc69e88be8e1117f))

## [0.4.0](https://github.com/sidevoice/sidevoice-desktop/compare/v0.3.0...v0.4.0) (2026-11-09)


### ⚠ BREAKING CHANGES

* **workspace:** shared, desktop and mobile, each with core and tauri ([#367](https://github.com/sidevoice/sidevoice-desktop/issues/367))
* **platform:** host kind decided at the crate root ([#359](https://github.com/sidevoice/sidevoice-desktop/issues/359))
* **bridge:** platform content behind the host, no os ([#350](https://github.com/sidevoice/sidevoice-desktop/issues/350))
* **bridge:** inject window.__sidevoiceHost and pin the web release ([#337](https://github.com/sidevoice/sidevoice-desktop/issues/337))
* **app:** identifier ai.sidevoice.desktop ([#316](https://github.com/sidevoice/sidevoice-desktop/issues/316))

### Features

* **bridge:** inject window.__sidevoiceHost and pin the web release ([#337](https://github.com/sidevoice/sidevoice-desktop/issues/337)) ([fa61e89](https://github.com/sidevoice/sidevoice-desktop/commit/fa61e89276e9fa06e327c2fc7b599df06a7cdf60))
* **bridge:** let the apps overlay the web's texts with their own wording ([#373](https://github.com/sidevoice/sidevoice-desktop/issues/373)) ([bcc43e4](https://github.com/sidevoice/sidevoice-desktop/commit/bcc43e4dfea8c9c98d1f9c2b4ef8ac85693e4d26))
* **call-controls:** tell the card its room to the work area's sides ([#454](https://github.com/sidevoice/sidevoice-desktop/issues/454)) ([6102dae](https://github.com/sidevoice/sidevoice-desktop/commit/6102daed895248a7005c1b51f5b821b48b54a308))
* **desktop:** global shortcuts: host.shortcuts, press and release, single keys and Fn on macOS ([#448](https://github.com/sidevoice/sidevoice-desktop/issues/448)) ([a879310](https://github.com/sidevoice/sidevoice-desktop/commit/a879310feff928db469140cf07b816163d122b69))
* **desktop:** host.screen says whether the screen is locked, for the thinking sound's screen off ([#480](https://github.com/sidevoice/sidevoice-desktop/issues/480)) ([50e694d](https://github.com/sidevoice/sidevoice-desktop/commit/50e694d9b6d1a7f64db9a4ee6f9a9630b31fa226))
* **i18n:** native texts in Portuguese (Brazil), Korean and Hindi ([#470](https://github.com/sidevoice/sidevoice-desktop/issues/470)) ([9b7c532](https://github.com/sidevoice/sidevoice-desktop/commit/9b7c532177fbe2d6e0eeaa09ff1b3aa1f954a840))
* **media:** lower or pause other apps' media while the microphone is open ([#408](https://github.com/sidevoice/sidevoice-desktop/issues/408)) ([683ec64](https://github.com/sidevoice/sidevoice-desktop/commit/683ec6489ca677a5cc75bb39030834c3d4c0b392))
* **mobile/android:** a foreground service keeps the call's process ([#330](https://github.com/sidevoice/sidevoice-desktop/issues/330)) ([e5fe792](https://github.com/sidevoice/sidevoice-desktop/commit/e5fe7927e48546cc934df763f870a258aa63f492))
* **mobile/android:** the call notification as Sidevoice's own card ([#400](https://github.com/sidevoice/sidevoice-desktop/issues/400)) ([3ee52ee](https://github.com/sidevoice/sidevoice-desktop/commit/3ee52eebf4230d3b11da144519ee4aeb94a15085))
* **mobile/ios:** host.cue plays the thinking sound the person chose, read from the bundled web ([#477](https://github.com/sidevoice/sidevoice-desktop/issues/477)) ([37ea211](https://github.com/sidevoice/sidevoice-desktop/commit/37ea21195c5a23466ea8da3f798173aa9d6ed5b4))
* **mobile/ios:** host.cue, the app plays the page's thinking sound in the call's audio session (web[#439](https://github.com/sidevoice/sidevoice-desktop/issues/439)) ([#468](https://github.com/sidevoice/sidevoice-desktop/issues/468)) ([5e6aa24](https://github.com/sidevoice/sidevoice-desktop/commit/5e6aa2413f9e8bc96747cf655f3b8009ca075579))
* **mobile/ios:** privacy manifest and export-compliance declaration ([#335](https://github.com/sidevoice/sidevoice-desktop/issues/335)) ([ebefecf](https://github.com/sidevoice/sidevoice-desktop/commit/ebefecf93808a1615cc64b9abd24e364d0c967ce))
* **mobile/ios:** the call's Live Activity and Dynamic Island ([#399](https://github.com/sidevoice/sidevoice-desktop/issues/399)) ([831f010](https://github.com/sidevoice/sidevoice-desktop/commit/831f0103fb9e637a5970d874b801d8e5c1f3f64d))
* **mobile/ios:** Whisper on the GPU through the engine's whisper.cpp on Metal ([#383](https://github.com/sidevoice/sidevoice-desktop/issues/383)) ([d6d7842](https://github.com/sidevoice/sidevoice-desktop/commit/d6d78420409e2203d3ed7cc8e47fbedd445d039c))
* **mobile:** CallKit and Core-Telecom off by default, behind a nightly developer option ([#397](https://github.com/sidevoice/sidevoice-desktop/issues/397)) ([9471b7a](https://github.com/sidevoice/sidevoice-desktop/commit/9471b7a70b0c9ef93b01efc555e3d06d11f7f525))
* **mobile:** carry the call surface across the bridge ([#398](https://github.com/sidevoice/sidevoice-desktop/issues/398)) ([bb7d6df](https://github.com/sidevoice/sidevoice-desktop/commit/bb7d6df5abe695cdafab8aeeef7b5e9b382190d7))
* **mobile:** keep the screen on while a model download runs ([#362](https://github.com/sidevoice/sidevoice-desktop/issues/362)) ([8ba39e7](https://github.com/sidevoice/sidevoice-desktop/commit/8ba39e7d4d5f0628ffce67e4f1225d01e787eb58))
* **mobile:** pause the conversation during audio interruptions ([#358](https://github.com/sidevoice/sidevoice-desktop/issues/358)) ([f4687dd](https://github.com/sidevoice/sidevoice-desktop/commit/f4687dd4a2dcac6a80595375de54bf541cf8c3ef))
* **mobile:** scan the pairing QR code ([#407](https://github.com/sidevoice/sidevoice-desktop/issues/407)) ([cdff626](https://github.com/sidevoice/sidevoice-desktop/commit/cdff62675afa3f3960d6dccb524654b6428d23ea))
* **mobile:** ship Android armeabi-v7a in a universal APK ([#375](https://github.com/sidevoice/sidevoice-desktop/issues/375)) ([3894e53](https://github.com/sidevoice/sidevoice-desktop/commit/3894e53c19aa7c11c544472bf21357da3196a726))
* **mobile:** the app's members through the shared plugin ([#357](https://github.com/sidevoice/sidevoice-desktop/issues/357)) ([38926d7](https://github.com/sidevoice/sidevoice-desktop/commit/38926d7f806629d19a0ac0041edf32587d058371))
* **mobile:** the call is a phone call (CallKit, Core-Telecom) ([#369](https://github.com/sidevoice/sidevoice-desktop/issues/369)) ([6bfe0db](https://github.com/sidevoice/sidevoice-desktop/commit/6bfe0db4ea6d364a3687af925b69bf66808ffbb1))
* **mobile:** the headset's single and double press on iOS and Android, through host.shortcuts ([#460](https://github.com/sidevoice/sidevoice-desktop/issues/460)) ([a46b399](https://github.com/sidevoice/sidevoice-desktop/commit/a46b3998c0dabf7e196bd00803273c034277bd4a))
* **mobile:** the mobile app crate ([#327](https://github.com/sidevoice/sidevoice-desktop/issues/327)) ([34cb240](https://github.com/sidevoice/sidevoice-desktop/commit/34cb240006d317426c60964b575ce26967821754))
* **mobile:** the page learns the phone's text size on Android ([#458](https://github.com/sidevoice/sidevoice-desktop/issues/458)) ([da4d63f](https://github.com/sidevoice/sidevoice-desktop/commit/da4d63fbb366d27655641a874f2482e231a08575))
* **platform:** iOS and Android headset buttons and lock-screen controls ([#363](https://github.com/sidevoice/sidevoice-desktop/issues/363)) ([0ee49b7](https://github.com/sidevoice/sidevoice-desktop/commit/0ee49b751793c4acec5f8101f5d545855fe1c82a))
* **platform:** iOS and Android microphone permission ([#333](https://github.com/sidevoice/sidevoice-desktop/issues/333)) ([d21a80c](https://github.com/sidevoice/sidevoice-desktop/commit/d21a80c7d716b8937024dd7a3c7043014cca20ca))
* **platform:** iOS and Android provider keys ([#332](https://github.com/sidevoice/sidevoice-desktop/issues/332)) ([c343478](https://github.com/sidevoice/sidevoice-desktop/commit/c3434788f03f1328c35abe2e74043460dcb7c42b))
* **shortcuts:** defaults as a state, hold/tap/double on one key, refusals that name the holder, a bounded record ([#456](https://github.com/sidevoice/sidevoice-desktop/issues/456)) ([4c88dc7](https://github.com/sidevoice/sidevoice-desktop/commit/4c88dc7b5a1ff4173728b82a1bcd1e9bd4627312))


### Bug Fixes

* **app-tauri:** engine commands ask who is calling, like the voice commands ([#344](https://github.com/sidevoice/sidevoice-desktop/issues/344)) ([0fceddd](https://github.com/sidevoice/sidevoice-desktop/commit/0fceddd3407cd5f72ee6a887a84b671f6e735c15))
* **app:** carry an install's data over from dev.sidevoice.desktop ([#341](https://github.com/sidevoice/sidevoice-desktop/issues/341)) ([b1f85e3](https://github.com/sidevoice/sidevoice-desktop/commit/b1f85e394e2d7abba902bf7f7753bcb9c68077d7))
* **call-controls:** a dragged card keeps its creature under the pointer ([#475](https://github.com/sidevoice/sidevoice-desktop/issues/475)) ([9f48751](https://github.com/sidevoice/sidevoice-desktop/commit/9f487519e7cffe71f575c67cf20eca71aa26edd0))
* **ios:** sign the Live Activity extension with its App Group ([#439](https://github.com/sidevoice/sidevoice-desktop/issues/439)) ([77a0794](https://github.com/sidevoice/sidevoice-desktop/commit/77a07949286df1ce0e2d9df0623125dd09e7bd2b))
* **live-activity:** your voice's bars in the brand purple ([#452](https://github.com/sidevoice/sidevoice-desktop/issues/452)) ([b7ce23e](https://github.com/sidevoice/sidevoice-desktop/commit/b7ce23e63120e1ba6197c42e6f24ebead9d1bb4b))
* **local-host:** find the connector's ai.sidevoice.* jobs ([#317](https://github.com/sidevoice/sidevoice-desktop/issues/317)) ([a15d3d3](https://github.com/sidevoice/sidevoice-desktop/commit/a15d3d378aae4815fc3c8a5338a47b869525ea3e))
* **media-duck:** offer a restart when System Audio Recording is allowed but needs one ([#446](https://github.com/sidevoice/sidevoice-desktop/issues/446)) ([d2fd8df](https://github.com/sidevoice/sidevoice-desktop/commit/d2fd8df625a3b7be9f1d757ce45a92e42f1a06c5))
* **mobile/ios:** declare the audio session again after a media services reset ([#389](https://github.com/sidevoice/sidevoice-desktop/issues/389)) ([220448f](https://github.com/sidevoice/sidevoice-desktop/commit/220448fa07144d4c4cf8b9438f615db5110a20cd))
* **mobile/ios:** every cargo of an app build links the engine's checked libraries ([#364](https://github.com/sidevoice/sidevoice-desktop/issues/364)) ([a056240](https://github.com/sidevoice/sidevoice-desktop/commit/a056240b7c8aae412b5a617bb129bc5094427f58))
* **mobile/ios:** every step of a creature still in the phone's log, and the signed App Group checked ([#434](https://github.com/sidevoice/sidevoice-desktop/issues/434)) ([075b266](https://github.com/sidevoice/sidevoice-desktop/commit/075b2663c46b16a1080c2c09a0ee859721478ead))
* **mobile/ios:** list src/panics.rs in the committed iOS project ([#418](https://github.com/sidevoice/sidevoice-desktop/issues/418)) ([76ace0e](https://github.com/sidevoice/sidevoice-desktop/commit/76ace0efeac06c4d223f2251fca9c21b1f77b65d))
* **mobile/ios:** Live Activity bars and layout match the design ([#412](https://github.com/sidevoice/sidevoice-desktop/issues/412)) ([889d60d](https://github.com/sidevoice/sidevoice-desktop/commit/889d60d9c40e9f1b77784bcddc647ae0b0ad2168))
* **mobile/ios:** Live Activity follows the call, and its buttons work while locked ([#410](https://github.com/sidevoice/sidevoice-desktop/issues/410)) ([014d150](https://github.com/sidevoice/sidevoice-desktop/commit/014d1501729853a20eb21e1746a4080ec1b3735b))
* **mobile/ios:** Live Activity text in pages, new bars and placeholders ([#428](https://github.com/sidevoice/sidevoice-desktop/issues/428)) ([51ac0d8](https://github.com/sidevoice/sidevoice-desktop/commit/51ac0d8cd7a87bb7b9c87f71c056e87908558d38))
* **mobile/ios:** log the headset state when it changes, not on every report ([#415](https://github.com/sidevoice/sidevoice-desktop/issues/415)) ([5bc55b1](https://github.com/sidevoice/sidevoice-desktop/commit/5bc55b18f7cf4fd3ce03d15427bd07ba4c2b23d0))
* **mobile/ios:** mute works from the locked Lock Screen ([#430](https://github.com/sidevoice/sidevoice-desktop/issues/430)) ([86c9f5e](https://github.com/sidevoice/sidevoice-desktop/commit/86c9f5e0e4814953914bcf4fac720806f5e5898d))
* **mobile/ios:** mute, hang up and cancel work from the Lock Screen ([#425](https://github.com/sidevoice/sidevoice-desktop/issues/425)) ([a97e206](https://github.com/sidevoice/sidevoice-desktop/commit/a97e20664971f9e2be1ae33313c0747b2b7262fe))
* **mobile/ios:** never show Now Playing at a call's start, and let go of every remote command ([#433](https://github.com/sidevoice/sidevoice-desktop/issues/433)) ([7bf2aeb](https://github.com/sidevoice/sidevoice-desktop/commit/7bf2aebaafdcc72d41da1893b0b7818d2a580f3f))
* **mobile/ios:** no empty Now Playing card beside the call's Live Activity ([#427](https://github.com/sidevoice/sidevoice-desktop/issues/427)) ([5f4134b](https://github.com/sidevoice/sidevoice-desktop/commit/5f4134bfd89d642660fbf5480c5fb708b14a16e2))
* **mobile/ios:** no Now Playing card beside the call's Live Activity ([#413](https://github.com/sidevoice/sidevoice-desktop/issues/413)) ([2060b16](https://github.com/sidevoice/sidevoice-desktop/commit/2060b1615cae5dd242365bd0536590c31aa04ca5))
* **mobile/ios:** Press to Mute without CallKit, no Now Playing in calls, and the headset reports what each press can do ([#469](https://github.com/sidevoice/sidevoice-desktop/issues/469)) ([bf44b85](https://github.com/sidevoice/sidevoice-desktop/commit/bf44b859cc2068244ad0be070f1d400471ea011a))
* **mobile/ios:** say why a creature's still is not drawn ([#414](https://github.com/sidevoice/sidevoice-desktop/issues/414)) ([39c409f](https://github.com/sidevoice/sidevoice-desktop/commit/39c409f30bcea3bdf50debcf19b71204a5b59fe4))
* **mobile/ios:** the app's own call declares the voice-chat session (car call) ([#466](https://github.com/sidevoice/sidevoice-desktop/issues/466)) ([9335dff](https://github.com/sidevoice/sidevoice-desktop/commit/9335dffbfd73aed26c9e59ab38232184a2dcbb07))
* **mobile/ios:** the creature in the compact island, the slashed tray, transcribing breathes ([#444](https://github.com/sidevoice/sidevoice-desktop/issues/444)) ([bd67c1b](https://github.com/sidevoice/sidevoice-desktop/commit/bd67c1b8eb161beb7cf7975ac0e0d2979b4784bb))
* **mobile/ios:** the expanded island's insets are ours alone and it fits 144 pt ([#431](https://github.com/sidevoice/sidevoice-desktop/issues/431)) ([8cc9d8e](https://github.com/sidevoice/sidevoice-desktop/commit/8cc9d8e24649a7b3117756b7866542fac1f675ae))
* **mobile/ios:** the Live Activity buttons register on the device ([#426](https://github.com/sidevoice/sidevoice-desktop/issues/426)) ([8872e51](https://github.com/sidevoice/sidevoice-desktop/commit/8872e51e2261df5aee390ff0afd755e6491b8104))
* **mobile/ios:** the Live Activity's skip icon matches the app's ([#422](https://github.com/sidevoice/sidevoice-desktop/issues/422)) ([f047004](https://github.com/sidevoice/sidevoice-desktop/commit/f047004098ea74560a7a879a5ffddce4cb15fc57))
* **mobile:** CallKit takes the call (the voip background mode) ([#374](https://github.com/sidevoice/sidevoice-desktop/issues/374)) ([72814b3](https://github.com/sidevoice/sidevoice-desktop/commit/72814b3aca41450066c1dc5a0f9ff9eed050b924))
* **mobile:** close the Live Activity: your words alone, the voice pill, full chunks, the state's action ([#441](https://github.com/sidevoice/sidevoice-desktop/issues/441)) ([c747fe8](https://github.com/sidevoice/sidevoice-desktop/commit/c747fe8b60a2b9caa74dc8966e0cadd7c6c97ea7))
* **mobile:** fill the screen on the system's background, and no pinch zoom ([#360](https://github.com/sidevoice/sidevoice-desktop/issues/360)) ([a2549c1](https://github.com/sidevoice/sidevoice-desktop/commit/a2549c117828376f8de7b7401f69dab799e229b7))
* **mobile:** log at info, not every crate's trace ([#378](https://github.com/sidevoice/sidevoice-desktop/issues/378)) ([61f02a4](https://github.com/sidevoice/sidevoice-desktop/commit/61f02a4f6406956e4d193fc5243a437264d1aae2))
* **mobile:** open Android call audio only once Telecom has activated the call ([#379](https://github.com/sidevoice/sidevoice-desktop/issues/379)) ([645b81c](https://github.com/sidevoice/sidevoice-desktop/commit/645b81c42d8921f27f46a7dcbdb983121ed3d26a))
* **mobile:** report host.build so Developer options unlock on a nightly ([#385](https://github.com/sidevoice/sidevoice-desktop/issues/385)) ([64e8c6b](https://github.com/sidevoice/sidevoice-desktop/commit/64e8c6b1532c153a6d8bb152cb799fcbfccf82d6))
* **mobile:** supply host.deviceName so onboarding stops proposing "Chrome on Android" ([#377](https://github.com/sidevoice/sidevoice-desktop/issues/377)) ([ea4af77](https://github.com/sidevoice/sidevoice-desktop/commit/ea4af7730c40caa5f8946c218c3f8bb5b227b98c))
* **mobile:** tell the native project's cargo what build it is (Developer options on TestFlight) ([#424](https://github.com/sidevoice/sidevoice-desktop/issues/424)) ([a4e4f5a](https://github.com/sidevoice/sidevoice-desktop/commit/a4e4f5ac52ab0a976c926db21a0c2c65a1f17874))
* **mobile:** the iOS page fills the screen; simulator screenshots in CI ([#370](https://github.com/sidevoice/sidevoice-desktop/issues/370)) ([0ee6d52](https://github.com/sidevoice/sidevoice-desktop/commit/0ee6d52650c934ed2c34abb4c3240853cd26f834))
* **mobile:** validate a still's PNG chunks and keep failed deletions pending ([#402](https://github.com/sidevoice/sidevoice-desktop/issues/402)) ([ce99aa0](https://github.com/sidevoice/sidevoice-desktop/commit/ce99aa0467b28362cd0736008aca11d115a6ac4b))
* **shortcuts:** an F-key recorded alone for hold to talk is held, as a right-hand modifier is ([#474](https://github.com/sidevoice/sidevoice-desktop/issues/474)) ([9866141](https://github.com/sidevoice/sidevoice-desktop/commit/9866141d7137209d797ab62c83cbd8662e568aab))
* **shortcuts:** name keys by the physical key; map a headset double press to next track ([#450](https://github.com/sidevoice/sidevoice-desktop/issues/450)) ([4344075](https://github.com/sidevoice/sidevoice-desktop/commit/4344075111ad0078b4325e2efea9cf07927d8e02))
* **voice:** pin voice 0.8.2 and hold the device streams during an interruption ([#390](https://github.com/sidevoice/sidevoice-desktop/issues/390)) ([1ca649a](https://github.com/sidevoice/sidevoice-desktop/commit/1ca649a907a7c29dbaa8c59658ee732d40b49375))


### Miscellaneous Chores

* **app:** identifier ai.sidevoice.desktop ([#316](https://github.com/sidevoice/sidevoice-desktop/issues/316)) ([ed18096](https://github.com/sidevoice/sidevoice-desktop/commit/ed180962079e954f997aec68337c8a15f1261d79))


### Code Refactoring

* **bridge:** platform content behind the host, no os ([#350](https://github.com/sidevoice/sidevoice-desktop/issues/350)) ([a1f3629](https://github.com/sidevoice/sidevoice-desktop/commit/a1f362995acc38ba8569b0914bedfc0aa19dcb85))
* **platform:** host kind decided at the crate root ([#359](https://github.com/sidevoice/sidevoice-desktop/issues/359)) ([cf9b49f](https://github.com/sidevoice/sidevoice-desktop/commit/cf9b49fc6aebd35c9239d1f5c1821c663cf37026))
* **workspace:** shared, desktop and mobile, each with core and tauri ([#367](https://github.com/sidevoice/sidevoice-desktop/issues/367)) ([89c8d90](https://github.com/sidevoice/sidevoice-desktop/commit/89c8d90ede8bef22db1ddae01cdcfa57153dc925))

## [0.3.0](https://github.com/sidevoice/sidevoice-desktop/compare/v0.2.1...v0.3.0) (2026-10-30)


### ⚠ BREAKING CHANGES

* rename the core binary to sidevoice-core ([#285](https://github.com/sidevoice/sidevoice-desktop/issues/285))

### Features

* abstract native reset confirmation across desktop platforms ([#199](https://github.com/sidevoice/sidevoice-desktop/issues/199)) ([02916bf](https://github.com/sidevoice/sidevoice-desktop/commit/02916bf3484be5f01e9225f9f17ed1d68ae54f20))
* **data:** one rule for the shared models folder ([#190](https://github.com/sidevoice/sidevoice-desktop/issues/190)) ([848d6a7](https://github.com/sidevoice/sidevoice-desktop/commit/848d6a708a44d22e65ecbb14c4c0069e4bc09afe))
* **local-host:** find the connector and core through the shared data directory ([#189](https://github.com/sidevoice/sidevoice-desktop/issues/189)) ([010d858](https://github.com/sidevoice/sidevoice-desktop/commit/010d85811e5f404df30476e9e38c0d754e268f23))
* **local-host:** Windows named pipes, Job Object trees and file identity ([#262](https://github.com/sidevoice/sidevoice-desktop/issues/262)) ([c592b56](https://github.com/sidevoice/sidevoice-desktop/commit/c592b5678f44aeec535c1e4462c2733a4ac3463c))
* open Windows microphone privacy settings ([#197](https://github.com/sidevoice/sidevoice-desktop/issues/197)) ([7564b41](https://github.com/sidevoice/sidevoice-desktop/commit/7564b41284ee26934030588a90a0734008f7ccfa))
* **platform:** add microphone access interface ([#214](https://github.com/sidevoice/sidevoice-desktop/issues/214)) ([54f9d69](https://github.com/sidevoice/sidevoice-desktop/commit/54f9d69277a57e7aa206ca327693bf3f71bda682))
* **platform:** authenticate reset helper handoff across desktop hosts ([#216](https://github.com/sidevoice/sidevoice-desktop/issues/216)) ([86bee48](https://github.com/sidevoice/sidevoice-desktop/commit/86bee485d143cdc22f190ed9bd53ea70b373e77b))
* **platform:** local host and native voice on every OS, no per-OS capability ([#264](https://github.com/sidevoice/sidevoice-desktop/issues/264)) ([3a14443](https://github.com/sidevoice/sidevoice-desktop/commit/3a14443aae78f68864cfba2c14ca94c9807cd1c9))
* **platform:** store provider credentials across desktop hosts ([#219](https://github.com/sidevoice/sidevoice-desktop/issues/219)) ([c248f77](https://github.com/sidevoice/sidevoice-desktop/commit/c248f77ba726a2fb853bc030e7875519d43f13b8))
* **reset:** connector step only if installed, with a fail-closed service-absence proof per OS ([#263](https://github.com/sidevoice/sidevoice-desktop/issues/263)) ([95d9c2d](https://github.com/sidevoice/sidevoice-desktop/commit/95d9c2d7609f75a86ebc586a3f8fbfea13dd6785))
* **reset:** one reset flow on every desktop host ([#261](https://github.com/sidevoice/sidevoice-desktop/issues/261)) ([f872087](https://github.com/sidevoice/sidevoice-desktop/commit/f872087c8ce52841e6f1a6b1c5a5864ed7f5fede))
* **reset:** Windows-safe helper handoff, checked relaunch and trusted-handle journal ([#265](https://github.com/sidevoice/sidevoice-desktop/issues/265)) ([c79acc9](https://github.com/sidevoice/sidevoice-desktop/commit/c79acc991449c3227dd598a0fc2ff6ff13c55dc5))
* **voice:** native voice call on Windows and Linux with sidevoice-voice 0.7.4 ([#258](https://github.com/sidevoice/sidevoice-desktop/issues/258)) ([ab2d5b5](https://github.com/sidevoice/sidevoice-desktop/commit/ab2d5b5d2f6174abf85655e711619e6111740c52))


### Bug Fixes

* **build:** regenerate command permissions and fail CI on drift ([#272](https://github.com/sidevoice/sidevoice-desktop/issues/272)) ([9a591c7](https://github.com/sidevoice/sidevoice-desktop/commit/9a591c730b538146aa8a6eca6cd19e9c56fc96a7))
* **call-controls:** the card is told whether each conversation's machine can be reached ([#194](https://github.com/sidevoice/sidevoice-desktop/issues/194)) ([2bc59a7](https://github.com/sidevoice/sidevoice-desktop/commit/2bc59a734d7d9c319dc30841ff45391e65894b0a))
* **call-controls:** the card's pick of a conversation whose agent is away reaches the room ([#193](https://github.com/sidevoice/sidevoice-desktop/issues/193)) ([2e7bae2](https://github.com/sidevoice/sidevoice-desktop/commit/2e7bae2c925c2db50cb04bf5aa05fcf0c73ea666))
* **engine:** pin sidevoice-engine v0.6.0 ([#252](https://github.com/sidevoice/sidevoice-desktop/issues/252)) ([c33d43d](https://github.com/sidevoice/sidevoice-desktop/commit/c33d43d6fbf501f2bdbe5281518458ba40f34f73))
* **install:** log the installer's failure detail ([#271](https://github.com/sidevoice/sidevoice-desktop/issues/271)) ([be174d9](https://github.com/sidevoice/sidevoice-desktop/commit/be174d9e8314608970b4f8acbdf3414f92ac7fe0))
* **microphone:** log what the device check sees ([#281](https://github.com/sidevoice/sidevoice-desktop/issues/281)) ([38237c0](https://github.com/sidevoice/sidevoice-desktop/commit/38237c0f0c4c95ad8216476aa37a5ed78832178b))
* **platform:** reach data deletion roots beneath the held home without following links ([#267](https://github.com/sidevoice/sidevoice-desktop/issues/267)) ([be6b31e](https://github.com/sidevoice/sidevoice-desktop/commit/be6b31e1b8ce935aab7ff6cbfdc3d2fc8dfab7d0))
* **platform:** read macOS task_vm_info through mach2 bindings ([#248](https://github.com/sidevoice/sidevoice-desktop/issues/248)) ([0dbf770](https://github.com/sidevoice/sidevoice-desktop/commit/0dbf77038776d8e97b0383724ba4f6e8e68d72c3))
* **platform:** report probe memory on every host ([#238](https://github.com/sidevoice/sidevoice-desktop/issues/238)) ([ba11e0f](https://github.com/sidevoice/sidevoice-desktop/commit/ba11e0f7ae921723c7aeb12c462aa73c01f714af))
* **release:** staple the signed app with retries, then repack and re-sign the update ([#276](https://github.com/sidevoice/sidevoice-desktop/issues/276)) ([1b5589d](https://github.com/sidevoice/sidevoice-desktop/commit/1b5589d6ddf90fd41b50f5d4f5794b49c9bf36f5))
* **windows:** declare UTF-8 as the app's ANSI code page ([#286](https://github.com/sidevoice/sidevoice-desktop/issues/286)) ([b651efc](https://github.com/sidevoice/sidevoice-desktop/commit/b651efc2f35f22151a7d1c6534233a3c467e3d76))
* **windows:** show the floating card when another app covers the room ([#280](https://github.com/sidevoice/sidevoice-desktop/issues/280)) ([f02bef0](https://github.com/sidevoice/sidevoice-desktop/commit/f02bef06ba404abc1d57d6b8eb27c4b086567600))
* **xtask:** keep signing passwords off the command line ([#273](https://github.com/sidevoice/sidevoice-desktop/issues/273)) ([1a91ad7](https://github.com/sidevoice/sidevoice-desktop/commit/1a91ad732025b56dc2b2930ae8f3bb2a01a38fe2))
* **xtask:** token-aware platform guard with alias and OS-command coverage ([#232](https://github.com/sidevoice/sidevoice-desktop/issues/232)) ([890f1b4](https://github.com/sidevoice/sidevoice-desktop/commit/890f1b4ee212447e0c5351a3a143fb87599866b0))


### Code Refactoring

* rename the core binary to sidevoice-core ([#285](https://github.com/sidevoice/sidevoice-desktop/issues/285)) ([65a3e4d](https://github.com/sidevoice/sidevoice-desktop/commit/65a3e4d2123552019fa6dc73f7906f9362cd4194))

## [0.2.1](https://github.com/sidevoice/sidevoice-desktop/compare/v0.2.0...v0.2.1) (2026-10-21)


### Features

* **call-card:** a click on a corner moves the card there and makes it the preferred one ([#148](https://github.com/sidevoice/sidevoice-desktop/issues/148)) ([92c2469](https://github.com/sidevoice/sidevoice-desktop/commit/92c246945e983d70be7db3a53f19ace22a3e0dfe))
* **call-card:** a setting to turn the floating card off ([#135](https://github.com/sidevoice/sidevoice-desktop/issues/135)) ([b042f61](https://github.com/sidevoice/sidevoice-desktop/commit/b042f61deaea8d38ff2b5c64decce800aaf08c7d))
* **call-card:** free resize for the picture-in-picture window ([#134](https://github.com/sidevoice/sidevoice-desktop/issues/134)) ([37746c6](https://github.com/sidevoice/sidevoice-desktop/commit/37746c6389053514f990cc7812b848fe8bf344dd))
* **call-controls:** show the card while the stage is out of view, not on focus loss ([#175](https://github.com/sidevoice/sidevoice-desktop/issues/175)) ([2d56e78](https://github.com/sidevoice/sidevoice-desktop/commit/2d56e78ae0af51046244535e5677996fa868d286))
* **callcard:** host the new card formats ([#128](https://github.com/sidevoice/sidevoice-desktop/issues/128)) ([51b23f1](https://github.com/sidevoice/sidevoice-desktop/commit/51b23f1ebc0e9dbd04a7a6238f652d0fb058309e))
* **debug:** developer options on nightly builds (Web Inspector, compositing borders, repaint counters) ([#168](https://github.com/sidevoice/sidevoice-desktop/issues/168)) ([2f3d6ed](https://github.com/sidevoice/sidevoice-desktop/commit/2f3d6edbf43f4388980397dac7c3727c99b3cb26))
* **logs:** persist an app log and show it in Ver registro ([#115](https://github.com/sidevoice/sidevoice-desktop/issues/115)) ([ac69666](https://github.com/sidevoice/sidevoice-desktop/commit/ac69666257415385fddce7fb2cad4cebc9bcaa3a)), closes [#113](https://github.com/sidevoice/sidevoice-desktop/issues/113)
* **tray:** open the room's Settings and drop the bundled settings window ([#133](https://github.com/sidevoice/sidevoice-desktop/issues/133)) ([17c1736](https://github.com/sidevoice/sidevoice-desktop/commit/17c1736898a3767c45ec4bd1433159dde09fbdf0))
* **voice:** start calls with optional input and output halves, update them live and preload models ([#123](https://github.com/sidevoice/sidevoice-desktop/issues/123)) ([efdf994](https://github.com/sidevoice/sidevoice-desktop/commit/efdf99467911b4e933b171999e0e371e9a335d11))


### Bug Fixes

* **app:** release every model and native resource before exit ([#146](https://github.com/sidevoice/sidevoice-desktop/issues/146)) ([9f474f6](https://github.com/sidevoice/sidevoice-desktop/commit/9f474f64eae89c243043b0ba89b35f18f9459c23)), closes [#121](https://github.com/sidevoice/sidevoice-desktop/issues/121)
* **app:** skip C++ static destructors at exit ([#138](https://github.com/sidevoice/sidevoice-desktop/issues/138)) ([d9e1c9b](https://github.com/sidevoice/sidevoice-desktop/commit/d9e1c9b10e08821df478656ad48d883b532f044b)), closes [#121](https://github.com/sidevoice/sidevoice-desktop/issues/121)
* **call-card:** hover and the hand cursor on macOS while the app is inactive ([#141](https://github.com/sidevoice/sidevoice-desktop/issues/141)) ([33e4c40](https://github.com/sidevoice/sidevoice-desktop/commit/33e4c4035b5ad77b0cff180c089bcea6da9a11f0)), closes [#139](https://github.com/sidevoice/sidevoice-desktop/issues/139)
* **call-card:** the Settings preview stays while it is used ([#143](https://github.com/sidevoice/sidevoice-desktop/issues/143)) ([bdbe072](https://github.com/sidevoice/sidevoice-desktop/commit/bdbe0721506e66bd3ed2a91b4fd2491cffb364cd))
* **call-controls:** never hold the card lock while calling window APIs ([#173](https://github.com/sidevoice/sidevoice-desktop/issues/173)) ([c10bf02](https://github.com/sidevoice/sidevoice-desktop/commit/c10bf02377c06f990cdde414d9f698711890fb79))
* **call-controls:** set the card's hand cursor itself while another app is in front ([#167](https://github.com/sidevoice/sidevoice-desktop/issues/167)) ([403225c](https://github.com/sidevoice/sidevoice-desktop/commit/403225ce105b2a58d59a126c225f455794668f0d))
* **call-controls:** the card shows whenever the room is not the app in use, focus lost included ([#162](https://github.com/sidevoice/sidevoice-desktop/issues/162)) ([24630f7](https://github.com/sidevoice/sidevoice-desktop/commit/24630f77ca76030a72f81d04aead56b1a3debaf2))
* **call-controls:** the card's window cannot disagree with its state; its size is checked when it shows ([#161](https://github.com/sidevoice/sidevoice-desktop/issues/161)) ([23dac63](https://github.com/sidevoice/sidevoice-desktop/commit/23dac639a2ab9228d3f2f5627f73bb6cb4e04832))
* **callcard:** show the call card only when the window is not in view ([#114](https://github.com/sidevoice/sidevoice-desktop/issues/114)) ([be1aedd](https://github.com/sidevoice/sidevoice-desktop/commit/be1aedd49cad69bff885341aa378ba408810d0a6))
* **logs:** keep previous sessions' logs and log far less of the call state ([#157](https://github.com/sidevoice/sidevoice-desktop/issues/157)) ([1b2a40f](https://github.com/sidevoice/sidevoice-desktop/commit/1b2a40f09746933c1b34cfb04ffb22accd4c3af6))
* **macos:** ask for the Dock tile at launch and log the policy before any window ([#125](https://github.com/sidevoice/sidevoice-desktop/issues/125)) ([023ff15](https://github.com/sidevoice/sidevoice-desktop/commit/023ff15779510c74f6d82ffe4e215615850100fb))
* **macos:** keep the app in the Dock while the call card stays non-activating ([#118](https://github.com/sidevoice/sidevoice-desktop/issues/118)) ([44029a1](https://github.com/sidevoice/sidevoice-desktop/commit/44029a164fe4f08a7eaa050411823eeec9120771)), closes [#117](https://github.com/sidevoice/sidevoice-desktop/issues/117)
* **macos:** start without the call card on demand and log how macOS sees the app ([#117](https://github.com/sidevoice/sidevoice-desktop/issues/117)) ([#127](https://github.com/sidevoice/sidevoice-desktop/issues/127)) ([996e160](https://github.com/sidevoice/sidevoice-desktop/commit/996e160a18f54c7e754cf58c55e287b03d5528c0))
* **tray:** never hold the icon lock while calling the tray API ([#174](https://github.com/sidevoice/sidevoice-desktop/issues/174)) ([6bc4fd6](https://github.com/sidevoice/sidevoice-desktop/commit/6bc4fd63b97c2fa37b862ae502a3c42b1cec76bf))
* **updater:** drop a stale download or ready update when the nightly feed moves on ([#126](https://github.com/sidevoice/sidevoice-desktop/issues/126)) ([5dd1b5b](https://github.com/sidevoice/sidevoice-desktop/commit/5dd1b5bbd62e03e27b473503fe94057a1179044f))
* **updater:** re-check the feed before installing a downloaded update ([#136](https://github.com/sidevoice/sidevoice-desktop/issues/136)) ([1ada658](https://github.com/sidevoice/sidevoice-desktop/commit/1ada65845391686bacfe1e2c7791402fd6e4160d)), closes [#119](https://github.com/sidevoice/sidevoice-desktop/issues/119)
* **update:** stop the voice call before the process exits ([#122](https://github.com/sidevoice/sidevoice-desktop/issues/122)) ([bf55371](https://github.com/sidevoice/sidevoice-desktop/commit/bf553714674b66f6d8acde2307ce5f1a0f79297b))
* **voice:** accept the turn settings the web now sends (voice 0.7.0) ([#153](https://github.com/sidevoice/sidevoice-desktop/issues/153)) ([d6488da](https://github.com/sidevoice/sidevoice-desktop/commit/d6488da429fb752c51d89950303340965a166a0f))
* **voice:** build the shutdown timeout inside the runtime ([#132](https://github.com/sidevoice/sidevoice-desktop/issues/132)) ([26fcb2c](https://github.com/sidevoice/sidevoice-desktop/commit/26fcb2cb124751061fab9ddff35fce308c9eea22)), closes [#130](https://github.com/sidevoice/sidevoice-desktop/issues/130) [#119](https://github.com/sidevoice/sidevoice-desktop/issues/119)


### Performance Improvements

* **voice:** hand the page the meters only when they change ([#150](https://github.com/sidevoice/sidevoice-desktop/issues/150)) ([364d2cb](https://github.com/sidevoice/sidevoice-desktop/commit/364d2cbaf6ae50936afbe4be38f96b03c8a28e47))


### Miscellaneous Chores

* **web:** pin the web 0.10.1 release ([#183](https://github.com/sidevoice/sidevoice-desktop/issues/183)) ([5ab3b3b](https://github.com/sidevoice/sidevoice-desktop/commit/5ab3b3be5bc74c7ae70553024f9f0cee52eb80c9))

## [0.2.0](https://github.com/sidevoice/sidevoice-desktop/compare/v0.1.0...v0.2.0) (2026-10-18)


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

* **app:** no default Reload context menu ([#107](https://github.com/sidevoice/sidevoice-desktop/issues/107)) ([52ea3e2](https://github.com/sidevoice/sidevoice-desktop/commit/52ea3e2aac07c49d0ab5d8b4df3180d48a0bbaf6))
* **bridge:** the call card changes the audio device through the web's setAudioDevices ([#49](https://github.com/sidevoice/sidevoice-desktop/issues/49)) ([f13c84d](https://github.com/sidevoice/sidevoice-desktop/commit/f13c84d3201ff1450ee4cd541255f753cb825595))
* bundle web pairing port defaults ([#105](https://github.com/sidevoice/sidevoice-desktop/issues/105)) ([3ffb42c](https://github.com/sidevoice/sidevoice-desktop/commit/3ffb42c639d0daa88ee7e9440d1bc11729eb78c4))
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
