use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use contract_flightplan::approve::{approve, check_outputs, supersede, write_json, write_output};
use contract_flightplan::corpus::run_corpus_in_temp;
use contract_flightplan::diff::compare_paths;
use contract_flightplan::error::Error;
use contract_flightplan::fixtures::generate_for_operation;
use contract_flightplan::generate::{write_jest, write_lossiness_witness, write_pytest};
use contract_flightplan::history::reject_proposal;
use contract_flightplan::proposal::load_proposal;
use contract_flightplan::provider::{catalog, consent_preview, propose};
use contract_flightplan::report::{build_report, terminal_line, terminal_text, write_report_pair};
use contract_flightplan::verify::{first_failure, review_proposal, verify_paths_report};

#[derive(Parser)]
#[command(
    name = "cfp",
    version,
    about = "Contract Flightplan: compare, propose, review, approve, verify, and generate local OpenAPI migrations without a model."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Compare two local OpenAPI documents.
    Compare {
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        destination: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
        /// Optional oasdiff-like JSON evidence to attach (not a compatibility verdict).
        #[arg(long)]
        oasdiff: Option<PathBuf>,
    },
    /// Validate a local file proposal or refuse generation in mode none.
    Propose {
        /// none, file, example (local), or a hosted id (not enabled).
        #[arg(long)]
        mode: String,
        #[arg(long)]
        proposal: Option<PathBuf>,
        /// Contracts for --mode example.
        #[arg(long)]
        source: Option<PathBuf>,
        #[arg(long)]
        destination: Option<PathBuf>,
        /// With --mode file: copy a refused proposal here with its diagnostics (exit 1).
        #[arg(long)]
        quarantine: Option<PathBuf>,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Explain and type-check a non-executable proposal before approval.
    /// Exits 3 when a blocking diagnostic exists; the review is still written.
    Review {
        #[arg(long)]
        proposal: PathBuf,
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        destination: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Approve an exact proposal into a versioned migration contract.
    /// Requires the review record `cfp review --out` wrote for this exact
    /// proposal and these contracts.
    Approve {
        #[arg(long)]
        proposal: PathBuf,
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        destination: PathBuf,
        #[arg(long)]
        fixtures: PathBuf,
        /// Review record written by `cfp review --out` for this proposal.
        #[arg(long)]
        review: PathBuf,
        #[arg(long)]
        reviewer: String,
        #[arg(long)]
        note: Option<String>,
        #[arg(long)]
        approved_at: Option<String>,
        #[arg(long)]
        out: PathBuf,
    },
    /// Record a rejected proposal as a local rejection record (not a signature).
    Reject {
        #[arg(long)]
        proposal: PathBuf,
        #[arg(long)]
        reviewer: String,
        #[arg(long)]
        reason: String,
        #[arg(long)]
        out: PathBuf,
    },
    /// Show a migration's hash-chained history. Each event links to the
    /// digest of the one before it; local records, not signatures.
    History {
        #[arg(long)]
        migration: PathBuf,
        /// Check every link and that the approval matches its events; exit 1
        /// when an event was removed, reordered or edited.
        #[arg(long)]
        verify_chain: bool,
    },
    /// Approve a successor proposal that replaces an approved migration.
    /// Writes the new migration (with `supersedes`) to --out and an archive
    /// copy of the old one, lifecycle `superseded`, to --archive.
    Supersede {
        #[arg(long)]
        migration: PathBuf,
        #[arg(long)]
        proposal: PathBuf,
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        destination: PathBuf,
        #[arg(long)]
        fixtures: PathBuf,
        /// Review record written by `cfp review --out` for the new proposal.
        #[arg(long)]
        review: PathBuf,
        #[arg(long)]
        reviewer: String,
        #[arg(long)]
        note: Option<String>,
        #[arg(long)]
        approved_at: Option<String>,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        archive: PathBuf,
    },
    /// Execute an approved migration against catalogued fixtures.
    /// On fixture failure the full report is still written to --out and the exit code is 1.
    Verify {
        #[arg(long)]
        migration: PathBuf,
        /// The reviewed proposal the migration was approved from; its digest must be
        /// the bound one and its rules must equal the migration's.
        #[arg(long)]
        proposal: PathBuf,
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        destination: PathBuf,
        #[arg(long)]
        fixtures: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Write offline HTML, JSON, Markdown, job-summary, and SARIF artifacts.
    /// With --migration the report re-runs verification itself (it needs
    /// --proposal and --fixtures) and takes the verdict only from that run.
    /// --sarif needs that verdict because SARIF results are fixture outcomes.
    Report {
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        destination: PathBuf,
        #[arg(long)]
        proposal: Option<PathBuf>,
        #[arg(long)]
        migration: Option<PathBuf>,
        /// Fixtures folder the migration was approved with; required with --migration.
        #[arg(long)]
        fixtures: Option<PathBuf>,
        /// Optional verify result to check: refused unless it matches the fresh run exactly.
        #[arg(long)]
        verify: Option<PathBuf>,
        #[arg(long)]
        html: PathBuf,
        #[arg(long)]
        json: PathBuf,
        #[arg(long)]
        sarif: Option<PathBuf>,
        #[arg(long)]
        md: Option<PathBuf>,
        #[arg(long)]
        job_summary: Option<PathBuf>,
    },
    /// Approve and verify every pack under a directory, and run the seeded
    /// fixture generator over the bodies each pack's rules read.
    Corpus {
        #[arg(long)]
        packs: PathBuf,
        /// u64 seed for the fixture generator; the same seed gives the same report.
        #[arg(long, default_value_t = 0)]
        seed: u64,
        /// Write the operation x outcome coverage matrix as Markdown.
        #[arg(long)]
        matrix: Option<PathBuf>,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Generate normal/missing/null/malformed/enum-negative and boundary fixture sketches.
    Fixtures {
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        method: String,
        #[arg(long)]
        path: String,
        #[arg(long)]
        location: String,
        #[arg(long)]
        status: Option<String>,
        /// u64 seed for the sample values; the same seed gives the same fixtures.
        #[arg(long, default_value_t = 0)]
        seed: u64,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Validate a JSON payload against one body schema of a contract.
    /// Exits 0 when it validates; otherwise the outcome decides the exit code.
    Validate {
        /// The contract whose body schema the payload must satisfy.
        #[arg(long)]
        schema_of: PathBuf,
        #[arg(long)]
        method: String,
        #[arg(long)]
        path: String,
        #[arg(long)]
        location: String,
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        payload: PathBuf,
    },
    /// List local providers or preview what a disabled outbound provider would receive.
    Provider {
        #[arg(long)]
        list: bool,
        #[arg(long)]
        preview: Option<String>,
        /// Local files to include in the consent preview (repeatable). Nothing is sent.
        #[arg(long = "select")]
        select: Vec<PathBuf>,
    },
    /// Walk one person through compare, propose, review, approve, verify and
    /// report in the terminal. Nothing is approved without answering y to
    /// "Approve this mapping? [y/N]" or passing --yes.
    Guide {
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        destination: PathBuf,
        #[arg(long)]
        fixtures: PathBuf,
        /// Folder for every file the guide writes.
        #[arg(long)]
        out_dir: PathBuf,
        /// A person's proposal file; without it the built-in proposer suggests one.
        #[arg(long)]
        proposal: Option<PathBuf>,
        /// Recorded as the reviewer; asked for when absent.
        #[arg(long)]
        reviewer: Option<String>,
        #[arg(long)]
        note: Option<String>,
        #[arg(long)]
        approved_at: Option<String>,
        /// Approve without asking (for non-interactive use, with --reviewer).
        #[arg(long)]
        yes: bool,
    },
    /// Generate Jest, Pytest, or lossiness witness files from a pack.
    Generate {
        #[arg(long)]
        kind: String,
        #[arg(long)]
        pack: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
}

fn main() -> ExitCode {
    match Cli::try_parse() {
        Ok(cli) => match run(cli) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                // Error text quotes input: shown escaped, never as raw control
                // characters or a line that starts a workflow command.
                // Nothing more can be reported if stderr itself is closed.
                let _ = writeln!(std::io::stderr(), "{}", terminal_line(&err.to_string()));
                ExitCode::from(err.exit_code() as u8)
            }
        },
        Err(err) => {
            if err.use_stderr() {
                let _ = write!(
                    std::io::stderr(),
                    "{}",
                    terminal_text(&err.render().to_string())
                );
                ExitCode::from(3)
            } else {
                let _ = err.print();
                ExitCode::SUCCESS
            }
        }
    }
}

fn run(cli: Cli) -> Result<(), Error> {
    match cli.command {
        Commands::Compare {
            source,
            destination,
            out,
            oasdiff,
        } => {
            guard(
                &[out.as_deref()],
                &[
                    Some(source.as_path()),
                    Some(destination.as_path()),
                    oasdiff.as_deref(),
                ],
            )?;
            let mut diff = compare_paths(&source, &destination)?;
            if let Some(path) = oasdiff {
                diff.imported_evidence = contract_flightplan::oasdiff::ingest_oasdiff(&path)?;
            }
            emit(&diff, out.as_deref())?;
        }
        Commands::Propose {
            mode,
            proposal,
            source,
            destination,
            quarantine,
            out,
        } => {
            guard(
                &[out.as_deref()],
                &[
                    proposal.as_deref(),
                    source.as_deref(),
                    destination.as_deref(),
                ],
            )?;
            let validated = match mode.as_str() {
                "file" => {
                    let path = proposal.ok_or_else(|| Error::Invalid {
                        message: "file provider requires --proposal".into(),
                    })?;
                    contract_flightplan::provider::propose_file(&path, quarantine.as_deref())?
                }
                "example" => {
                    let (Some(source), Some(destination)) = (source, destination) else {
                        return Err(Error::Invalid {
                            message: "--mode example requires --source and --destination".into(),
                        });
                    };
                    let request = contract_flightplan::provider::ProposalRequest::new(
                        &source,
                        &destination,
                        &[],
                    )?;
                    let provider = contract_flightplan::provider::ExampleLocalProvider {
                        source,
                        destination,
                    };
                    contract_flightplan::provider::run_provider(&provider, &request)?.proposal
                }
                other => propose(other, proposal.as_deref())?,
            };
            emit(&validated, out.as_deref())?;
        }
        Commands::Review {
            proposal,
            source,
            destination,
            out,
        } => {
            guard(
                &[out.as_deref()],
                &[
                    Some(proposal.as_path()),
                    Some(source.as_path()),
                    Some(destination.as_path()),
                ],
            )?;
            let review = review_proposal(&proposal, &source, &destination)?;
            emit(&review, out.as_deref())?;
            if review.is_blocked() {
                return Err(Error::Invalid {
                    message: format!(
                        "review found {} blocking type-check diagnostic(s); approve will refuse this proposal",
                        review.blocking_diagnostics
                    ),
                });
            }
        }
        Commands::Approve {
            proposal,
            source,
            destination,
            fixtures,
            review,
            reviewer,
            note,
            approved_at,
            out,
        } => {
            guard(
                &[Some(out.as_path())],
                &[
                    Some(proposal.as_path()),
                    Some(source.as_path()),
                    Some(destination.as_path()),
                    Some(review.as_path()),
                    Some(fixtures.as_path()),
                ],
            )?;
            let migration = approve(
                &proposal,
                &source,
                &destination,
                &fixtures,
                &review,
                &reviewer,
                note.as_deref(),
                approved_at.as_deref(),
            )?;
            write_json(&out, &migration)?;
        }
        Commands::Reject {
            proposal,
            reviewer,
            reason,
            out,
        } => {
            guard(&[Some(out.as_path())], &[Some(proposal.as_path())])?;
            let proposal = load_proposal(&proposal)?;
            let rejection = reject_proposal(&proposal, &reviewer, &reason, None)?;
            write_json(&out, &rejection)?;
        }
        Commands::History {
            migration,
            verify_chain,
        } => {
            let loaded = contract_flightplan::approve::load_migration(&migration)?;
            if verify_chain {
                contract_flightplan::history::check_history(&loaded)?;
                emit(
                    &serde_json::json!({
                        "chain": "ok",
                        "events": loaded.history.len(),
                        "head": loaded.history.last().map(|e| e.action.clone()),
                        "lifecycle": loaded.lifecycle,
                    }),
                    None,
                )?;
            } else {
                emit(&loaded.history, None)?;
            }
        }
        Commands::Supersede {
            migration,
            proposal,
            source,
            destination,
            fixtures,
            review,
            reviewer,
            note,
            approved_at,
            out,
            archive,
        } => {
            // The archive may replace the old migration file itself (revoking it in
            // place); no other output may land on an input.
            let inputs = [
                Some(proposal.as_path()),
                Some(source.as_path()),
                Some(destination.as_path()),
                Some(review.as_path()),
                Some(fixtures.as_path()),
            ];
            guard(&[Some(out.as_path()), Some(archive.as_path())], &inputs)?;
            guard(&[Some(out.as_path())], &[Some(migration.as_path())])?;
            let (new, archived) = supersede(
                &migration,
                &proposal,
                &source,
                &destination,
                &fixtures,
                &review,
                &reviewer,
                note.as_deref(),
                approved_at.as_deref(),
            )?;
            write_json(&out, &new)?;
            write_json(&archive, &archived)?;
        }
        Commands::Verify {
            migration,
            proposal,
            source,
            destination,
            fixtures,
            out,
        } => {
            guard(
                &[out.as_deref()],
                &[
                    Some(migration.as_path()),
                    Some(proposal.as_path()),
                    Some(source.as_path()),
                    Some(destination.as_path()),
                    Some(fixtures.as_path()),
                ],
            )?;
            let report =
                verify_paths_report(&migration, &proposal, &source, &destination, &fixtures)?;
            emit(&report, out.as_deref())?;
            if let Some(err) = first_failure(&report) {
                return Err(err);
            }
        }
        Commands::Report {
            source,
            destination,
            proposal,
            migration,
            fixtures,
            verify,
            html,
            json,
            sarif,
            md,
            job_summary,
        } => {
            guard(
                &[
                    Some(html.as_path()),
                    Some(json.as_path()),
                    sarif.as_deref(),
                    md.as_deref(),
                    job_summary.as_deref(),
                ],
                &[
                    Some(source.as_path()),
                    Some(destination.as_path()),
                    proposal.as_deref(),
                    migration.as_deref(),
                    fixtures.as_deref(),
                    verify.as_deref(),
                ],
            )?;
            if sarif.is_some() && migration.is_none() {
                return Err(Error::Invalid {
                    message: "--sarif requires --migration, --proposal and --fixtures; SARIF results are the fixture outcomes of the report's own verification run"
                        .into(),
                });
            }
            let verify_report = match verify {
                Some(path) => Some(load_verify(&path)?),
                None => None,
            };
            let report = build_report(
                &source,
                &destination,
                proposal.as_deref(),
                migration.as_deref(),
                fixtures.as_deref(),
                verify_report.as_ref(),
            )?;
            write_report_pair(&report, &json, &html)?;
            if let Some(md_path) = md {
                contract_flightplan::report::write_markdown(&md_path, &report)?;
            }
            if let Some(summary_path) = job_summary {
                write_output(
                    &summary_path,
                    contract_flightplan::report::render_job_summary(&report).as_bytes(),
                )?;
            }
            if let (Some(sarif_path), Some(checked)) = (sarif, report.verify.as_ref()) {
                let sarif_doc = contract_flightplan::sarif::from_verify(checked)?;
                write_json(&sarif_path, &sarif_doc)?;
            }
        }
        Commands::Guide {
            source,
            destination,
            fixtures,
            out_dir,
            proposal,
            reviewer,
            note,
            approved_at,
            yes,
        } => {
            let options = contract_flightplan::guide::GuideOptions {
                source,
                destination,
                fixtures,
                out_dir,
                proposal,
                reviewer,
                note,
                approved_at,
                yes,
            };
            let stdin = std::io::stdin();
            let mut input = stdin.lock();
            let stdout = std::io::stdout();
            let mut out = stdout.lock();
            contract_flightplan::guide::run_guide(&options, &mut input, &mut out)?;
        }
        Commands::Corpus {
            packs,
            seed,
            matrix,
            out,
        } => {
            guard(
                &[matrix.as_deref(), out.as_deref()],
                &[Some(packs.as_path())],
            )?;
            let report = run_corpus_in_temp(&packs, seed)?;
            if let Some(path) = matrix {
                write_output(
                    &path,
                    contract_flightplan::corpus::render_matrix(&report).as_bytes(),
                )?;
            }
            if !report.stable {
                emit(&report, out.as_deref())?;
                return Err(Error::Invalid {
                    message: "corpus had failing packs".into(),
                });
            }
            emit(&report, out.as_deref())?;
        }
        Commands::Fixtures {
            source,
            method,
            path,
            location,
            status,
            seed,
            out,
        } => {
            guard(&[out.as_deref()], &[Some(source.as_path())])?;
            let generated = generate_for_operation(
                &source,
                &method,
                &path,
                &location,
                status.as_deref(),
                seed,
            )?;
            emit(&generated, out.as_deref())?;
        }
        Commands::Validate {
            schema_of,
            method,
            path,
            location,
            status,
            payload,
        } => {
            let schema = contract_flightplan::fixtures::body_schema_for(
                &schema_of,
                &method,
                &path,
                &location,
                status.as_deref(),
            )?;
            let value = contract_flightplan::proposal::load_json_value(&payload)?;
            contract_flightplan::schema::validate_instance(&schema, &value, "payload")?;
            emit(&serde_json::json!({"valid": true}), None)?;
        }
        Commands::Provider {
            list,
            preview,
            select,
        } => {
            if let Some(id) = preview {
                emit(&consent_preview(&id, &select)?, None)?;
            } else if list {
                emit(&catalog(), None)?;
            } else {
                return Err(Error::Invalid {
                    message: "provider requires --list or --preview <id>".into(),
                });
            }
        }
        Commands::Generate { kind, pack, out } => match kind.as_str() {
            // No generated file may land inside the pack it reads.
            "jest" | "pytest" => {
                contract_flightplan::approve::check_output_folder(&out, &[pack.as_path()])?;
                if kind == "jest" {
                    write_jest(&pack, &out)?;
                } else {
                    write_pytest(&pack, &out)?;
                }
            }
            "lossiness" => {
                guard(&[Some(out.as_path())], &[Some(pack.as_path())])?;
                write_lossiness_witness(&pack, &out)?;
            }
            other => {
                return Err(Error::Unknown {
                    what: "generate_kind".into(),
                    location: other.into(),
                    message: "supported kinds are jest, pytest, lossiness".into(),
                });
            }
        },
    }
    Ok(())
}

/// Refuse, before anything runs, outputs that would replace one of the inputs
/// or share one path.
fn guard(
    outputs: &[Option<&std::path::Path>],
    inputs: &[Option<&std::path::Path>],
) -> Result<(), Error> {
    let outputs: Vec<&std::path::Path> = outputs.iter().flatten().copied().collect();
    let inputs: Vec<&std::path::Path> = inputs.iter().flatten().copied().collect();
    check_outputs(&outputs, &inputs)
}

fn emit<T: serde::Serialize>(value: &T, out: Option<&std::path::Path>) -> Result<(), Error> {
    match out {
        Some(path) => write_json(path, value),
        None => {
            let text = serde_json::to_string_pretty(value).map_err(|e| Error::Invalid {
                message: e.to_string(),
            })?;
            // A closed pipe or a full disk is an I/O error (exit 3), not a panic.
            let mut stdout = std::io::stdout().lock();
            writeln!(stdout, "{text}")
                .and_then(|()| stdout.flush())
                .map_err(|e| Error::io("standard output", e))
        }
    }
}

fn load_verify(path: &std::path::Path) -> Result<contract_flightplan::verify::VerifyReport, Error> {
    let value = contract_flightplan::proposal::load_json_value(path)?;
    serde_json::from_value(value).map_err(|e| Error::Parse {
        path: path.display().to_string(),
        message: e.to_string(),
    })
}
