# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0-alpha.8](https://github.com/vig-os/tessera/compare/tessera-wasm-v0.1.0-alpha.7...tessera-wasm-v0.1.0-alpha.8) - 2026-11-27

### Added

- *(core)* seal collection version sources and require fork identity

### Other

- *(wasm)* guard primary root computation and malformed inputs
- *(corpus)* pin paired fixtures and recompute the wasm primary root
- *(corpus)* add derived archive golden and exhaustive cross-check

## [0.1.0-alpha.7](https://github.com/vig-os/tessera/compare/tessera-wasm-v0.1.0-alpha.6...tessera-wasm-v0.1.0-alpha.7) - 2026-11-25

### Added

- [**breaking**] derived blocks leave content_hash (ADR-0028 §4)
- [**breaking**] seal the stored canonical bytes (ADR-0066 A′ cutover)

### Fixed

- refuse a chunk index without its block, in every reader and writer
- *(wasm)* verify the stored member bytes, not strings decoded from them

## [0.1.0-alpha.6](https://github.com/vig-os/tessera/compare/tessera-wasm-v0.1.0-alpha.5...tessera-wasm-v0.1.0-alpha.6) - 2026-11-19

### Fixed

- *(core)* seal only integers a double holds exactly (I-JSON)
- *(core)* verify a manifest's seal over the object it parsed

## [0.1.0-alpha.1](https://github.com/vig-os/tessera/releases/tag/tessera-wasm-v0.1.0-alpha.1) - 2026-09-23

### Added

- *(signing)* bind signed_at + key_format; ssh-ed25519 loader; usage tests-as-docs
- *(wasm)* tessera-wasm JS bindings — in-browser verify, build→bindgen→node gated ([#210](https://github.com/vig-os/tessera/pull/210))
