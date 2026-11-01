# Changelog

All notable changes to this project are documented in this file.

## [0.2.0] – 2026-11-01

### Added

- Support images in questions and answers ([bb89e25](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/bb89e25e6f83a7927696bbf6a909c3330e3110aa))
- Refuse questions whose content exceeds 10 MiB ([63e430e](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/63e430eb97c1b88afad792e7ab1914304a06c9fd))
- Keep the content of only the question being asked ([59f7f41](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/59f7f41ff977d6b9c2ba7ecda80ea2304116ab38))

### Build and deployment

- CI: Delete artifacts after 2 weeks ([85555ff](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/85555ff67181cdf53ce4797a8e116d203e1f55f0))
- Ignore RUSTSEC-2026-0387 ([bde2412](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/bde24128653350c66bd817817ee68f85669d245f))
- security-update.sh: Change MR title ([09950de](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/09950de807c857f8bd59b47943d88e4213dbc3f8))

## [0.1.1] – 2026-10-25

### Added

- CLI: Add answer time in seconds to question CSV ([5284b04](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/5284b042c8ead62d848e5c6c1034bd8fc632556f))
- Add a button closing the question to the presentation view ([3a1b19b](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/3a1b19b1f621e5ed9698b4ece0341233b73b7e52))
- Add a button revealing the right answers to the presentation view ([5b1bfe4](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/5b1bfe45b0456b8d034eb368cc8eb1aa19539ef5))
- Add --output-dir to quiz ask ([e17a291](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/e17a29149db3fa9c89382cb8e9127db9bef10378))
- Add --room-dir to quiz ask ([b7b9097](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/b7b90975e28d3526b1371046d12b14a8bb233b19))
- Count down --close-after on the presentation view ([51f0a56](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/51f0a566dcadac935adc1eaa0eb65072bda8ca3b))
- Translate web pages to English and allow switching the language ([cbde84c](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/cbde84cde4da7dedee9bdd32f4eb3e9d7ae72fb2))

### Changed

- Update some crates to address security advisories ([fb04c73](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/fb04c73f23ec8b403998f4c3def8a70bb57a4abc))
- Increase rust-version to 1.88 ([e8a1a98](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/e8a1a981aea198961cffa7c4e1986f9630dcb182))
- Ignore the rsa advisory RUSTSEC-2023-0071 in cargo audit ([c2d845a](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/c2d845af3a0c4346d23dd2af99aa2dc0a6033563))
- Require login for the presentation view ([a698d16](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/a698d16a8201951450e1e8215bae95e7b1e59aa0))
- Drop unmaintained basic-toml dependency ([b3cf5a3](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/b3cf5a3e1991fe0696515fc55405b15129b81486))

### Fixed

- Fix security vulnerabilities in dependencies ([a45785e](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/a45785e7887aa5cce10071eed6b630a755b82010))
- Keep the login when a token refresh is interrupted ([df50388](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/df50388cf7919e27cab73bd611dbea7c3bde70ec))

### Documentation

- README: Simplify the first `quiz ask` command ([064081e](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/064081e92aa142f81b40f6820e3a320bc4a0125b))
- README: Highlight the example command ([53673f2](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/53673f2595f4bf8aa8c050175d4825c4f82dc951))
- README: Use English names for the example question files ([6670106](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/6670106fb28abb04f46235fa5d71f6bf248b6343))
- Improve doc of the close subcommand ([4027cc0](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/4027cc0de0840d02b1804d54aa50f729f36e7024))

### Build and deployment

- Add release automation script ([e248818](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/e2488188c173dcdeadfb53fea823834ba23bafef))
- CI: Add a scheduled job proposing fixes of vulnerable crates ([6c91245](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/6c912451075e82ae3657e5a3b13b03c93ef0f0b3))
- CI: Give the security update token only to its job ([cfcaf21](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/cfcaf210272e1aa1c7a4a1718250868b576a71eb))
- CI: Do not build in pipelines started with "Run pipeline" ([20c67ff](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/20c67ff66bdf512da9c9cb47af5c39f0f79f267f))
- CI: Check releases on Alpine 3.22 ([6ec219f](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/6ec219f63b2f335ae3af88ca0d70d20d1d1e9e97))

