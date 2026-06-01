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
