# Security policy

## Reporting a vulnerability

Please report suspected vulnerabilities privately via GitHub's **Report a
vulnerability** (Security advisories) on the repository, rather than a public
issue. We aim to acknowledge within a few days. Pre-1.0, only the latest release
line is supported with fixes.

## Threat model (a terminal has a real one)

Skelly is a local dev tool that sees the user's shell, code, and keystrokes. The
security posture is part of the product's trust contract, not an afterthought.

- **Privacy by default.** No telemetry, no phone-home, no analytics without an
  explicit, off-by-default, documented opt-in. Command contents, file contents,
  paths, repo names, and environment are never logged at default levels or sent
  anywhere.
- **Untrusted terminal output.** Bytes from programs flow straight into the ANSI
  parser; it is fuzzed and honors only safe control sequences. Sensitive
  operations (clipboard write, any command-execution escape) are gated on the
  user, never performed blindly.
- **Untrusted config / themes / repos.** Config and theme files are parsed
  defensively and never executed. Opening a repo does not implicitly run its git
  hooks.
- **Non-destructive rewind.** The session timeline restores past states in a
  shadow worktree and never rewrites history or moves HEAD/refs (Hard rule 3). Any
  code path that could mutate the user's real branch is treated as a
  security-grade bug and tested adversarially.
- **Secrets.** The only secrets are CI release-signing credentials; they live only
  as CI secrets, never in the repo or logs.

## Supply chain

`cargo deny` gates advisories, licenses, bans, and sources on every PR;
`cargo audit` runs daily against the RustSec database. `Cargo.lock` is committed
and CI builds `--locked`.
