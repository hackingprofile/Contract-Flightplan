# CLI (implemented commands)

Binary: `cfp`. Hosted providers are listed as `not_enabled` and cannot send data.

## compare

`cfp compare --source <file> --destination <file> [--out <file>] [--oasdiff <file>]`

Contracts are JSON or YAML. A contract with a duplicate key anywhere (for example two `post:` entries under one path, or `title` twice in `info`) is refused with exit 3, naming the key and its pointer (``duplicate key `post` at /paths/~1users/post``); the reader never keeps one value silently.

Operations are matched by method and path. Each matched request and response body is compared at every depth reachable through `properties` and `items`, following local `$ref`s. Every difference becomes one entry in the body's `changes[]` with:

- `pointer`: an RFC 6901 pointer to the schema in the source contract (`/paths/~1users/post/requestBody/content/application~1json/schema/properties/address/properties/zip`). Below a `$ref` the pointer keeps following `properties/<name>` and `items`, so it names the logical position in the body rather than the component's own location.
- `field`: the payload path, for example `address.zip` or `lines[].price`.
- `kind`: `property_added`, `property_removed`, `required_added`, `required_removed`, `type_changed`, `enum_changed`, `additional_properties_changed`, `keyword_changed` (a constraint such as `maxLength`, `format` or `default` that the tool does not classify), `annotation_changed` (`description`, `title`, `example`, `deprecated`, `x-` extensions and similar), or `unsupported_changed` (the schema changed and uses a construct outside the supported subset, such as `allOf`).
- `compatibility`: `breaking`, `non_breaking`, `unknown` or `unchanged`. A request is read by the server, so narrowing it (a new required field, a removed enum value, a narrower type) is `breaking` and widening it is `non_breaking`; a response is read by the caller, so widening it is `breaking`. A removed property is `breaking` on either side. `keyword_changed` and `unsupported_changed` are always `unknown`.

Each body also carries a `compatibility`: the worst class among its changes, raised to at least `unknown` when the body contains an unsupported construct. A changed schema never yields an empty `changes[]`.

