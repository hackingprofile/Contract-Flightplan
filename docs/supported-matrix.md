# Supported / unsupported matrix

| Construct | 3.0 | 3.1 | Notes |
| --- | --- | --- | --- |
| JSON request/response bodies (`application/json`) | yes | yes | Other media types stay visible as unsupported |
| Local `#/…` schema `$ref` | yes | yes | Traversal, remote, and cycles are rejected or marked unsupported; a contract expands to at most 100,000 schema nodes (about 64 MiB) over all of its bodies, beyond that exit 3 (`schema_nodes`, `schema_bytes`) |
| Object properties, required, additionalProperties true/false | yes | yes | Nested additionalProperties schemas are unsupported |
| Diff depth | yes | yes | Every depth reachable through `properties` and `items`, following local `$ref`s; each change has an RFC 6901 pointer and a class; unclassified keywords and unsupported constructs are `unknown`, never "no change" |
| Moved operations | yes | yes | Same `operationId`, new method or path: one `moved_operations` entry, bodies still compared |
| Affected component schemas | yes | yes | Changed `#/components/schemas/*` entries with every operation reaching them through local `$ref`s |
| Line numbers | JSON only | JSON only | YAML contracts are marked `line_unavailable` and their changes carry `null` lines |
| oasdiff JSON import | yes | yes | Changelog array or `{changes: [...]}`; keeps `level`, `operation`, `path`; level maps to a compatibility class |
| Scalar string/number/integer/boolean | yes | yes | |
| Schema without `type` | yes | yes | Any type is accepted (JSON Schema semantics); `enum`, bounds and the other keywords still apply. Adding a `type` is reported as a narrowing from `any`; `nullable` adds null only to a declared type |
| Value constraints | yes | yes | `minimum`/`maximum` (3.0 boolean and 3.1 numeric exclusivity; compared exactly, integers as integers beyond 2^53 and up to the 64-bit limits), `minLength`/`maxLength` (characters), `pattern` (Rust regex syntax, compiled once per contract within 512 KiB; others unsupported, visible; at most 256 distinct patterns per contract), `minItems`/`maxItems`/`uniqueItems`; `format` checked for `date`, `date-time`, `uuid`, `email`, other formats are annotations |
| Validator and JSON Schema / ECMA-262 | matched | matched | Matched: `\d`, `\w`, `\b` (and `\D`, `\W`, `\B`) are ASCII in `pattern`, as in ECMA-262; `enum` and `uniqueItems` compare numbers by value (`1` equals `1.0`); `integer` accepts a number with no fractional part (`1.0`); `email` refuses control and invisible (format) characters; a leap second (`:60`) is accepted only at 23:59:60 UTC. Remaining differences: `pattern` uses Rust regex syntax (lookaround and backreferences are unsupported and shown as such); `\s` is Unicode White_Space (ECMA-262 also counts U+FEFF and not U+0085); `.` excludes only `\n` (ECMA-262 also excludes `\r`, U+2028, U+2029); leap seconds are accepted at 23:59:60 UTC on any date (no leap-second table); `email` is a pragmatic check, not the full RFC 5321 grammar |
| Arrays of scalars/objects with `items` | yes | yes | Max 256 items |
| Enum | yes | yes | Unmapped values fail unless an approved const policy exists |
| `nullable: true` | yes | visible, not rewritten | 3.1 `nullable` is reported, not converted |
| `type: [string, "null"]` | visible, not rewritten | yes | 3.0 type arrays are reported, not converted |
| YAML or JSON encoding | yes | yes | Size ≤ 1 MiB, depth ≤ 32 |
| Mapping ops rename/copy/default/enum_map/drop/required_field/safe_cast/unit_conversion/split/merge/array_map/array_filter/pagination | yes | yes | Type-checked against both schemas; errors block approval, warnings are reported |
| Lossiness / reversibility vocabulary | yes | yes | Fixed vocabularies; observed events are computed at verify time; the lossiness witness computes reversibility from execution and reports declarations that claim more |
| SARIF 2.1.0 output | yes | yes | From a verify report only; one rule per outcome kind |
| Generated Node / Python suites | yes | yes | One test per catalog item; drive `cfp review`, `approve`, `verify` and check successful outputs with `cfp validate`; both run inside `cargo test`; the Python suite reports UNEXECUTED on a machine without Python 3 |
| allOf / oneOf / anyOf / not / discriminator | unsupported | unsupported | Left visible in the document |
| Recursive schemas | unsupported | unsupported | |
| Path/query/header mapping | not mapped | not mapped | Noted; not transformed |
| Remote references | rejected | rejected | |
| Hosted provider generation | preview only | preview only | `none`, `file` and the local `example` heuristic are the enabled modes; hosted generation is not delivered |
