# Local release tree

`scripts/assemble-v1.ps1` builds a local, unsigned release tree (default `dist/release`). Nothing is published, tagged, signed or uploaded.

| Path | Content |
| --- | --- |
| `export/` | the public source tree, as `scripts/export.ps1` writes it |
| `bin/<os>-<arch>/cfp[.exe]` | the binary for the host that ran the assembly, built from `export/` with `--locked`, a fresh target directory and `--remap-path-prefix`, so no build-machine path is embedded |
| `schemas/` | the published JSON schemas (`schemas/v1`) |
| `sbom/cfp.cdx.json` | a minimal CycloneDX 1.5 document built by `scripts/sbom.ps1` from `cargo metadata --locked` (no SBOM tool is used): every package in the build and test graph with version, purl and declared licence, and the dependency edges |
| `FIXTURE-DIGEST.txt` | the SHA-256 of every file under `packs/`, and one digest over that listing |
| `SUPPORT-MATRIX.md` | the supported constructs and the hosts that were and were not executed |
| `SIGNATURES.md` | states that the tree is unsigned and carries no attestation |
| `checksums/SHA256SUMS.txt` | the SHA-256 of every other file in the tree |

`scripts/verify-local-release.ps1 -Tree dist/release` checks every checksum and that no file is unlisted, runs the binary and compares its version with `export/Cargo.toml`, refuses a binary that embeds a user-profile, home or cargo-registry path, checks that the SBOM is CycloneDX 1.5 for this package and that `SIGNATURES.md` says the tree is unsigned, and runs the README quickstart from a copy of `export/` on a fresh target directory. It exits 1 on any failure.

Limits: each assembly contains only the binary for its own host, built and executed on that host; there is no signature, attestation or provenance statement, so the checksums show that files are unchanged since assembly, not who assembled them; the version stays `0.1.0`.
