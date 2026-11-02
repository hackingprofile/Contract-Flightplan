# Architecture

Single Rust package `contract-flightplan`, binary `cfp`. Everything runs locally; there is no network code path in the crate.

| Module | Role |
| --- | --- |
| `strict_json` | JSON reading that refuses duplicate object keys, used for every JSON input |
| `parse` | Bounded JSON/YAML OpenAPI 3.0/3.1 load, local `$ref` policy, portable path reduction for recorded artifacts |
| `yaml_nesting` | Streaming pre-pass over YAML with the same tokenizer (libyaml-rs) that stops at collections nested deeper than 32, before the full parse; the only `unsafe` code, a guarded call sequence into that tokenizer |
| `schema` | Bounded schema resolution and instance validation (limits in `docs/supported-matrix.md`) |
| `diff` | Operation identity, property delta, visible unsupported notes |
| `oasdiff` | Optional import of an `oasdiff` JSON report as extra visible notes |
| `proposal` | Non-executable mapping documents; decodes the 13 operations and rejects malformed parameters |
| `typecheck` | Static check of every rule against both contracts; emits error/warning diagnostics, blocks approval on errors |
| `mapping` | Executes the 13 operations on JSON bodies and records the lossiness events that actually happened |
| `digest` | SHA-256 over proposal, canonical rules, contracts and every fixture file |
| `state` | Lifecycle stages `proposal → reviewed → approved → stale / rejected / failed / superseded` |
| `approve` | Digest-bound migration records; refuses ill-typed proposals |
| `history` | Appends and reads lifecycle events on a migration |
| `verify` | Catalogued fixture execution; always produces a full report, failing fixtures carry field and line/column |
| `sarif` | SARIF 2.1.0 from a verify report, one rule per outcome kind, regions inside the fixture files |
| `report` | Offline JSON, Markdown, job summary and static HTML including the diagnostics/uncertainty section |
| `corpus` | Pack inventory and coverage summary |
| `fixtures` | Fixture skeletons derived from a contract |
| `generate` | Node/Jest and Python test suites that drive `cfp approve` + `cfp verify`, and the computed lossiness witness |
| `guide` | `cfp guide`: the compare → propose → review → approve → verify → report walk-through in the terminal, built on the same calls as the separate commands; approves only on an explicit `y` or `--yes` |
| `provider` | Provider registry: `none`, `file` and the local `example` heuristic enabled, hosted ids blocked, redaction-only consent preview |
| `error` | Error kinds and exit codes 0/1/2/3 |

Parsing does not rewrite 3.0 `nullable` into 3.1 type arrays or the reverse.

Public JSON Schemas for the four artifacts live in `schemas/v1/`; `tests/schema_contract.rs` validates every shipped pack and the CLI outputs against them.

## Failure evidence path

`review` writes its document and exits 3 when type errors exist. `verify` writes `--out` and exits 1 when any fixture disagrees with the catalog. `report` takes its verdict only from its own verification run: with `--migration` it needs `--proposal` and `--fixtures`, re-runs verify, and refuses a `--verify` file that differs from that run; `--sarif` requires that run, so a SARIF file can never be silently empty or forged. Absolute paths passed on the command line are reduced to file names before they are recorded.
