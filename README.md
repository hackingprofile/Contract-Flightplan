# Contract Flightplan

Local API-contract migration planner. Compare two OpenAPI documents, review an explicit mapping, approve the exact content, and verify fixtures without a model or network service.

Implemented mapping operations: rename, copy, default, enum_map, drop, required_field, safe_cast, unit_conversion, split, merge, array_map, array_filter, pagination.

## Scope

Supported now:

- Commands: `compare`, `propose`, `review`, `approve`, `reject`, `history`, `supersede`, `verify`, `report`, `corpus`, `fixtures`, `validate`, `provider`, `generate`, and `guide` (all of the main steps in one guided terminal session)
- Local JSON and YAML OpenAPI 3.0 / 3.1 files
- JSON object properties, required fields, scalars, enums, and arrays with `items`, compared at every depth with an RFC 6901 pointer and a `breaking` / `non_breaking` / `unknown` class per change; moved operations and affected component schemas are reported
- Value constraints enforced on fixtures: numeric and length bounds, `pattern`, `minItems` / `maxItems` / `uniqueItems`, and the formats `date`, `date-time`, `uuid`, `email`
- OpenAPI 3.0 `nullable` and OpenAPI 3.1 `type: [..., "null"]` as distinct dialects
- Local `#/…` references with cycle, traversal and size bounds
- File proposals, `none` provider mode (no generation), and a deterministic local `example` proposer that shows its alternatives and assumptions (no model)

Not delivered, and therefore not advertised:

- hosted providers, BYOK, or any outbound model call (`openai` / `anthropic` preview as `not_enabled`)
- combinators (`allOf` / `oneOf` / `anyOf` / `not`), path/query mapping, remote `$ref`

See `docs/CHARTER.md` for the product promise, `docs/CLI.md` for commands, and `docs/GOVERNANCE.md` for Apache-2.0 / DCO policy.

## Quickstart

Requires Rust 1.98.1, pinned in `rust-toolchain.toml` with rustfmt and clippy (rustup picks it up automatically), and the source tree. The scripts run under PowerShell 7 (`pwsh`) on Windows, Linux and macOS, and under Windows PowerShell 5.1. The quickstart builds `cfp`, finds the binary even when `CARGO_TARGET_DIR` is set, and writes the compare, review, approval, verify and report outputs for the rename pack to `out/`.

```sh
pwsh -File scripts/quickstart.ps1
```

Without PowerShell, the same steps with cargo alone:

```sh
cargo build --release --locked
cargo run --release -- compare --source packs/rename-user-id/base.openapi.yaml --destination packs/rename-user-id/next.openapi.yaml
```

## Guided approval

`cfp guide` walks one person through the whole flow in the terminal: compare the contracts, suggest a mapping (from your proposal file, or from the built-in proposer when you give none), review and type-check it, approve it, verify it against the examples, and write the report. Each step says what it does and what it found, the suggestion says who made it, and the guide stops at `Approve this mapping? [y/N]`: only `y` approves. Your name and the time are recorded as `cfp approve` records them.

```sh
cfp guide --source packs/rename-user-id/base.openapi.yaml --destination packs/rename-user-id/next.openapi.yaml \
  --fixtures packs/rename-user-id/fixtures --proposal packs/rename-user-id/proposal.json --out-dir out/guide
```

For scripts, pass `--reviewer <name>` and `--yes`; without input and without those flags nothing is approved. The separate commands below are unchanged.

## Checks

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

More checks, each exiting non-zero on failure:

```sh
pwsh -File scripts/regression-attacks.ps1 -Cfp target/release/cfp   # known attack and false-verdict paths, on pack copies (cfp.exe on Windows)
pwsh -File scripts/mutation-check.ps1                               # 26 hand-written mutants of the mapping interpreter; threshold 85%
pwsh -File scripts/run-generated-tests.ps1                          # generated Node suite, and the Python suite when Python 3 is on PATH
pwsh -File scripts/smoke.ps1                                        # happy path plus the two most important failure paths
```

Local install, upgrade and Action dry run, and a release tree (no publish, no remote Action, nothing signed):

```sh
pwsh -File scripts/rehearse.ps1                                     # install, provider-disabled use, Action use, upgrade, uninstall in a temp sandbox
pwsh -File scripts/assemble-v1.ps1                                  # dist/release: source, this host's binary, schemas, SBOM, checksums
pwsh -File scripts/verify-local-release.ps1 -Tree dist/release
```

## Packs

| Pack | OpenAPI | Operation |
| --- | --- | --- |
| `packs/rename-user-id` | 3.0.3 | rename `id` → `user_id` |
| `packs/default-status` | 3.1.0 | default `status=active` when missing |
| `packs/copy-display-name` | 3.0.3 | copy `name` → `display_name` |
| `packs/enum-map-color` | 3.0.3 | exhaustive enum map |
| `packs/drop-legacy-flag` | 3.0.3 | drop `legacy` |
| `packs/require-email` | 3.0.3 | required_field `email` |
| `packs/safe-cast-count` | 3.0.3 | string → integer |
| `packs/unit-mass-grams` | 3.0.3 | kg → g exact |
| `packs/split-full-name` | 3.0.3 | split full name |
| `packs/merge-display` | 3.0.3 | merge names |
| `packs/array-tags` | 3.0.3 | array filter + map |
| `packs/page-envelope` | 3.0.3 | pagination reshape |

Each pack has base/next contracts, a proposal, a fixture catalog, and positive plus negative fixtures. Approval files are produced by `cfp review --out` followed by `cfp approve --review`; they bind file digests and carry a hash-chained history.

### Line endings

An approval binds the exact bytes of the contracts, rules, catalog and fixtures. If Git converts line endings on checkout (for example `core.autocrlf=true` on Windows), every byte-bound file changes and every approval becomes stale. The repository's `.gitattributes` marks packs, fixtures, schemas, contracts (`*.openapi.yaml`, `*.openapi.json`) and approvals (`*.migration.json`) as `-text`, so Git stores and checks them out unchanged. Do the same in your own repository for the contracts, fixtures and approvals you keep there.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | success |
| 1 | mapping, digest, approval, history or ambiguity failure, a catalog item that names a rule the migration does not have, or a fixture that disagrees with its catalog |
| 2 | unsupported construct rejected at the boundary |
| 3 | usage, parse, type-check, I/O, or resource bound, including a malformed or incomplete catalog: a missing or unreadable catalog, an item whose file is missing, an item that names no rule, an id outside the safe set or used twice, a catalog with no fixtures, or a rule that no fixture exercises |

## Limits

- CI runs the full build and test suite on Linux, Windows and macOS on every push to `main`; see `docs/HOST-MATRIX.md`. A platform counts as run only with evidence from that platform.
- Reviewer names, review records, the hash-chained history and the release checksums are local, tamper-evident records: the chain commits to every bound digest, and `cfp verify` runs only rules equal to the reviewed proposal's, so a rewritten migration needs a visibly changed proposal too. They are not authentication or signatures; signing is not implemented.
- Mapping operations act on top-level body fields. `compare` reports nested changes; it does not map them.
- The generated Node and Python suites run inside `cargo test`; the Python suite is reported as UNEXECUTED on a machine without Python 3.
- Changing a rule, contract or fixture after approval makes the migration stale.
- A default never replaces an explicit JSON `null`. A rename never overwrites an existing destination field. An unmapped enum fails unless the proposal records an explicit const policy.
- Hosted providers cannot send data. `cfp provider --preview openai` shows a blocked consent preview.