Path, query, header and cookie parameters are compared by name and location (path-item parameters, overridden by the operation's own; local `$ref`s followed; a path parameter is always required). Each difference is one entry in the operation's `parameter_changes[]` with `name`, `in`, `kind` and `compatibility`: a new required parameter (`added`) or an optional one becoming required (`required_added`) is `breaking`; a new optional parameter (`added`) or a required one becoming optional (`required_removed`) is `non_breaking`; a removed parameter (`removed`) is `unknown`, because whether the server ignores or rejects it is not described. Parameters are only compared: mapping rules act on bodies, never on parameters. Each operation carries a `compatibility`: the worst class over its request, its responses and its parameter changes, so a new required parameter never leaves an operation `unchanged`.

Unsupported constructs are listed in the document's `unsupported` list (and `unknown` for unknown ones): each note is emitted once, with its own construct name (for example `allOf`, `recursive_reference`) and the RFC 6901 `location` of the body schema it sits in, even when both contracts contain it. When either side's body root is such a construct, the body derives no `added_properties`, `removed_properties` or `changed_properties` (nor `required_added`/`required_removed`) from it; it is `unknown`, and `changes[]` holds one `unsupported_changed` entry if the schema changed.

Each change also has `source_line` and `destination_line`: the 1-based line in that contract where the changed schema is written (below a `$ref`, the line inside the component; for a `required` change, the line of the `required` list), or `null` when that side has no such position. Lines are located for JSON contracts only; each contract reference sets `line_numbers` to `json_key_path` or, for YAML, `line_unavailable`, so a `null` line is never silent.

Operations are paired by method and path first. An operation left unpaired on both sides with the same `operationId` is reported once in `moved_operations` (`from`, `to`) and its bodies are still compared; it does not appear in `added_operations` or `removed_operations`.

`affected_schemas` lists every component schema whose definition differs between the contracts (`component_pointer`, e.g. `/components/schemas/Address`) with every operation, on either side, whose request or response bodies reach it through `$ref`s, directly or through other components.

`--oasdiff <file>` imports an `oasdiff` JSON changelog (a top-level array, or an object with a `changes` array) into `imported_evidence`. Each entry keeps `identity`, `summary`, `level`, `operation` and `path`; `level` 3 (error) maps to `compatibility: breaking`, 1 (info) to `non_breaking`, and 2 (warning) or a missing level to `unknown`. Imported evidence is shown beside the tool's own diff, not merged into it.

## propose

`cfp propose --mode none|file|example [--proposal <file>] [--quarantine <dir>] [--source <file> --destination <file>] [--out <file>]`

`none` does not generate a proposal. `file` validates a local proposal; with `--quarantine <dir>`, a refused proposal is copied to `<dir>/<sha256>.json` beside `<sha256>.diagnostics.json` (file name, digest, outcome, diagnostics) and the command exits 1; a file that cannot be read within the input bounds (over 1 MiB, or not a regular file) is not copied and the command exits 3. `example` is a deterministic local proposer with no model: in each body it pairs a removed property with an added property of the same schema as a `rename`, lists every other same-schema pairing in `alternatives`, and states its heuristic in `assumptions`; the same contracts always give the same proposal. Review renders both, and its `ambiguity` lists every rule with alternatives and every rule whose body carries an unsupported construct. A proposal from any mode is only a proposal: it is reviewed and approved by a person before anything runs. Hosted ids exit 2 (unsupported), also after a consent preview.

Providers implement one contract: a `ProposalRequest` (contract digests, the selected inputs by digest and redacted shape, and the diff digest; never raw values) in, a `ProposalResponse` (candidate proposal and diagnostics) out. Every response passes the same checks as a file proposal before it is accepted.

## review

`cfp review --proposal <file> --source <file> --destination <file> [--out <file>]`

Type-checks every rule against both contracts and writes a document with `typecheck: ok|failed`, `blocking_diagnostics`, and `diagnostics[]` (each with `rule_id`, `severity`, `location`, `message`). When any diagnostic has `severity: error` the document is still written and the command exits 3. Non-executable.

## approve / reject / history / supersede

`cfp approve --proposal <file> --source <file> --destination <file> --fixtures <dir> --review <file> --reviewer <name> [--note <text>] [--approved-at <rfc3339>] --out <file>`

`--approved-at` is an RFC 3339 UTC timestamp with whole seconds, `YYYY-MM-DDTHH:MM:SSZ` (the form the migration schema records), with a real calendar date; anything else, such as `yesterday`, an offset like `+05:30` or fractional seconds, is refused with exit 3 and no file is written. Without it, approve records the current UTC time. `supersede` applies the same rule.

`--review` is required: the review record that `cfp review --out` wrote for this exact proposal and these contracts. It is read strictly (duplicate keys and unknown fields are refused, exit 3). Its proposal digest and both contract digests must match the inputs being approved, otherwise approve refuses with `stale_digest` (exit 1); a record whose type-check failed is refused. The record's SHA-256 is bound as `approval.bound_digests.review_record`. Approve reads every input once: each digest it binds (proposal, contracts, review record, catalog, fixtures) is the digest of the bytes it parsed, type-checked and compared, and a catalog or fixture that changes between the folder digest and its check is refused (`stale_digest`). The review record is reproducible: running `cfp review --out` again on the same inputs writes the same bytes.

Approve runs the same type-check first; an ill-typed proposal never becomes a migration (exit 3, no file written). Before binding any digest it also reads every catalogued fixture: each must be UTF-8 JSON without duplicate keys, and a fixture that is not is refused by its catalog id and path (exit 3, no file written).

The migration's `history` is hash chained: approve writes a `reviewed` event (digest = the review record) and an `approved` event whose digest is the canonical digest of every bound digest (proposal, rules, both contracts, every fixture file and the review record); every event's `prev` is the canonical digest of the event before it (`null` on the first). `cfp verify` and `cfp history --verify-chain` check the chain, that the approval (reviewer, time and every bound digest) matches its events, and that the migration's rules, fixture list and contract digests are the bound ones; an emptied history, a removed or edited event, an edited reviewer, or a rule or fixture digest recomputed inside the migration fails with a `history` error (exit 1), and a rule edited without recomputing its digest with `stale_digest` (exit 1). These are tamper-evident local records, not signatures: someone who rewrites the migration and recomputes every digest in it passes `history --verify-chain`, and is then stopped by `verify`, which runs only rules equal to the reviewed proposal's (see verify), unless the proposal file was changed as well, which a code review sees. Signing migrations is not implemented.

`cfp reject --proposal <file> --reviewer <name> --reason <text> --out <file>`

`cfp history --migration <file> [--verify-chain]`

Prints the events. With `--verify-chain` it checks every link and the approval against its events, prints `{"chain": "ok", ...}` and exits 0, or exits 1 naming the first broken link.

`cfp supersede --migration <old> --proposal <new> --source <file> --destination <file> --fixtures <dir> --review <file> --reviewer <name> [--note <text>] [--approved-at <rfc3339>] --out <file> --archive <file>`

Approves the new proposal exactly like `approve` (it needs its own review record) and adds `supersedes: {id, digest}` naming the old migration by id and canonical digest. It writes a copy of the old migration to `--archive` with lifecycle `superseded` and a chained `superseded` event whose digest is the new migration's. A superseded migration cannot be verified. The old input file is not modified, but `cfp verify` refuses it (exit 3) while the successor, or the archive copy, sits in the same folder: verify reads the JSON files beside the migration (at most 256) and refuses a migration that another one names in `supersedes`, or whose history an archive copy with the same id continues with a supersession. A copy edited back from the archive (last event dropped, lifecycle set to `approved`) is the old migration and is refused the same way. This is local evidence: with both the successor and the archive removed from the folder nothing can tell, so keep them beside the old file (or replace the old file with the archive copy).

## verify

`cfp verify --migration <file> --proposal <file> --source <file> --destination <file> --fixtures <dir> [--out <file>]`

`--proposal` is the reviewed proposal the migration was approved from; keep it next to the migration. Before any fixture runs, verify checks the history (above), that this file's digest is the bound proposal digest, and that its rules are exactly the migration's rules; otherwise it refuses with `stale_digest` (exit 1) and writes nothing. A rule changed inside the migration therefore also needs a changed proposal file, which a code review sees.

Runs every catalogued fixture. `--out` is written whether or not the run is stable; each outcome records `expect`, `actual`, `passed`, and for failures `field` plus `location.line`/`location.column` inside the fixture file. Exits 1 when any fixture disagrees with the catalog.

## report

`cfp report --source <file> --destination <file> --html <file> --json <file> [--proposal <file>] [--migration <file> --fixtures <folder>] [--verify <file>] [--sarif <file>] [--md <file>] [--job-summary <file>]`

Without `--migration` the report shows the diff (and the review, with `--proposal`) and its verdict is "not run". With `--migration` it also needs `--proposal` and `--fixtures` (exit 3 otherwise): the report re-runs verification itself, exactly as `cfp verify` would, and the verdict, the pass counts and the failing list come only from that fresh run. A verdict is never read from a file.

The HTML report is a one-file offline dashboard: no scripts, fonts, images or network references (a Content-Security-Policy forbids them), readable in light and dark mode (it follows the system setting) and on a narrow screen. In order: a verdict badge (green Stable, red Not stable, amber Held when no migration was given, so nothing was verified) with the pass count and failing ids; summary cards for the approval (reviewer and time), the contracts, changes by class and examples passed/failed; a timeline (compared, proposed and by whom, reviewed, approved by whom and when, verified when; `verified_at` is recorded by `cfp verify`); the contract delta as a table per operation (field, change, class badge, reason, including parameter changes); every example's before and after side by side with the reason for a refusal next to it; then rules, diagnostics, approval and the fixture table. Raw JSON is folded under "Raw JSON" disclosures. Every untrusted string is escaped as described below.

The HTML report, the Markdown report and the job summary open with the verdict: stable yes or no, N of M fixtures passed, and the ids of the failing fixtures (or "not run" without `--migration`). Approval text comes after it. Untrusted text (reviewer, note, catalog ids, diagnostics, messages) is neutralized: control and line-separator characters and every invisible or reordering character (Unicode format characters such as zero-width spaces and joiners, U+FEFF, U+061C and the other bidirectional marks and isolates, variation selectors, U+034F and the Hangul fillers) are shown as escapes such as `\n`; in Markdown, `<`, `>` and `&` become entities and inline Markdown punctuation is backslash-escaped, so such text cannot start a new line or list item, open HTML or a comment, or restate the verdict.

`--verify` is optional and only checked: it needs `--migration` (exit 3 otherwise), and it is refused with exit 3, before anything is written, when it differs in any way from the fresh run: its migration id or approval, the number of outcomes, or any outcome's id, path, expect, actual, pass flag or details, or the run's `stable` (only `verified_at`, the time it ran, may differ). A hand-edited verify file therefore cannot change the verdict, drop a failing fixture, or carry a URL into SARIF. In SARIF, each result's `artifactLocation.uri` is the outcome's relative fixture path, percent-encoded, under `uriBaseId` `FIXTURES`.

`--sarif` requires `--migration` with `--proposal` and `--fixtures` (exit 3 otherwise): its results are the fresh run's fixture outcomes. SARIF results use rule ids `cfp.fixture.<outcome>` and point at the failing field's line/column in the fixture. Absolute input paths are reduced to file names in every recorded artifact.

## corpus / fixtures / generate

`cfp corpus --packs <dir> [--seed <u64>] [--matrix <file>] [--out <file>]`

Approves and verifies every pack under `--packs` (each with its own review record), then runs the fixture generator with `--seed` (default 0) over every source body the pack's rules read; each generated positive must validate. Each pack records `generated` and `generated_digest`; the report records the numeric `seed`. The report also carries `coverage`: one row per mapping operation with the verified outcome kinds of every catalogued fixture whose rules use it, the kinds it requires (a null input counts as refused by `explicit_null` or, when the source schema forbids null, `schema_validation`), and any still missing; a missing required kind makes the corpus unstable (exit 1). `--matrix <file>` writes the same table as Markdown; `docs/COVERAGE-MATRIX.md` is that file, checked in. The review records and migrations it writes go to a new, uniquely named folder under the system temporary folder, which is removed when the run ends, pass or fail; the packs themselves are only read.

Every catalog item carries `id`, `path`, `rule_ids`, `expect`, a one-sentence `description` and a `version`, and optionally the `seed` it was generated with; approve and verify refuse an item without them. An `id` is a plain name: 1 to 128 characters from `A-Z`, `a-z`, `0-9`, `.`, `_` and `-`, not only dots; any other id (a path such as `../x`, an absolute path, spaces or control characters) is refused when the catalog is read (exit 3). Each id may appear only once in a catalog. The catalog must list at least one fixture, and every rule of the proposal (at approve) or migration (at verify) must appear in at least one item's `rule_ids`; otherwise approve and verify refuse with exit 3, and a run without outcomes is never reported stable. The same seed always gives the same report, and a different seed draws different sample values.

`cfp fixtures --source <file> --method POST --path /users --location request [--status 200] [--seed <u64>] [--out <file>]`

Sample values are drawn from a SplitMix64 stream seeded with `--seed` (default 0), recorded as `seed` in the output; the same seed and schema give the same fixtures. Besides the positive, missing, null, malformed and enum-negative sketches, it adds boundary cases for each top-level property: the empty string, zero and the largest integer, the empty array, an array at the 256-item bound, and one item over it (`bound`). Each `expect` is what the contract itself reports for that body (a null is `success` only for a nullable field, otherwise `schema_validation`); mapping-level outcomes come from `cfp verify`.

`cfp generate --kind jest|pytest|lossiness --pack <dir> --out <dir-or-file>`

Generated suites contain no local paths. They read `CFP_PACK` and `CFP_BIN` from the environment, run `cfp review --out`, `cfp approve --review` and `cfp verify` once, and hold one test per catalog item: its actual outcome must be the catalogued one, and for a `success` item the output (`after`) must pass `cfp validate` against the destination body its last rule writes. That output is written inside the suite's own temporary folder as `fixture-<position>.after.json`, named by the fixture's position, never by its id. A final test requires a stable run with exit 0. `jest` writes `pack.test.js` (runs with `node --test`), `pack.jest.test.js` and a `package.json`; `pytest` writes a `unittest`-compatible module. `lossiness` writes the witness computed from execution (see `docs/UNCERTAINTY.md`).

## guide

`cfp guide --source <file> --destination <file> --fixtures <dir> --out-dir <dir> [--proposal <file>] [--reviewer <name>] [--note <text>] [--approved-at <rfc3339>] [--yes]`

A guided terminal session over the same steps as the separate commands, which stay unchanged. It prints six numbered steps, each with a plain-language explanation:

1. Compare: operations matched, added, removed and moved, and every body and parameter change with its class and reason. Writes `diff.json`.
2. Suggest a mapping: with `--proposal`, "Suggested by: a person, in the proposal file <name>"; without it, the built-in local proposer (`propose --mode example`) suggests one, its assumptions are listed, and it is written to `proposal.json`. Every rule is listed with its operation, parameters, lossiness and reversibility, plus any alternatives.
3. Review and type-check: each rule's explanation and every diagnostic. Writes `review.json`. A blocking diagnostic stops the guide (exit 3) before approval.
4. Approve: shows what will be bound, asks for the reviewer's name when `--reviewer` is not given, then asks `Approve this mapping? [y/N]`. Only `y` or `yes` approves; Enter, any other answer or the end of input is No, and the guide stops with exit 1 (unapproved) without writing a migration. `--yes` gives the answer `y` up front. The reviewer and time are recorded exactly as `approve` records them (`--approved-at` follows the same rule; the current UTC time otherwise). Writes `migration.json`.
5. Verify: every catalogued example with its expected and actual outcome and the reason for a refusal. Writes `verify.json`.
6. Report: writes `report.html`, `report.json` and `report.md` and prints the report path and the verdict. Like `cfp report`, it re-runs verification itself and refuses (exit 3) if its own `verify.json` differs from that run.

The exit code is 0 when the mapping was approved and every example passed, 1 when it was not approved or an example failed, 3 for a refused input, as for the separate commands. Non-interactive use passes `--reviewer` and `--yes`; with no input and no `--reviewer` the guide refuses (exit 3) and approves nothing. The built-in proposer names its own rules, so a fixture catalog written for another proposal's rule ids is refused at approve (exit 3: no fixture exercises the suggested rules); pass that proposal with `--proposal`.

## validate

`cfp validate --schema-of <contract> --method <METHOD> --path <path> --location request|response [--status <code>] --payload <file>`

Validates one JSON payload against one body schema of the contract (the request body, or the response body for `--status`, default 200), with the same rules `verify` applies. Prints `{"valid": true}` and exits 0, or exits with the outcome's code (for example `malformed` or `schema_validation`, exit 1). The generated suites use it to check every successful output.

## provider

`cfp provider --list`

`cfp provider --preview openai [--select <file>...]`

Lists the selected files with their byte size, SHA-256 and a redacted shape (keys kept, scalar values replaced by `<string>`, `<number>` markers). `blocked` stays `true` and no request is issued.

## JSON inputs

Every JSON input (proposal, migration, fixture catalog, fixture, verify report, oasdiff report) is read strictly: a duplicate object key is a parse error that names the key and its RFC 6901 pointer (exit 3), instead of the last value silently winning. Proposals, migrations and fixture catalogs also refuse any field their published schema does not define, including unknown rule parameters (exit 3).

## Resource bounds

Every input is treated as hostile, so each command stops with a `resource bound exceeded (<kind>)` error (exit 3) instead of exhausting memory or time:

| Kind | Bound |
| --- | --- |
| `file_size` | 1 MiB per input file, enforced while reading (a file that grows past its reported size stops one byte past the bound); only regular files are read: a folder, device, FIFO or socket, also behind a symlink, is refused with exit 3 before anything is read; the file is opened first (on Linux and macOS without blocking, so a FIFO never waits for a writer) and the open handle's type decides, so a file swapped between a check and the open cannot slip through |
| `depth` | nesting depth 32; a YAML contract is checked by a streaming pre-pass before it is parsed, so deep flow collections (`[[[[...`) are refused in milliseconds instead of being parsed in time that grows with the square of their depth |
| `ref_chain` | 16 `$ref`s followed in a row (reported as an unsupported note on that schema) |
| `schema_nodes` | 100,000 schema nodes per contract once every `$ref` is expanded in place, over all of its bodies |
| `schema_bytes` | about 64 MiB of schema copied by that expansion |
| `diff_entries` | 100,000 changes plus notes in one diff |
| `patterns` | 256 distinct `pattern` strings per contract; each is compiled once, when the contract is read, and a pattern that compiles beyond 512 KiB is reported as unsupported (exit 2) |
| `properties` | 256 top-level properties per body |
| `enum` | 64 values per `enum` |
| `array` | 256 items per array in a payload |

A contract of a few kilobytes whose `$ref`s fan out (each component referring to the next one several times) expands to millions of nodes; it reaches `schema_nodes` or `schema_bytes` within a second and nothing is written.

## Output files

Every command checks its output paths before it reads anything: an output that is one of the command's own inputs (the same file under any spelling), an output that lies anywhere inside an input folder (`--fixtures`, `corpus --packs`, `generate --pack`, including a new file or a new subfolder there, which would change what was approved), or two outputs with one path, is refused with exit 3 and nothing is written or created. `generate --out` is an output folder and is checked the same way; `guide --out-dir` is checked before it is created. The one exception is `cfp supersede --archive`, which may replace the old `--migration` file itself. A new or regular-file output is written to a new temporary file beside it and then renamed into place; an output path that is a link (symlink, junction) or a folder is refused, so an output is never written through a link. An existing character device or FIFO (`--out /dev/null`, a named pipe; `NUL` on Windows) is written to directly and is never renamed over, so it stays a device or FIFO, and several outputs may go to it in one command. A block device, a socket, any other special file, and a Windows device other than `NUL` (`CON`, `AUX`, `COM1`, `LPT1`, ...) are refused with exit 3.

## Terminal output

Error messages on stderr, usage errors and the lines `cfp guide` prints quote input (paths, keys, references, names). Every control character in them, line breaks, ESC, C1 controls, bidirectional controls and zero-width characters included, is shown as an escape such as `\u{001b}` or `\n`, so input cannot recolour or redraw the terminal or forge a line, and no line starts with `::`, which GitHub Actions reads as a workflow command (a leading `::` is shown as `\u{003a}:`). JSON on stdout is plain JSON: characters below U+0020 inside strings are JSON escapes. A standard output or error that is closed or full (a pipe whose reader has gone, `/dev/full`) is an I/O error: the command stops with exit 3 instead of crashing.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | success |
| 1 | mapping, digest, approval, history or ambiguity failure, a catalog item that names a rule the migration does not have, or a fixture that disagrees with its catalog |
| 2 | unsupported construct rejected at the boundary |
| 3 | usage, parse, type-check, I/O, or resource bound, including a malformed or incomplete catalog: a missing or unreadable catalog, an item whose file is missing, an item that names no rule, an id outside the safe set or used twice, a catalog with no fixtures, or a rule that no fixture exercises |