## [0.1.0] – 2026-10-22

### Added

- Add `quiz ask --shuffle` for a random order of the answers ([6804c40](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/6804c40204e20ea73652b0b16ba3b56f737911f9))
- Add `quiz status` to show the room and the questions kept in it ([a39091d](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/a39091d1b6078cea39ac5f29bc2bf55b2a73444c))
- Add Prometheus metrics at /metrics ([e549b3c](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/e549b3cd51f6ca4b89d2e00ba0db12b91638e83b))
- Add GitLab CI building binaries for Linux, macOS and Windows ([3ba1a03](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/3ba1a037bb5f16173406fa4f1dafc36de419f99b))
- Generate the shell completions while the CLI is compiled ([593eb62](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/593eb62c812f5aa73c600523e8e0373b02524111))
- Read default option values from quiz.toml and ~/.config/quiz/config.toml ([a77275c](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/a77275c38246466d9c7e88fd8872c8f4c8758408))
- Let answers carry points: `[x]`, `[ ]` and `{points}` ([f4b586d](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/f4b586d0b3423fe0fb284a39d4b7dfc37c53be5e))
- Show the teacher how many of the students in the room have answered ([18af371](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/18af37188c357cb915f9ccef313287331bcf9568))
- Link to the source code from the home page ([fe47ade](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/fe47ade24714b9d164646f5a4ee0ff8f892e0244))
- Style markdown tables in questions ([8dadda6](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/8dadda6f1446a891873593351a8b7b2429c8a7e9))
- Server: Authorize teachers by a role in the access token too ([d782799](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/d782799ce73ddb95fecfde704400e09f5e6d3ac8))
- CLI: Save the points of the whole room in a CSV ([f9d1855](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/f9d18550c6352bf95858b25a2d16069563be7f6e))
- Server: Save rooms, questions and answers over a restart with --state-file ([2bf6995](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/2bf699589a5d1de14e09b1fc9ba6d873f9a72838))

### Changed

- Highlight the syntax of code blocks ([cb4f62e](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/cb4f62e0ef1b64085101e1f007a30bc851d7b004))
- Keep students on one URL for the whole lecture ([6afc6da](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/6afc6daabe619b5a095af2b9695488d4c639ef24))
- Let a question be kept off the students' own screens ([d547d45](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/d547d4565b9a8af7e15fbc5156036c2725c5b6ec))
- Keep the room code's style off the code blocks on the projector ([7383d79](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/7383d793507e7a69c73c6da8f4734a8b375e47bb))
- Reveal the right answers only when the teacher presses Enter again ([0cafc55](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/0cafc5564b0ef8d011f5c10fe16bf8d25e779ab8))
- Make the URL below the QR code a link, and version the static files ([4389dbe](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/4389dbee69a5fbd85e6963fe2bd5652e34f9ea50))
- Exit on the first Ctrl-C or SIGTERM ([48421f0](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/48421f0cfff83fac11f64b87e5f12727433053aa))
- Number the questions asked in a room ([bf432ce](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/bf432ce5da77ce3fde51a4bfba517544e001720f))
- Title pages with the question number instead of the file name ([8132cf0](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/8132cf039b897cdf8e70f0a21c7a08497f730226))
- Keep one presentation window open for the whole lecture ([176bb4b](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/176bb4bff09d9e491dd0cd5c0ddbccf703facbdf))
- Shrink the presentation view boxes that do not fit into the window ([e935626](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/e935626f206c17d4e04056bf30fdb3e56833c703))
- Refresh the access token while a question is open ([45d5d9c](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/45d5d9c361bd24c8cebfeb14b1b37dcb96723990))
- Let only the browser that started a login finish it ([ee59b83](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/ee59b837c2e98c1efd1815f13e57745119421a9d))
- Leave HTML comments out of rendered questions ([fca87e7](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/fca87e75eca1c91d1d23987d3fb4754a6403e1d0))
- Exit right after Ctrl-C on the reveal prompt ([46a3193](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/46a3193e98cc220022017b414e2b98ba67fd12a4))
- CLI: Find the configuration directory on Windows too ([36bb434](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/36bb4345cfa8969e99234f5ed5f81846632643b5))
- Follow the answer count over server-sent events instead of polling ([123aacf](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/123aacfd6d8581c1faf4223dc0f68e6fce6ef38e))
- Keep at most 100 questions in a room ([8815435](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/8815435fd533cdd8127d70fbbe279904c1622579))
- CLI: Read Enter even while the event stream is not connected ([35240ce](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/35240ce173a615407fa3c7580c9fb6fa23b4727a))
- CSS: Make inline code visible on the grey question background ([5266858](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/5266858e1503e2bb42d451e3f6c1668e424dbf89))
- Server: Tell students whether their answer was right after reveal ([be6eabc](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/be6eabca6ac2053e7094e789dfc86ae131415315))
- Server: Keep rooms and questions for 4 hours by default ([4267f63](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/4267f63fd0695aada222e04ff41d4373847969e1))
- Server: Show the student's answer in green only once it is revealed ([b763767](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/b763767f4367b402c8977724bdea9cb35ee2a686))

