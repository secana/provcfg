# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0](https://github.com/secana/provcfg/compare/provcfg-sea-orm-v0.1.2...provcfg-sea-orm-v0.2.0) - 2026-11-01

### Added

- *(sea-orm)* [**breaking**] JSON values, validator, writable keys and batch writes
- *(sea-orm)* watch the database with LISTEN/NOTIFY and revision polling
- *(sea-orm)* add LiveSettings with validated write-back
- *(sea-orm)* track per-namespace revisions with database triggers
- *(sea-orm)* add provcfg-sea-orm crate for SQLite and PostgreSQL sources

### Other

- *(sea-orm)* add a live_settings example service
- *(sea-orm)* document LiveSettings, write-back and watching
