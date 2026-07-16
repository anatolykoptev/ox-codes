# Changelog

## [0.1.2](https://github.com/anatolykoptev/ox-codes/compare/v0.1.1...v0.1.2) (2026-07-16)


### Added

* **walk:** guard + budget /rewrite with surfaced oversize-skip, completes the walk-budget arc ([#57](https://github.com/anatolykoptev/ox-codes/issues/57) PR3) ([#73](https://github.com/anatolykoptev/ox-codes/issues/73)) ([012e590](https://github.com/anatolykoptev/ox-codes/commit/012e5904e00eeb8681c125fa6ffabab5344f8df4))
* **walk:** guard + clamp + byte-cap the 4 read-only endpoints, grep truncate-before-expand ([#57](https://github.com/anatolykoptev/ox-codes/issues/57) PR2) ([#72](https://github.com/anatolykoptev/ox-codes/issues/72)) ([3459a9d](https://github.com/anatolykoptev/ox-codes/commit/3459a9d9fba0243ea4caa2c0a3e5c3c609fd6744))


### Fixed

* **cache:** loud TTL=0 backstop-disabled warning + is_hit observability signal ([#62](https://github.com/anatolykoptev/ox-codes/issues/62)) ([5a5c622](https://github.com/anatolykoptev/ox-codes/commit/5a5c622205a81156bcafb4c68a6a9c4b04f312ab))
* **dataflow+core+langs:** JSX-aware grammar across all TS routes + is_ts_secondary save/restore ([#44](https://github.com/anatolykoptev/ox-codes/issues/44), [#49](https://github.com/anatolykoptev/ox-codes/issues/49)) ([cebb54f](https://github.com/anatolykoptev/ox-codes/commit/cebb54fa0d4e76dde5e20482ef0e8b11ebb331ca))
* **dataflow:** defer Svelte template refs when script follows template ([#38](https://github.com/anatolykoptev/ox-codes/issues/38)) ([509faf1](https://github.com/anatolykoptev/ox-codes/commit/509faf199af6ee9472cb5262bcfc69414ccc81c0)), closes [#15](https://github.com/anatolykoptev/ox-codes/issues/15)
* **dataflow:** parse-and-detect-ERROR fallback for JSX in .js/.ts files ([#70](https://github.com/anatolykoptev/ox-codes/issues/70)) ([3c5df55](https://github.com/anatolykoptev/ox-codes/commit/3c5df554039b6d539eb2333b479febe4daff4f23))
* **dataflow:** track taint through object/array destructuring declarators ([#63](https://github.com/anatolykoptev/ox-codes/issues/63)) ([130aa5e](https://github.com/anatolykoptev/ox-codes/commit/130aa5ee1593d70a69747a2a3edbc3ba43be773e)), closes [#59](https://github.com/anatolykoptev/ox-codes/issues/59)
* harden dataflow cache key + TTL/BYTES escape hatches ([#29](https://github.com/anatolykoptev/ox-codes/issues/29)) ([#40](https://github.com/anatolykoptev/ox-codes/issues/40)) ([64eb777](https://github.com/anatolykoptev/ox-codes/commit/64eb777bb6e8b575efe77c60dd54e714822f6b1b))
* **langs:** /search/structural language=tsx now matches .tsx/.jsx files ([#68](https://github.com/anatolykoptev/ox-codes/issues/68)) ([799a4c9](https://github.com/anatolykoptev/ox-codes/commit/799a4c97b87fa9e9cd1b8305f3508426f27a8e07)), closes [#56](https://github.com/anatolykoptev/ox-codes/issues/56)
* **rewrite:** harden /rewrite write path vs silent corruption + lost updates ([#41](https://github.com/anatolykoptev/ox-codes/issues/41), [#47](https://github.com/anatolykoptev/ox-codes/issues/47)) ([30ad140](https://github.com/anatolykoptev/ox-codes/commit/30ad140c9395da26a0e5bbd6ea8585baa5bfd8bb))
* **server:** dataflow walk-pool timeout + byte-weighed cache + exact-cap truncation ([#43](https://github.com/anatolykoptev/ox-codes/issues/43), [#45](https://github.com/anatolykoptev/ox-codes/issues/45), [#50](https://github.com/anatolykoptev/ox-codes/issues/50)) ([#60](https://github.com/anatolykoptev/ox-codes/issues/60)) ([547d75a](https://github.com/anatolykoptev/ox-codes/commit/547d75a8afcb3bb0a1f11532a25d23ff5cb0fdad))


### Changed

* **cache:** extract shared WeightedEnvCache&lt;K,V&gt; from Scope/Dataflow caches ([#65](https://github.com/anatolykoptev/ox-codes/issues/65)) ([2534cd7](https://github.com/anatolykoptev/ox-codes/commit/2534cd75c648f7f4259eda0373b16369827d4b42))
* **rewrite:** reuse ast_grep_core for the re-parse invariant, drop the double raw tree_sitter parse ([#66](https://github.com/anatolykoptev/ox-codes/issues/66)) ([56f7934](https://github.com/anatolykoptev/ox-codes/commit/56f79344e77d42eff76983321def1cf9c8b9f860))
* **walk:** extract ox-core::walk + ox-server::walk_guard, rewire /dataflow/analyze ([#57](https://github.com/anatolykoptev/ox-codes/issues/57) PR1) ([#71](https://github.com/anatolykoptev/ox-codes/issues/71)) ([ae2cfc7](https://github.com/anatolykoptev/ox-codes/commit/ae2cfc74f8c00688af785ef1bab9181c509a3da3))


### Documentation

* drop broken docs/architecture link from README ([#36](https://github.com/anatolykoptev/ox-codes/issues/36)) ([5fbf040](https://github.com/anatolykoptev/ox-codes/commit/5fbf040731dad826cb86536b8d83755071b9eba3))

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