### Fixed

- Fix the security problems found in a review ([43ed8f3](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/43ed8f3c6b8a599f25b1be25d4f5d557b4393802))
- Fix project repo URL ([f7f9eeb](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/f7f9eebb4d464f19ffbb5c8a864a755d2ce7dcbe))
- Drop the shebang line of an executable question ([3387de2](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/3387de2bd69b493bf5907bf22a7a9dbe85b4b438))
- Call the API with access tokens instead of ID tokens ([494e413](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/494e413fc25a32edb49ae12a491dfe64150ca8a9))
- CLI: Do not panic in `quiz ask` when the standard input is closed ([5c2f1cb](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/5c2f1cba74ffb58e6efd9fb2198d6234dd784968))
- Server: Ask proxies not to buffer the event streams ([4560cc4](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/4560cc40f8f47e68ee30bae413458036a65d1817))
- Server: Treat an empty RUST_LOG as unset ([b6e8f9c](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/b6e8f9cae58d9f015fce8f2209e0fc20e17aae03))

### Documentation

- README: Better description of "quiz ask" switches ([eb13635](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/eb13635f5c879d86f8178162265e577fcddf0ea1))
- README: Reorder and update some sections ([b02aef2](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/b02aef2059e3d453026dba934652ce25029c86ae))
- README: Add Getting started section ([e3851df](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/e3851dff7d78f0ebb29ee287aa6969767657b1b5))
- README: Mention that the close-after value is in seconds ([6b5a23a](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/6b5a23ab5a2d22cf9b1953ca6b6b2bc9218c153f))

### Build and deployment

- CI: Put the binaries at the root of the artifact archive ([406fdbf](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/406fdbff303e21f1d2b89c4f1c71be3c45f7321a))
- Nix: Move the package and the NixOS module to nix/ ([3bfae45](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/3bfae457af97d43d0c64b25eff0599461524f0ce))
- Nix: Add flake checks for the package, the dev shell and the module ([247272c](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/247272cc3a871c803c85716bf188d024c3ae5c19))
- CI: Add release automation ([2bdbc3f](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/2bdbc3f0f2d7823eb54a589b7f2346a1796fa4a8))
- Let the NixOS module set the client ID of the teacher CLI ([d84fd16](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/d84fd1642c8080ff5bfe806ffc503f88124f34fd))
- Let the NixOS module set the redirect URI of the teacher CLI ([47db642](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/47db64208854d4542e7a82f0e04c8b5aab97f759))
- Let the NixOS module set the OIDC scopes ([8bbd2c9](https://gitlab.fel.cvut.cz/sojkam1/quiz/-/commit/8bbd2c9e114239122976e6b0623fafbf2b9b34c6))

