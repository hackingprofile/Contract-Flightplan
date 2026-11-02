# Host matrix

Every push to `main` runs the full CI on three hosts: `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, a release build and the shipped-packs round-trip.

| Host | Status | Evidence |
| --- | --- | --- |
| Linux (`ubuntu-24.04`, rustc 1.98.1) | executed in CI | the full CI above, including the Unix-only tests (`#[cfg(unix)]`: device, FIFO and socket outputs, a FIFO swapped in during reads) and the Linux-only `/dev/full` test; every script also ran under PowerShell 7 |
| Windows (`windows-2022`, rustc 1.98.1) | executed in CI | the full CI above; `scripts/sbom.ps1` also runs under Windows PowerShell 5.1 |
| macOS (`macos-15`, rustc 1.98.1) | executed in CI | the full CI above, including the Unix-only tests |
| Generated Node suite (`node --test`) | executed | runs inside `cargo test` on every CI host against the release binary |
| Generated Python suite (unittest) | executed where Python 3 is present | runs inside `cargo test`; `scripts/run-generated-tests.ps1` reports the suite as PASS when Python 3 is available and as unexecuted (UNEXECUTED) when it is not |

Do not treat cross-compilation, linted workflow YAML, or generated-but-unrun test files as runtime proof.
