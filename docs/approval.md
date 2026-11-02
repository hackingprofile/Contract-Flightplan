# Approval

A proposal is a non-executable mapping. `cfp review --out` explains and type-checks it and writes a review record. `cfp approve --review <record>` is the only command that creates a migration (`cfp supersede` uses the same path for a successor), and it refuses a record written for a different proposal or different contracts.

Approval records:

- local reviewer name
- timestamp
- SHA-256 digests of the proposal, canonical rules, source contract bytes, destination contract bytes, every file under the fixtures directory, and the review record
- a hash-chained history: a `reviewed` event and an `approved` event, each linking to the digest of the event before it; the `approved` event carries the canonical digest of every bound digest above, so the chain commits to the rules and every fixture file, not only the proposal

Those digests are the execution bound. Changing a rule, a contract file, or any fixture after approval yields `stale_digest` and verification refuses to run. Verification also checks the history: an emptied history, a removed or edited event, an approval whose reviewer, time or digests disagree with its events, or a bound digest recomputed inside the migration is refused (`history`, exit 1). `cfp history --verify-chain` runs the same check on its own.

`cfp verify --proposal` takes the reviewed proposal file and runs the migration only when that file is the approved proposal and its rules are exactly the migration's. Someone who edits a rule and recomputes every digest inside the migration is refused there; to get past it they must change the proposal file too, which is a visible change to the reviewed artifact.

## Supersession

`cfp supersede` approves a successor proposal, records `supersedes: {id, digest}` on the new migration, and writes an archive copy of the old migration with lifecycle `superseded` and a chained `superseded` event that names the new migration's digest. A superseded migration is never executable. `cfp verify` also refuses the old file itself while the successor or the archive copy is in the same folder; with both removed nothing local can tell.

Injected `approval` fields on a proposal are rejected. Passing a proposal to `cfp verify` is `unapproved`.

## Identity boundary

Reviewer names and content hashes are an auditable local workflow: tamper-evident, not tamper-proof. They are not remote identity and not a signature, and they do not stop someone who can rewrite the migration file, the reviewed proposal and the bound inputs together and recompute every digest. Signing migrations (for example with a key the reviewer holds) is not implemented. Keep migrations with the proposals and contracts they name, and review changes to all of them.
