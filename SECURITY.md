# Ponduin security

Ponduin is a local AI agent with access to files, terminal commands and optional
external services. Security reports are handled privately by PondSec.

## Reporting a vulnerability

Do not publish suspected vulnerabilities, credentials, private data or working
exploits in a public issue.

Use the repository's private security-advisory workflow to report:

- command execution or permission bypasses;
- unsafe file access;
- credential or secret exposure;
- prompt-injection paths with material impact;
- insecure update or installation behavior;
- dependency vulnerabilities that affect Ponduin.

Include the affected version, platform, reproduction steps, expected behavior,
observed behavior and any proposed mitigation.

## Response process

PondSec will triage reports, reproduce the issue, assess impact and coordinate a
fix and release. Disclosure timing is agreed with the reporter after affected
users can update safely.

## Security expectations

- Never commit credentials or customer data.
- Keep local execution and permission boundaries explicit.
- Treat model output and extension content as untrusted input.
- Validate update sources and release artifacts.
- Add regression tests for security fixes.

General company information is available at [pondsec.com](https://pondsec.com).
