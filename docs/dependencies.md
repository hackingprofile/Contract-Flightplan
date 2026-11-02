# Dependency and licence inventory

Toolchain: rustc 1.98.1, pinned in `rust-toolchain.toml` (with rustfmt and clippy). The hosts CI runs on are listed in `docs/HOST-MATRIX.md`.

Direct crates (see `Cargo.lock` for the complete graph):

| Crate | Version | Licence (upstream) | Role |
| --- | --- | --- | --- |
| clap | 4.6.7 | MIT OR Apache-2.0 | CLI |
| serde | 1.0.229 | MIT OR Apache-2.0 | typed documents |
| serde_json | 1.0.151 | MIT OR Apache-2.0 | JSON parse/report |
| yaml_serde | 0.10.7 | MIT OR Apache-2.0 | YAML OpenAPI (YAML org maintained serde_yaml fork; docs: https://crates.io/crates/yaml_serde and https://docs.rs/yaml_serde) |
| sha2 | 0.10.9 | MIT OR Apache-2.0 | content digests (docs: https://docs.rs/sha2/0.10.9/sha2/) |
| thiserror | 2.0.20 | MIT OR Apache-2.0 | error types |
| regex | 1.13.1 | MIT OR Apache-2.0 | JSON Schema `pattern` (Rust regex syntax; a pattern it cannot compile is reported as unsupported) |
| libyaml-rs | 0.3.0 | MIT | The YAML tokenizer `yaml_serde` is built on. Called directly by `yaml_nesting` for an event pre-pass that stops at 33 levels of nesting, so deeply nested flow collections are refused before the full parse |

Development-only crates (tests, not shipped in the binary):

| Crate | Version | Licence (upstream) | Role |
| --- | --- | --- | --- |
| jsonschema | 0.57 | MIT | validates packs, examples and outputs against `schemas/v1` |
| proptest | 1.11.0 | MIT OR Apache-2.0 | property tests (`default-features = false`, `std`); adds rand 0.9.5, rand_chacha 0.9.0, rand_core 0.9.5, rand_xorshift 0.4.0, ppv-lite86 0.2.21, unarray 0.1.4, all MIT OR Apache-2.0 |

No runtime network client is a dependency. Compare/review/approve/verify/report do not call providers.

Local crate license graph: `scripts/sbom.ps1`. Review notes: `docs/LICENSE-SCAN.md`.
