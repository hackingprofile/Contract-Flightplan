# Product charter

Promise: Contract Flightplan produces a **reviewed, digest-bound migration contract** that can be re-executed locally against fixtures without a model.

Users: API owners, SDK maintainers, and integration reviewers who must prove that a contract change is applied exactly as approved.

Excluded autonomy:

- The tool does not invent mappings. File/manual proposals are inputs, not decisions.
- The tool does not call hosted models or open network requests for compare/verify.
- The tool does not execute unapproved proposals.
- The tool does not guess compatibility for unsupported OpenAPI constructs.

Every shipped feature must support that reviewed-migration proof: compare shows the delta, review explains rules and lossiness, approve binds content, verify reproduces the transform, report shows the evidence. Features that do not serve that proof are out of scope until a later release scopes them.
