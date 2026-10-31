# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](http://keepachangelog.com/en/1.0.0/)
and this project adheres to [Semantic Versioning](http://semver.org/spec/v2.0.0.html).

<!-- insertion marker -->
## [v0.6.1](https://github.com/fgagarin/framecover/releases/tag/v0.6.1) - 2026-10-31

<small>[Compare with v0.6.0](https://github.com/fgagarin/framecover/compare/v0.6.0...v0.6.1)</small>

### Bug Fixes

- keep portrait video upright and make the new frame Android's thumbnail ([f6fa02d](https://github.com/fgagarin/framecover/commit/f6fa02d00dfafcf8cc896d82313aee3f8282f2a9) by Fedor Gagarin). Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
- initialise x265's B-frame delay before one-frame encodes ([0171428](https://github.com/fgagarin/framecover/commit/0171428b98141b7d160bfc9349a7fb9dc733e876) by Fedor Gagarin). Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>

## [v0.6.0](https://github.com/fgagarin/framecover/releases/tag/v0.6.0) - 2026-10-30

<small>[Compare with v0.5.0](https://github.com/fgagarin/framecover/compare/v0.5.0...v0.6.0)</small>

### Features

- save full-range video without re-encoding; request support for re-encoded formats ([089d17a](https://github.com/fgagarin/framecover/commit/089d17ab5a2a592d133e5c892f25d8de862abc36) by Fedor Gagarin). Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>

## [v0.5.0](https://github.com/fgagarin/framecover/releases/tag/v0.5.0) - 2026-10-30

<small>[Compare with v0.4.0](https://github.com/fgagarin/framecover/compare/v0.4.0...v0.5.0)</small>

### Features

- Android download card ([2ab5722](https://github.com/fgagarin/framecover/commit/2ab5722c176757d3a7301319b2259e6ac50da766) by Fedor Gagarin). Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>

### Continuous Integration

- build, sign and publish the Android APK ([e4bcd92](https://github.com/fgagarin/framecover/commit/e4bcd92dc090ad69285ee2853b1586e4dce40732) by Fedor Gagarin). Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>

## [v0.4.0](https://github.com/fgagarin/framecover/releases/tag/v0.4.0) - 2026-10-29

<small>[Compare with v0.3.1](https://github.com/fgagarin/framecover/compare/v0.3.1...v0.4.0)</small>

### Features

- Android app with FFmpeg linked in-process ([05e17e1](https://github.com/fgagarin/framecover/commit/05e17e1e7bf8d5cc849f85559635e822b43e98b8) by Fedor Gagarin). Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>

### Bug Fixes

- clippy overlapping ranges and the FFmpeg build script mode ([c6cb866](https://github.com/fgagarin/framecover/commit/c6cb8660e46cfc15030b38a4999dfa8811c55296) by Fedor Gagarin). Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>

## [v0.3.1](https://github.com/fgagarin/framecover/releases/tag/v0.3.1) - 2026-10-26

<small>[Compare with v0.3.0](https://github.com/fgagarin/framecover/compare/v0.3.0...v0.3.1)</small>

### Bug Fixes

- link liblzma statically so the macOS app starts on other Macs ([a44e86f](https://github.com/fgagarin/framecover/commit/a44e86f246644734f77f446b693f9e8d90c48558) by Fedor Gagarin). Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>

## [v0.3.0](https://github.com/fgagarin/framecover/releases/tag/v0.3.0) - 2026-10-25

<small>[Compare with v0.2.0](https://github.com/fgagarin/framecover/compare/v0.2.0...v0.3.0)</small>

### Features

- console launcher framecover.com for the Windows CLI ([a3b0f03](https://github.com/fgagarin/framecover/commit/a3b0f03aab4a5394cbf6b88f5966ec2e664cdbdc) by Fedor Gagarin). Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
- command-line interface (set, info, convert, extract) ([13a0f46](https://github.com/fgagarin/framecover/commit/13a0f46ecb788175282f43cf8c59a3e8d5eea4ef) by Fedor Gagarin). Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
- HDR and 10-bit sources on the fast path ([4b7c19d](https://github.com/fgagarin/framecover/commit/4b7c19d90df3cb6a8fb6a9698b792f0f6ced68cb) by Fedor Gagarin). Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
- use an image file as the thumbnail (fit or crop to frame) ([5cef629](https://github.com/fgagarin/framecover/commit/5cef6292291fbc4bb7df21456684a5014fe710a7) by Fedor Gagarin). Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
- load still images and fit or crop them into a video frame ([a8a3798](https://github.com/fgagarin/framecover/commit/a8a37986d3b0062cbcbb52626eae63cddf00fbed) by Fedor Gagarin). Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>

## [v0.2.0](https://github.com/fgagarin/framecover/releases/tag/v0.2.0) - 2026-10-25

<small>[Compare with v0.1.0](https://github.com/fgagarin/framecover/compare/v0.1.0...v0.2.0)</small>

### Features

- 1 GiB frame cache for videos above 1080p ([758e953](https://github.com/fgagarin/framecover/commit/758e953cd9dc2b8324dcf50225357d6ec0a9c213) by Fedor Gagarin). Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
- persistent ffmpeg decoding session for smooth scrubbing ([c189285](https://github.com/fgagarin/framecover/commit/c189285131aab1f1c1ca0b96006a5bd63df206a3) by Fedor Gagarin). Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
- bring back the update status dot ([e9d3334](https://github.com/fgagarin/framecover/commit/e9d3334c80f514741c962a4129697b44ec8e4e2e) by Fedor Gagarin). Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
- align footer items and show update status as plain text ([326a408](https://github.com/fgagarin/framecover/commit/326a408c08007d9d602e0a31dae314076bd2320a) by Fedor Gagarin). Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
- make the project page link grey with hover highlight ([f070b0d](https://github.com/fgagarin/framecover/commit/f070b0d77a84b350ba14c49ff9caaa3412e35838) by Fedor Gagarin). Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
- live frame updates while scrubbing; simpler footer ([04d597f](https://github.com/fgagarin/framecover/commit/04d597f8a422b173938b1753149ffbe32bb47c99) by Fedor Gagarin). Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
- show the writer's success note in the save tooltip ([4aff5db](https://github.com/fgagarin/framecover/commit/4aff5db0665f4a6d4c10276318b3729a689bfe02) by Fedor Gagarin). Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
- embed compressed FFmpeg in the Windows executable ([fb3266a](https://github.com/fgagarin/framecover/commit/fb3266a38848b848f018dfdf6d6b9ac755da6f8a) by Fedor Gagarin). Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
- wire the GUI to the core services ([5b45546](https://github.com/fgagarin/framecover/commit/5b455469482ab89df8f52e10ea8c269161bbe148) by Fedor Gagarin). Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
- Slint GUI port ([21c8293](https://github.com/fgagarin/framecover/commit/21c82931fbca4955a7609dd2c3f22bf460c11eec) by Fedor Gagarin). Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
- platform integration, update checker, xtask and Rust CI ([e92d50d](https://github.com/fgagarin/framecover/commit/e92d50de41bb904a50037905f6052d2a2a5cd89f) by Fedor Gagarin). Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
- thumbnail writer, safe save-copy and MP4 converter ([73b53c6](https://github.com/fgagarin/framecover/commit/73b53c6c2e671214f0f8fbf7da965957f2ef3ba7) by Fedor Gagarin). Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
- ffprobe loader and ffmpeg frame extractor ([b447e14](https://github.com/fgagarin/framecover/commit/b447e1411ab387e25807f43a5d6f82fda3d14474) by Fedor Gagarin). Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>

### Bug Fixes

- only define the Windows manifest when building on Windows ([b3cb21a](https://github.com/fgagarin/framecover/commit/b3cb21adf0ba26ec36b33d2d136154a9a67f08aa) by Fedor Gagarin). Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
- keep the window on screen when it grows after opening a video ([6a6e122](https://github.com/fgagarin/framecover/commit/6a6e122707dfd291589881a6763f49b7ffe705f7) by Fedor Gagarin). Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
- concat fast path for MP4s whose video is not the first track ([8cfb6cd](https://github.com/fgagarin/framecover/commit/8cfb6cd167cd9e2de58d0048494baae0d4ed934d) by Fedor Gagarin). Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
- give MOV the concat fast path and keep it QuickTime ([0cf53d2](https://github.com/fgagarin/framecover/commit/0cf53d2346a5195b8e16e37dc02f87d019f990ae) by Fedor Gagarin). Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
- insert the preview in the full re-encode instead of overwriting frame 0 ([5a17f07](https://github.com/fgagarin/framecover/commit/5a17f07bab8e8300115377e904747b6da60f88fe) by Fedor Gagarin). Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
- skip the converter's remux when it cannot be concat-compatible ([0f6bd5f](https://github.com/fgagarin/framecover/commit/0f6bd5fb7e53289195eebe008544c37aeab1ecb7) by Fedor Gagarin). Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
- accurate notes for containers without cover art ([61a1b88](https://github.com/fgagarin/framecover/commit/61a1b88617da2b514b341b7e579b8cb5550507ad) by Fedor Gagarin). Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
- keep all tracks when re-muxing MKV and AVI ([439093c](https://github.com/fgagarin/framecover/commit/439093c970d9dc510a0b881e3bf57f97642afdfd) by Fedor Gagarin). Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
- keep source frame rate after the concat fast path ([860d6ff](https://github.com/fgagarin/framecover/commit/860d6ff4f33a8ab87c2fa06314986e91f1479360) by Fedor Gagarin). Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
- encode cover JPEGs with 4:2:0 chroma like Pillow ([154d2c0](https://github.com/fgagarin/framecover/commit/154d2c0c7f99e997dcd366fe4c286939de24b39b) by Fedor Gagarin). Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
- quote apostrophes correctly in concat manifests ([3b2ebc3](https://github.com/fgagarin/framecover/commit/3b2ebc3d6279b0800e89d47bbdc5877893be4794) by Fedor Gagarin). Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>

### Code Refactoring

- replace the Python implementation with the Rust port ([3369d51](https://github.com/fgagarin/framecover/commit/3369d5176390272e64c48a8bd3591dd02c758982) by Fedor Gagarin). Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>

### Build

- render changelog dates in Asia/Tokyo like git-changelog did ([33e0c5c](https://github.com/fgagarin/framecover/commit/33e0c5cb5f678b40c4d64bfa3ea2cd09b56ac04b) by Fedor Gagarin). Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
- ship FFmpeg GPL notice and license text in release bundles ([299adf6](https://github.com/fgagarin/framecover/commit/299adf6ae8cf38a400a765e61f5e73a73b645e7a) by Fedor Gagarin). Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>

### Docs

- rewrite README for the Rust version ([baaee47](https://github.com/fgagarin/framecover/commit/baaee47d147d6f4e93f1a1ee3aa020ea87810ce7) by Fedor Gagarin). Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>

## [v0.1.0](https://github.com/fgagarin/framecover/releases/tag/v0.1.0) - 2026-10-24

<small>[Compare with first commit](https://github.com/fgagarin/framecover/compare/df45912de7bb89ec4bf27f1b8063f3feb5c7a8ca...v0.1.0)</small>

### Features

- add OS icons to download cards ([39eda74](https://github.com/fgagarin/framecover/commit/39eda7447c5c22fb833eb8ee480724a091886304) by Fedor Gagarin). Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>
- check for updates on startup and from footer ([8774475](https://github.com/fgagarin/framecover/commit/87744755bbaaa27f7ecb7ddb79962095d0123a58) by Fedor Gagarin). Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>
- explain video errors and offer recovery actions ([4767886](https://github.com/fgagarin/framecover/commit/476788618f17ce78041b2b16d2826e1e9c148b57) by Fedor Gagarin). Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>
- redesign start screen with step guide and persistent footer ([a695b9a](https://github.com/fgagarin/framecover/commit/a695b9ab381b7064bd963a0e07b805ef1d321a7b) by Fedor Gagarin). Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>
- add copy-first saving and original overwrite choices ([c1fff27](https://github.com/fgagarin/framecover/commit/c1fff279c8ad50e1a733fafc9cf28ad4a3ea0d13) by copilot-swe-agent[bot]). Co-authored-by: fgagarin <922530+fgagarin@users.noreply.github.com>
- add Russian landing page and GitHub Pages deployment ([d011263](https://github.com/fgagarin/framecover/commit/d0112633029d644fc99d4172833e006bfd59d9fb) by copilot-swe-agent[bot]). Co-authored-by: fgagarin <922530+fgagarin@users.noreply.github.com>
- add GitHub Actions workflow for release automation ([be08176](https://github.com/fgagarin/framecover/commit/be08176d3af45b201719372ec25a337ba23a647c) by Fedor Gagarin).
- Enhance video conversion and thumbnail workflow with concat compatibility checks ([5eec91d](https://github.com/fgagarin/framecover/commit/5eec91d45a5994e7c2531b1c9fdf236a2bfbacd1) by Fedor Gagarin).
- Implement video format conversion to MP4 with FFmpeg support ([5d344cc](https://github.com/fgagarin/framecover/commit/5d344cc542d468dae665996d6fc52492e2a8af62) by Fedor Gagarin).
- Enhance video processing by adding video track timescale handling in thumbnail writer ([7d2f69a](https://github.com/fgagarin/framecover/commit/7d2f69a3dac4fb00b7f6b6368f35a2bc23a559a8) by Fedor Gagarin).
- Add support for replacing existing MP4 previews without re-encoding ([ffa2f0e](https://github.com/fgagarin/framecover/commit/ffa2f0e72a16dd4d7d2174e63eb117c82223c4a1) by Fedor Gagarin).
- Implement re-encoding reason handling and UI updates for video processing ([6533767](https://github.com/fgagarin/framecover/commit/65337672be0a97287dc0e257693ef52d4d77bfb1) by Fedor Gagarin).
- Add duration shift handling for frame embedding in video files ([919ff38](https://github.com/fgagarin/framecover/commit/919ff38be3e3d7c21d9d946fe2b6289db444f7a1) by Fedor Gagarin).
- Implement progress tracking for thumbnail saving process in UI and backend ([41f9cfa](https://github.com/fgagarin/framecover/commit/41f9cfab4679526d28ae33a89ad5100200953e63) by Fedor Gagarin).
- Enhance thumbnail writer to run ffmpeg without console window on Windows ([fb87431](https://github.com/fgagarin/framecover/commit/fb87431e27c6d9e9a38813b6965f645183b7e024) by Fedor Gagarin).
- Add application icons and enhance UI for thumbnail saving functionality ([61e7556](https://github.com/fgagarin/framecover/commit/61e7556038d51a0de9c6fefcb0239292da3a00d8) by Fedor Gagarin).
- Implement FilmstripWidget for adaptive video frame display and integrate with MainWindow ([0ab074a](https://github.com/fgagarin/framecover/commit/0ab074aa08c0c4116e1c445c9928f263220ebf56) by Fedor Gagarin).
- Add support for saving and restoring thumbnail source metadata in MP4 files ([e26d69d](https://github.com/fgagarin/framecover/commit/e26d69d64e420e895a23c4ea74597e69edeed40f) by Fedor Gagarin).
- Update UI to hide/show video controls and improve preview widget scaling ([b543bef](https://github.com/fgagarin/framecover/commit/b543befc1e3b239ec89b01b3e5b5fb8259557148) by Fedor Gagarin).
- Enhance frame extraction logic and UI to support timestamp display for selected frames ([77ed23d](https://github.com/fgagarin/framecover/commit/77ed23d77a8301c54007d6bf7c9efa3ea1989e75) by Fedor Gagarin).
- Enhance atomic_replace to optionally skip copying existing target and update thumbnail writer to embed a preview frame ([b0e14c3](https://github.com/fgagarin/framecover/commit/b0e14c327383ef4b158fe3a7b37c41fade39e48a) by Fedor Gagarin).
- Add support for embedding MP4 cover art and enhance video frame handling ([5cad832](https://github.com/fgagarin/framecover/commit/5cad8329dc27d714e7765a13449d58853a6fb3a0) by Fedor Gagarin).
- Add implementation of Video Thumbnail Changer ([11d6db6](https://github.com/fgagarin/framecover/commit/11d6db61243be9e430d1024ee9b86ae9e9df5b47) by Fedor Gagarin).
- initial project setup and feature spec 001-change-video-thumbnail ([df45912](https://github.com/fgagarin/framecover/commit/df45912de7bb89ec4bf27f1b8063f3feb5c7a8ca) by Fedor Gagarin).

### Bug Fixes

- improve controls and bundle update certificates ([88ccf49](https://github.com/fgagarin/framecover/commit/88ccf498ce15cdbe7d35800ef0acf2848c3304ec) by fgagarin). Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>
- preserve concurrent outputs and support filesystems without hardlinks ([2a2f658](https://github.com/fgagarin/framecover/commit/2a2f6583b7dc1564697d8bd8cab0b8b8ae308550) by copilot-swe-agent[bot]). Co-authored-by: fgagarin <922530+fgagarin@users.noreply.github.com>
- translate landing page to English ([065315d](https://github.com/fgagarin/framecover/commit/065315dfb516e6061d7fd05e5bc81140566662e5) by copilot-swe-agent[bot]). Co-authored-by: fgagarin <922530+fgagarin@users.noreply.github.com>
- use distro FFmpeg for Linux conversion tests ([acceb04](https://github.com/fgagarin/framecover/commit/acceb049a05494a803e7bcf9773b94f668c4b278) by Fedor Gagarin). Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>
- restore CI tests and frame extraction performance ([83707c9](https://github.com/fgagarin/framecover/commit/83707c9ccd2754971ffbf4d14563a7d705083e78) by Fedor Gagarin). Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>
- Adjust frame rate validation logic in FormatDispatchThumbnailWriter ([113972e](https://github.com/fgagarin/framecover/commit/113972ec207691b12a3711164ef6d91a44cae43a) by Fedor Gagarin).
- Error when saving MP4 with a timecode track ([f4ba22d](https://github.com/fgagarin/framecover/commit/f4ba22da0a8d6d8dbbc8ed1746c34d380cfd8942) by Fedor Gagarin).

### Performance Improvements

- replace full PySide6 with Essentials ([186ae9e](https://github.com/fgagarin/framecover/commit/186ae9eeded9f00f60b9b43fc169bba1e8a43968) by Fedor Gagarin). Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>
- collect only required Qt runtime resources ([fccbac4](https://github.com/fgagarin/framecover/commit/fccbac4c1d91e99390dbf6da67ea7381eed6c054) by Fedor Gagarin). Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>

### Code Refactoring

- rename the app to FrameCover ([fe36367](https://github.com/fgagarin/framecover/commit/fe36367755b6d2853519d6aff26409acea1b898c) by Fedor Gagarin). Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>

### Build

- upgrade runtime and development dependencies ([8d72907](https://github.com/fgagarin/framecover/commit/8d72907e375810a4afa6769180da060bcda97f0c) by Fedor Gagarin). Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>

### Continuous Integration

- annotate Windows pytest failures ([db10fbd](https://github.com/fgagarin/framecover/commit/db10fbd1f7ae691927ccaed41e136d5145a52802) by Fedor Gagarin). Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>
- upgrade actions to latest (checkout@v6, setup-python@v6, upload-artifact@v7) ([4b44897](https://github.com/fgagarin/framecover/commit/4b44897c1aeac0aabf1d811098e6ae10d9ad728b) by Fedor Gagarin).
- opt into Node.js 24 to silence deprecation warnings ([c526b8d](https://github.com/fgagarin/framecover/commit/c526b8d2bd95572053e0e80a5fffe2406d260956) by Fedor Gagarin).
- enforce coverage threshold on linux only ([8b00c7c](https://github.com/fgagarin/framecover/commit/8b00c7cded2471eac09555ca875dffbe68ee30d5) by Fedor Gagarin).
- Fix text (ubuntu-latest) ImportError: libEGL.so.1: cannot open shared object file: No such file or directory. ([be4e54a](https://github.com/fgagarin/framecover/commit/be4e54affd5f4824cda9105b5c7d3fb5548c81e3) by Fedor Gagarin).
- Add uv and ci workflow for github ([411aa4b](https://github.com/fgagarin/framecover/commit/411aa4be22f4395ac2fcb561ae84336a7210be9c) by Fedor Gagarin).

### Docs

- require Conventional Commits in agent instructions ([d1e3c70](https://github.com/fgagarin/framecover/commit/d1e3c700f9156ea60b884014da43c85c5002dfc5) by Fedor Gagarin). Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>
- Add screenshots to readme. ([6d8bb51](https://github.com/fgagarin/framecover/commit/6d8bb51058136ee64be6dfe9d456eae80adaeb34) by Fedor Gagarin).
- Explain main usage in readme ([eafa786](https://github.com/fgagarin/framecover/commit/eafa7866e6d12597d038a4a867b08f871896887a) by Fedor Gagarin).
