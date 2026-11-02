# Security Policy

Contract Flightplan reads OpenAPI documents, mappings, proposals and fixtures as **untrusted input**. It runs locally, calls no model or network service, and treats an approval as valid only for the exact files it was made for.

## Supported versions

| Version | Supported |
| --- | --- |
| 0.1.x | Yes |

## Reporting a vulnerability

**Do not** file security reports in public issues.

Report it privately through GitHub: open the repository's **Security** tab and choose **Report a vulnerability** (GitHub private vulnerability reporting). The report stays private between you and the maintainers until an advisory is published.

- Describe the command, the input shape and the output you saw.
- Replace real contract contents, credentials and other private data with placeholders.
- Do not publish exploit details before a fix or advisory is available.

## Scope

In scope: anything that lets crafted input bypass a verification or approval binding, escape the files and folders a command is allowed to read or write, make the tool reach the network or a model provider, crash it, or exhaust resources beyond the documented limits.

The threat model, its known limits and the provider boundary are described in [`docs/THREAT-MODEL.md`](docs/THREAT-MODEL.md) and [`docs/PROVIDER-BOUNDARY.md`](docs/PROVIDER-BOUNDARY.md).
