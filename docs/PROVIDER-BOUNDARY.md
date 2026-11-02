# Provider and privacy boundary

Compare, review, approve, verify, report, corpus, generate, and file propose are local. They do not open sockets; the crate has no HTTP dependency.

Enabled provider modes:

- `none` — does not generate proposals
- `file` — validates a local proposal file (`--quarantine <dir>` keeps a refused one aside with its diagnostics)
- `example` — a deterministic local heuristic (same-schema removed/added properties become rename candidates) that lists its alternatives and assumptions; no model, no network

Every provider implements the same contract: it receives a `ProposalRequest` holding the contract digests, the selected inputs by digest and redacted shape, and the diff digest, and returns a `ProposalResponse`. The response is accepted only through the same checks as a file proposal (no approval fields, known operations, well-formed parameters, no unknown fields).

Hosted ids `openai` and `anthropic` are listed as `not_enabled`. `cfp provider --preview <id> [--select <file>...]` shows the consent preview that a future outbound path would have to present:

- `would_send[]` lists each selected file with `bytes`, `sha256` and a redacted `shape` (object keys and array structure kept, every scalar replaced by a type marker such as `<string>`);
- `blocked: true` and `outbound_request_issued: false` are always set;
- without `--select` the list is empty.

No command issues an outbound request.

If a later release enables outbound calls:

1. The user must select the exact payload that would leave the machine (`--select`).
2. The redaction preview above must be shown first.
3. Explicit consent is required before any request.
4. The deterministic core must still run with providers disabled (`none` / file proposals).
