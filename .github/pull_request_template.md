## Purpose

Describe the problem and solution. Link the relevant issue or roadmap entry.

## Affected areas

- Modes: Graphics / Graphics Minimal / Console / Hello / packaging
- Security, signing, networking, power or hardware boundaries:
- Documentation / translations affected:

## Verification

Record what actually ran and where (host, Alpine ARM64, VM, real Cardputer).

- [ ] Relevant tests and lint passed, or limitations are documented.
- [ ] Installed package or VM evidence is supplied if this changes installed behavior.
- [ ] No private keys, credentials, unsanitized logs or build outputs are committed.
- [ ] English/Russian docs and EN/RU/ES interface strings are kept consistent where applicable.

Full acceptance uses one runner sequentially and is not required for every change. See [testing](../docs/en/testing.md). Passing CI does not establish physical Cardputer Zero acceptance.
