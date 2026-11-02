# Uncertainty classes

Executable examples live in the twelve packs under `packs/` and in `fixtures/negatives`. Every class below is either a diagnostic, a fixture outcome, or a lossiness event, and each appears in the artifact for its stage: the tables below name where. SARIF reports failing fixture outcomes, not every static review diagnostic or lossiness event.

## Static (review time)

| Class | Meaning | Where it appears |
| --- | --- | --- |
| type error | A rule contradicts the contracts: missing field, string into integer, default value that fails the destination schema, enum key or value outside the declared enum, `safe_cast` pair that does not match declared types, duplicate `split` targets | `review.json` `diagnostics[]` with `severity: error`; `typecheck: failed`; `approve` refuses (exit 3) |
| warning | Rule is well-typed but suspect: `operation_id` differs between proposal and contract, declared lossiness `none` on a collapsing enum map, integer widened to number, an enum map that leaves source values uncovered under an explicit `unmapped: fail` | `review.json` `diagnostics[]` with `severity: warning`; does not block |
| ambiguous | Identity cannot be chosen uniquely (duplicate `operationId`); a rule with proposer alternatives; a rule whose body carries an unsupported or unknown construct | `ambiguity[]` in the review and the HTML report (with the proposal's `alternatives` and `assumptions`); a duplicate `operationId` fails `compare` and `verify` |
| unsupported | Construct is outside the subset and is not rewritten (`allOf`, mixed media types, remote `$ref`) | `unsupported[]`, exit 2 at the boundary |
| unknown | Operation name is not in the implemented language | rejected at decode (`unknown_mapping_operation`) |

## Dynamic (verify time)

Every refusal has one outcome name (`src/error.rs`); a fixture's `actual` is `success` or one of them, and SARIF rule ids are `cfp.fixture.<outcome>`.

| Outcome | Meaning | Exit |
| --- | --- | --- |
| `success` | Rule chain applied and the destination body validates | 0 |
| `missing` | A field the rule or a schema requires is absent | 1 |
| `explicit_null` | JSON `null` where the rule needs a value; never treated as missing | 1 |
| `collision` | Destination field already present (rename, copy, split, merge, pagination) | 1 |
| `unknown_enum` | Value not in the enum or the `map`, and no const policy | 1 |
| `malformed` | Wrong type or shape: non-scalar enum value, too few `split` parts, non-array for `array_filter`, unparsable cast | 1 |
| `schema_validation` | A body fails its schema in a way not named above: a forbidden null or additional property, a bound (`minimum`, `maxLength`, `minItems`, `uniqueItems` and the rest), a `pattern`, or a validated `format` (`date`, `date-time`, `uuid`, `email`). Source fixtures are validated before mapping and outputs against the destination after it | 1 |
| `bound` | A resource bound (file size, depth, array length, rule count) is exceeded | 3 |
| `unsupported` | A construct outside the supported subset; never guessed | 2 |
| `unknown` | An unknown name (generator kind, provider id, operation) | 1 |
| `unknown_mapping_operation` | A rule names an operation the language does not implement | 1 |
| `ambiguous` | Identity cannot be chosen uniquely (duplicate `operationId`, several JSON media types) | 1 |
| `unapproved` | A proposal, or a document without approval, given where a migration is required | 1 |
| `injected_approval` | A proposal carries approval fields | 1 |
| `stale_digest` | A bound file or review record no longer matches the approval | 1 |
| `history` | The migration's history chain is broken or disagrees with its approval | 1 |
| `unexpected_outcome` | A fixture's actual outcome differs from the catalog's `expect` | 1 |
| `quarantined` | A refused proposal was copied to the quarantine folder | 1 |
| `parse` | Input is not valid JSON/YAML/UTF-8, has a duplicate key or an unknown field | 3 |
| `io` | A file cannot be read or written | 3 |
| `invalid` | Usage or a refused request: a type-check failure, a missing flag, a superseded migration | 3 |

Failing outcomes carry `field` and, when the field exists in the fixture text, `location.line` / `location.column`.

## Lossiness events (computed, not declared)

Each rule declares `lossiness` from the vocabulary `none | default_inserted | enum_many_to_one | dropped | rounded | merged` and `reversibility` from `reversible | irreversible | partially_reversible`. Execution records what actually happened:

| Event | Emitted when |
| --- | --- |
| default_inserted | `default` inserted a value because the key was absent (not when the key was present) |
| enum_many_to_one | `enum_map` / `array_map` collapsed two source values to one destination value, or a const fallback fired |
| dropped | `drop` removed a field, or `array_filter` removed at least one item |
| rounded | `unit_conversion` divided with a remainder under `floor` / `ceil` / `round` |
| merged | `merge` combined fields |

`cfp generate --kind lossiness` writes a witness computed by running each rule on the pack's successful fixtures. Per rule it lists the observed events, the fields actually discarded (removed without their value surviving under another key; a rename or a pagination move is not a discard), inserted, collapsed and rounded, and a `computed_reversibility`: `reversible` only when nothing was discarded, inserted, collapsed or rounded, otherwise `irreversible` (`not_observed` without a successful fixture). `declaration_mismatch` lists every place the declaration claims more than the evidence shows: lossiness `none` with observed events, a declared event never observed, or `reversible` while the computed value is `irreversible`. `consistent` is true when that list is empty. `partially_reversible` is not contradicted by observed loss.
