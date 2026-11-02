# Trial protocols (synthetic)

These protocols are materials for later external trials. They are not evidence of completed external trials.

## Protocol A — ambiguity first

Team: contract owners. Input: two synthetic OpenAPI documents with a duplicated `operationId`. Expected: compare exits 1 with `ambiguous`, no guessed pairing, HTML report if produced lists the failure.

## Protocol B — trust boundary

Team: security reviewers. Input: a proposal that includes `approval` fields and a hosted `--mode openai` propose. Expected: injected approval rejected; openai mode unsupported; `cfp provider --preview openai` shows `blocked: true` and empty `would_send`.

## Protocol C — irreversible drop

Team: API consumers. Input: `packs/drop-legacy-flag`. Expected: verify passes; lossiness witness lists dropped `legacy`; reversing the migration is not claimed.

Priority-one issues to record in a later trial: any guessed success, any outbound payload, any silent overwrite.
