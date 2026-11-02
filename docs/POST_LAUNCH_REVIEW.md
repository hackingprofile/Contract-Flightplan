# Post-launch review template

Not a claim that the product has launched. Use after a public release.

| Area | Evidence to collect | Rank |
| --- | --- | --- |
| Adoption | independent dry-run completions of `scripts/quickstart.ps1` | |
| False confidence | cases where unknown/unsupported was shown vs guessed pass | |
| Defects | verify mismatches, stale digest reports, mapping overflow | |
| Provider | confirm hosted modes remained not_enabled unless a later consent path is accepted | |
| Maintenance | Rust/toolchain drift, fixture corpus growth, Action permission creep | |

Next-version items must be evidence-ranked from those rows. Do not promote backlog ideas without a recorded incident or trial note.
