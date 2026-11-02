# Privacy and reports

- Compare, review, approve, verify, report, corpus, generate, and file propose read only the local paths you pass. They do not open network connections.
- Remote `$ref` values are rejected.
- Absolute paths given on the command line are reduced to their file name before they are recorded in diff, migration, report, SARIF or generated-test files. Relative paths keep their relative form with `/` separators.
- Generated Node/Python suites embed no local paths; they read `CFP_PACK` and `CFP_BIN` from the environment.
- `cfp provider --preview --select` shows only key structure and type markers, never scalar values.
- Reports must not include credentials. Fixtures in this repository are synthetic.
- HTML reports are static: no remote scripts, no remote stylesheets, CSP `default-src 'none'` with inline CSS only.
- Fixture values and diagnostics are HTML-escaped. A string such as `<script>alert(1)</script>` is shown as text.
- Error messages name fields and outcome kinds. They do not dump entire files.

Local input is the default and the only implemented input mode.
