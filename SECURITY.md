# Security status and reporting

**English** · [Русский](SECURITY.ru.md)

Alcard is under development and has no supported public binary release series yet. Source tests and VM results are not a production security certification. Known release gates are recorded in the [roadmap](docs/en/roadmap.md); the actual authentication and recovery contracts are in [security architecture](docs/en/security.md).

## Report a vulnerability

Use GitHub's **Report a vulnerability** entry for this repository if private vulnerability reporting is enabled and available to you. This documentation change does not enable or verify that repository setting. If the private entry is unavailable, use an existing private contact with the maintainer; no dedicated reporting email or response-time commitment has been configured in this repository. Establishing an available private reporting channel remains a public-opening task.

Do not post passwords, signing keys, tokens, personal logs or an uncoordinated security exploit in a public issue. A useful private report identifies the affected source commit/package, installation mode, device or isolated VM, prerequisites, reproduction steps, impact and sanitized evidence. Ordinary non-security bugs can use the normal issue tracker.

## Areas requiring particular care

Firstboot account provisioning, doas/su, polkit/PAM, session locking and recovery, privileged radio helpers, package signing and image release verification are security-sensitive boundaries. Preserve the independent backend checks and process boundaries when modifying them. Security fixes need relevant behavior checks on an isolated Alpine system, not only source-string assertions.

Before publishing an image, complete the release gates for developer-credential separation, strict root/Hello state verification, signing provenance and current installed-package acceptance. The documentation records these gaps rather than asserting they have been fixed.
