# Changelog

## [0.3.0](https://github.com/laurigates/shore-recon/compare/v0.2.0...v0.3.0) (2026-10-30)


### Features

* check tools before the first stage and add a CPU OpenMVS image ([#88](https://github.com/laurigates/shore-recon/issues/88)) ([3f58dc1](https://github.com/laurigates/shore-recon/commit/3f58dc14b99d9c90113bbb67d29d4d8b4c9004d9))
* show run progress, estimates from history and a summary ([#86](https://github.com/laurigates/shore-recon/issues/86)) ([1d3ce4e](https://github.com/laurigates/shore-recon/commit/1d3ce4eee1d7c926ad1a290e012871326654b745))
* **tools:** run every stage but the videos locally by default ([#91](https://github.com/laurigates/shore-recon/issues/91)) ([079d5ca](https://github.com/laurigates/shore-recon/commit/079d5ca547f30914f60c58e97afc5e102073402e))


### Bug Fixes

* **containers:** pin the Python images with uv projects ([#84](https://github.com/laurigates/shore-recon/issues/84)) ([bac5c8e](https://github.com/laurigates/shore-recon/commit/bac5c8e99f30123372883cbc074cbacb575871d7)), closes [#83](https://github.com/laurigates/shore-recon/issues/83)

## [0.2.0](https://github.com/laurigates/shore-recon/compare/v0.1.0...v0.2.0) (2026-10-29)


### Features

* **cli:** shoreline-recon &lt;video&gt; builds the models in one command ([fbc3ab8](https://github.com/laurigates/shore-recon/commit/fbc3ab8c47d6173b62e8dc30f16cf09580e6aac4))
* **core:** built-in defaults and user config layers ([bd0633f](https://github.com/laurigates/shore-recon/commit/bd0633f95c4869aed6a91c412ebd8742b0c061eb))
* make the video stage optional with [export] videos ([9497231](https://github.com/laurigates/shore-recon/commit/94972316b8a35bd477bfb2b62ab89d47c27eae5a))

## 0.1.0 (2026-10-29)


### Features

* **containers:** add Brush and OpenMVS images for the GPU host ([#35](https://github.com/laurigates/shore-recon/issues/35)) ([072a756](https://github.com/laurigates/shore-recon/commit/072a756759f385562693c4edece0fdc8435f336d))
* **containers:** add the render image for showcase videos ([#36](https://github.com/laurigates/shore-recon/issues/36)) ([87f0fd4](https://github.com/laurigates/shore-recon/commit/87f0fd402b797f28fadf21dac14c6dc623eb9029))
* **containers:** add the segmentation image for the masks stage ([#25](https://github.com/laurigates/shore-recon/issues/25)) ([226faf6](https://github.com/laurigates/shore-recon/commit/226faf6fe172567923390c42eb9fbf565c36ecc7))
* **core:** add stage cache, manifest and pipeline runner ([#22](https://github.com/laurigates/shore-recon/issues/22)) ([1ab83a5](https://github.com/laurigates/shore-recon/commit/1ab83a57068332f24d898774511d7a7c564ccf80))
* **core:** add ToolRunner with native, podman and docker runtimes ([#19](https://github.com/laurigates/shore-recon/issues/19)) ([c543596](https://github.com/laurigates/shore-recon/commit/c543596efa7d7cf34e48d128ec771662f7d0e1f9))
* **export:** gather the v1 deliverables and write report.json ([#54](https://github.com/laurigates/shore-recon/issues/54)) ([4875ccf](https://github.com/laurigates/shore-recon/commit/4875ccfb5767436ce9755b2c677d3e20dc610eff))
* **frames:** extract and blur-filter video frames; wire up run ([#24](https://github.com/laurigates/shore-recon/issues/24)) ([2cccd60](https://github.com/laurigates/shore-recon/commit/2cccd608ecef1ce75b1e4d74ae7218cb733a95dc))
* make the capture-to-deliverables process repeatable for new videos ([#62](https://github.com/laurigates/shore-recon/issues/62)) ([da5273f](https://github.com/laurigates/shore-recon/commit/da5273f68f9e2f4cd05f33c69a61049abdbca075))
* **masks:** add the masks stage on the segmentation container ([#26](https://github.com/laurigates/shore-recon/issues/26)) ([ed20391](https://github.com/laurigates/shore-recon/commit/ed20391d0b42f207dffed7b44dd920fb9144df67))
* **post:** add the post stage: water plane, mesh cleanup, splat cull ([#52](https://github.com/laurigates/shore-recon/issues/52)) ([eb93a63](https://github.com/laurigates/shore-recon/commit/eb93a637dc334c5d35b985ef2863f85a3172d8d1))
* **sfm:** add the sfm stage and an overnight batch recipe ([#34](https://github.com/laurigates/shore-recon/issues/34)) ([d338a3e](https://github.com/laurigates/shore-recon/commit/d338a3ee46bc8e8006cac71aceb76d0661ad7032))
* **sfm:** read COLMAP binary models and compute the sfm gates ([#32](https://github.com/laurigates/shore-recon/issues/32)) ([014cee4](https://github.com/laurigates/shore-recon/commit/014cee43ccd461354530f502a21de25c76ecdb72))
* **spike:** M0 spike tooling, GPU-host recipes and ADR-0002 ([#3](https://github.com/laurigates/shore-recon/issues/3)) ([315e752](https://github.com/laurigates/shore-recon/commit/315e7529e35a39c66400fe1c172f9b26a9a99f66))
* **splat,dense:** add the Brush splat and OpenMVS dense stages ([#37](https://github.com/laurigates/shore-recon/issues/37)) ([659584a](https://github.com/laurigates/shore-recon/commit/659584a51161b87892a31da480762053729616ac))
* **tools:** add the Apptainer runtime arm ([#30](https://github.com/laurigates/shore-recon/issues/30)) ([ce21e95](https://github.com/laurigates/shore-recon/commit/ce21e957264be0fe4d4bd7a091494a2b6e913381))
* **tools:** run container tools on a remote host over ssh ([#31](https://github.com/laurigates/shore-recon/issues/31)) ([3156a92](https://github.com/laurigates/shore-recon/commit/3156a92f9e74e81c0bab61f85fc32b4d004dd478))
* **video:** add a timeline bar to the demo video ([#58](https://github.com/laurigates/shore-recon/issues/58)) ([fef7cd2](https://github.com/laurigates/shore-recon/commit/fef7cd21a8b8134d66730b11eb960f4aa6141ff9))
* **video:** add the showcase video stage ([#38](https://github.com/laurigates/shore-recon/issues/38)) ([78a7d50](https://github.com/laurigates/shore-recon/commit/78a7d5039a98c1e660bddec21453c17bb256327d))
* **video:** assemble the v1 demo video from the export deliverables ([#50](https://github.com/laurigates/shore-recon/issues/50)) ([12d49d3](https://github.com/laurigates/shore-recon/commit/12d49d3b37940dec95bc643d097488ebf9db9852))
* **video:** redesign the timeline bar for long, many-chapter videos ([#61](https://github.com/laurigates/shore-recon/issues/61)) ([0036e95](https://github.com/laurigates/shore-recon/commit/0036e95e5757a197d75d9b015aed19c0b88661e8))
* **video:** render a clip per processing step ([#60](https://github.com/laurigates/shore-recon/issues/60)) ([6e8b5c3](https://github.com/laurigates/shore-recon/commit/6e8b5c320c49bf30df0fc549b6641ccf1aff4ac6))
* **video:** render the cleaned splat and the mesh with its water ([#53](https://github.com/laurigates/shore-recon/issues/53)) ([9d2c534](https://github.com/laurigates/shore-recon/commit/9d2c534ee8d582780791c7cd0ac2781123c57ef8))
* **video:** render the drone path as well as the arc ([#59](https://github.com/laurigates/shore-recon/issues/59)) ([f8cadde](https://github.com/laurigates/shore-recon/commit/f8cadded5e03b64cbdf15a6242fef27c8c926664))
* **viewer:** add a local web viewer for the export outputs ([#67](https://github.com/laurigates/shore-recon/issues/67)) ([271c2c3](https://github.com/laurigates/shore-recon/commit/271c2c3804e59a5ef67d53f59eec8e2dc3871f60))


### Bug Fixes

* **configs:** point reposalmi-vartiosaari at its video capture ([#2](https://github.com/laurigates/shore-recon/issues/2)) ([ef8a8d9](https://github.com/laurigates/shore-recon/commit/ef8a8d98dfe87fbd409b9bc2d3f83a149ea956a5))
* **containers:** build OpenMVS with upstream's black-texture fix ([#51](https://github.com/laurigates/shore-recon/issues/51)) ([6ae7974](https://github.com/laurigates/shore-recon/commit/6ae7974eee0fb641e3bec242ee3ba43262743b93))
* **post:** keep texture seams when decimating ([#56](https://github.com/laurigates/shore-recon/issues/56)) ([c148354](https://github.com/laurigates/shore-recon/commit/c148354a28f7dfee9b9d34e695988f79d490b8ce))
* **render:** put torch 2.13.0 in the lock and move the image to CUDA 13.0 ([#65](https://github.com/laurigates/shore-recon/issues/65)) ([8a49e9e](https://github.com/laurigates/shore-recon/commit/8a49e9e38cb90911d5201aff4b374bb18a876548))
* **tools:** keep a remote tool's log out of the mount sync ([#33](https://github.com/laurigates/shore-recon/issues/33)) ([a45cd24](https://github.com/laurigates/shore-recon/commit/a45cd24efb9ca778bd0b1f8bf64b498198f4b170))
* **tools:** run docker containers as the pipeline's user ([#66](https://github.com/laurigates/shore-recon/issues/66)) ([c045049](https://github.com/laurigates/shore-recon/commit/c045049553b31722c3bdbdbb2419c98df57d720a))
* **viewer:** draw the water plane with the video stage's shader ([#68](https://github.com/laurigates/shore-recon/issues/68)) ([39ea305](https://github.com/laurigates/shore-recon/commit/39ea305e4243879e0317236d560f2cabb772da63))
