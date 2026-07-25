## v3.6.0 — 2026-07-25

### Features
- expose index-build fetch windows as options and reset read-ahead on a seek (#92)

## v3.5.0 — 2026-07-25

### Features
- batch gap discovery and add IndexBuilder for wasm index builds (#91)

## v3.4.2 — 2026-07-23

### Fixes
- initialise the wasm module in the many-channel lazy-index test (#90)

## v3.4.1 — 2026-07-23

### Performance
- gather all missing ranges in one metadata-walk pass for incremental index builds (#89)

## v3.4.0 — 2026-07-23

### Features
- defer conversion block resolution to read time for remote indexes

## v3.3.0 — 2026-07-22

### Features
- add browser (web-target) npm build and metadata-only MdfIndex.fromUrl (#87)

## v3.2.3 — 2026-07-19

### Fixes
- republish the npm package with the configured trusted publisher (#86)

## v3.2.2 — 2026-07-19

### Fixes
- let npm trusted publishing authenticate the release npm publish (#85)

## v3.2.1 — 2026-07-19

### Fixes
- ship the WebAssembly module in the npm package and publish it on release (#84)

## v3.2.0 — 2026-07-18

### Features
- support VLSD channel reads through the index reader (#83)

## v3.1.0 — 2026-07-18

### Features
- add WebAssembly bindings and TypeScript npm package (#82)

## v3.0.0 — 2026-07-09

### BREAKING CHANGES
- correctness, robustness and performance fixes from asammdf cross-validation review (#81)

### Fixes
- correctness, robustness and performance fixes from asammdf cross-validation review (#81)

## v2.0.0 — 2026-06-02

### BREAKING CHANGES
- redesign reader/index APIs around name-based navigation and signals (#80)

### Features
- redesign reader/index APIs around name-based navigation and signals (#80)

## v1.7.8 — 2026-06-01

### Fixes
- probe remote file size with ranged GET instead of HEAD (#79)

## v1.7.7 — 2026-06-01

### Fixes
- configure native-tls backend for HTTPS range reads (#78)

## v1.7.6 — 2026-05-28

### Fixes
- get wheel builds green and add PR mirror workflow

## v1.7.5 — 2026-05-28

### Fixes
- install perl-FindBin and perl-IPC-Cmd for vendored OpenSSL build (#76)

## v1.7.4 — 2026-05-28

### Fixes
- vendor OpenSSL so aarch64 Linux wheels build without sysroot headers (#75)

## v1.7.3 — 2026-05-28

### Fixes
- switch ureq to native-tls to fix aarch64 Linux wheel build (#74)

## v1.7.2 — 2026-05-28

### Fixes
- set -march=armv8-a for aarch64 Linux wheel build (#73)

## v1.7.1 — 2026-05-28

### Fixes
- disable sccache for aarch64 Linux wheel build (#72)

## v1.7.0 — 2026-05-03

### Features
- expose cloud index creation and channel reads from URL
- build MdfIndex over HTTP range requests (cloud indexing)

## v1.6.0 — 2026-05-02

### Features
- ship .pyi type stubs for IDE hover docs in Python wheel

## v1.5.2 — 2026-05-01

### Fixes
- emit variable-length DL for multi-fragment VLSD chains

## v1.5.1 — 2026-05-01

### Fixes
- rewrite VLSD inline offsets when cutting

## v1.5.0 — 2026-04-27

### Features
- preserve source HD start time when cutting MDF files

## v1.4.0 — 2026-04-27

### Features
- preserve source/text/conversion blocks when cutting MDF files

## v1.3.1 — 2026-04-27

### Fixes
- attach docstring to MdfException so help() works

### Docs
- expand Python API docstrings for pip-package users

## v1.3.0 — 2026-04-25

### Features
- expose merge_files in Python bindings

### Fixes
- support VLSD signal channels and verify byte-array merging

## v1.2.0 — 2026-04-25

### Features
- cut by absolute UTC time
- expose cut_mdf_by_time

### Fixes
- preserve VLSD, byte-array, and invalidation data when cutting

## v1.1.2 — 2026-04-25

### Fixes
- drop Intel Mac wheel build to avoid macos-13 runner queue

## v1.1.1 — 2026-04-25

### Fixes
- build a single abi3 wheel per OS/arch for Python 3.8+

## v1.1.0 — 2026-04-25

### Features
- use max of latest tag and Cargo.toml version as bump base (#58)

# Changelog

All notable changes to mf4-rs are documented in this file. This project follows Semantic Versioning and Conventional Commits.

## v0.1.0 — 2026-04-25

### Features
- enable automated releases (#57)

### Refactors
- reuse record gathering logic

### CI
- add automated SemVer release pipeline driven by Conventional Commits
