# Changelog

## [0.1.1](https://github.com/anatolykoptev/ox-codes/compare/v0.1.0...v0.1.1) (2026-07-15)


### Added

* add apply mode to POST /rewrite ([e58409c](https://github.com/anatolykoptev/ox-codes/commit/e58409ca6e56660b500889ad03b5a2baf70ee2d2))
* add format=markdown to expand output ([0c611cc](https://github.com/anatolykoptev/ox-codes/commit/0c611ccad3247b908406c2374da0bf9e291773bd))
* add TypeScript/JavaScript and Rust dataflow language queries ([0bc2220](https://github.com/anatolykoptev/ox-codes/commit/0bc2220ff3597a7f358d2233eb8ef19ac9623f9b))
* cross-request parsed scope cache for POST /search/scoped ([#22](https://github.com/anatolykoptev/ox-codes/issues/22)) ([a7d57b0](https://github.com/anatolykoptev/ox-codes/commit/a7d57b0c3a4c2f4d0e7df90b82c66a8b63865ab1))
* **docker:** add sccache + mold для signal-grade build cache ([#8](https://github.com/anatolykoptev/ox-codes/issues/8)) ([4f0e989](https://github.com/anatolykoptev/ox-codes/commit/4f0e989818c0122ca06c582642123f6614f4c28c))
* harden scope and dataflow caches ([#28](https://github.com/anatolykoptev/ox-codes/issues/28)) ([e4076bf](https://github.com/anatolykoptev/ox-codes/commit/e4076bfc357a8e9895173f057603629b3ef22589)), closes [#23](https://github.com/anatolykoptev/ox-codes/issues/23) [#24](https://github.com/anatolykoptev/ox-codes/issues/24)
* **langs+dataflow:** Svelte template edges in callgraph ([#14](https://github.com/anatolykoptev/ox-codes/issues/14)) ([45f44af](https://github.com/anatolykoptev/ox-codes/commit/45f44af5239d41950cdf4e8263845341ea1f08ba))
* Phase 4+5+6 — TS/JS/Rust dataflow queries, rewrite write mode, markdown format ([36432a6](https://github.com/anatolykoptev/ox-codes/commit/36432a60d6f56c1691d1e0c34f4a1e3b863874b7))
* **security:** add cargo-deny config ([#7](https://github.com/anatolykoptev/ox-codes/issues/7)) ([5fa0cdb](https://github.com/anatolykoptev/ox-codes/commit/5fa0cdbe22e96c80ccd8a936791d85ad749283fe))


### Fixed

* atomic rewrite via tempfile, strengthen Rust query tests ([29c25e7](https://github.com/anatolykoptev/ox-codes/commit/29c25e77fb8013c73057b8b02a977235a74b3530))
* **dataflow:** serde default + blocking semaphore + svelte doc (Issue [#12](https://github.com/anatolykoptev/ox-codes/issues/12)) ([#13](https://github.com/anatolykoptev/ox-codes/issues/13)) ([ac66385](https://github.com/anatolykoptev/ox-codes/commit/ac66385da623163a12dedd8b1f717eb4209f58bf))
* Go method call patterns with $RECV.Method($$$) now match correctly ([353dfb2](https://github.com/anatolykoptev/ox-codes/commit/353dfb2502aafc91dcfe3f414707f72640fd3ed3))
* **langs+dataflow:** .svelte support + /dataflow/analyze file-cap and timeout ([#11](https://github.com/anatolykoptev/ox-codes/issues/11)) ([858cc47](https://github.com/anatolykoptev/ox-codes/commit/858cc477bc692eac6d03976ddf3d6fa980d82aab))
* **release:** use simple release-type + fleet-standard config ([#30](https://github.com/anatolykoptev/ox-codes/issues/30)) ([e9c1992](https://github.com/anatolykoptev/ox-codes/commit/e9c199209d78f5369a600baaded36b0273d1c3f3))


### Performance

* cross-request result cache for /dataflow/analyze ([#20](https://github.com/anatolykoptev/ox-codes/issues/20)) ([#25](https://github.com/anatolykoptev/ox-codes/issues/25)) ([8ce42f4](https://github.com/anatolykoptev/ox-codes/commit/8ce42f4890047b3d161195668a006a2e7d0a96b8))


### Documentation

* add INTEGRATION guide for consumers ([#5](https://github.com/anatolykoptev/ox-codes/issues/5)) ([43b51d1](https://github.com/anatolykoptev/ox-codes/commit/43b51d144e1b17e8058097fe122d52b04a8932aa))
* add README ([#6](https://github.com/anatolykoptev/ox-codes/issues/6)) ([696b0de](https://github.com/anatolykoptev/ox-codes/commit/696b0de03f014dd7a4936cc16d7d0ff40af8bd55))
* update ROADMAP to reflect actual implementation state ([4571409](https://github.com/anatolykoptev/ox-codes/commit/4571409a9ce9d4861836e7dd1c0564948cc40a80))
