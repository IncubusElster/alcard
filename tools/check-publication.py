#!/usr/bin/env python3
"""Check tracked source and reachable Git history before publication.

Only Git objects reachable from local refs are inspected; this does not inspect
remote caches, forks, Actions artifacts, or credentials held outside Git.
Matches are never printed because an audit log must not disclose a secret.
"""

import argparse
from collections import defaultdict
from pathlib import Path
import re
import subprocess
import sys


SENSITIVE_NAMES = {
    '.env', '.npmrc', '.pypirc', '.netrc', 'id_rsa', 'id_ed25519',
    'credentials.json', 'service-account.json', 'kubeconfig', 'known_hosts.private',
}
SENSITIVE_SUFFIXES = ('.pem', '.p12', '.pfx', '.key', '.keystore', '.kdbx',
                      '.ovpn', '.sqlite', '.db', '.log', '.bak', '.apk', '.img',
                      '.iso', '.zip', '.tar', '.tar.gz', '.tar.xz')
SECRETS = {
    'private-key PEM': re.compile(b'-----BEGIN ' +
                                  rb'(?:RSA |OPENSSH |EC |DSA |ENCRYPTED )?PRIVATE KEY-----'),
    'GitHub credential': re.compile(rb'\b(?:gh[pousr]_[A-Za-z0-9_]{20,}|github_pat_[A-Za-z0-9_]{20,})\b'),
    'AWS access key': re.compile(rb'\b(?:AKIA|ASIA)[A-Z0-9]{16}\b'),
    'OpenAI credential': re.compile(rb'\bsk-' + rb'(?:proj-)?[A-Za-z0-9_-]{24,}\b'),
    'Slack credential': re.compile(rb'\bxox[baprs]-[A-Za-z0-9-]{16,}\b'),
    'Google API key': re.compile(rb'\bAIza[A-Za-z0-9_-]{35}\b'),
    'GitLab credential': re.compile(rb'\bglpat-[A-Za-z0-9_-]{20,}\b'),
    'Stripe live credential': re.compile(rb'\b(?:sk_live|rk_live)_[A-Za-z0-9]{20,}\b'),
    'basic-auth URL': re.compile(rb'https?://[^:@/\s]{1,100}:[^@/\s]{1,100}@'),
}
# These patterns are useful for manual review but can match examples and tests.
REVIEW = {
    'literal credential assignment': re.compile(
        rb'(?i)\b(?:password|passwd|client_secret|api[_-]?key|access_token|refresh_token|'
        rb'auth_token|secret_key|private_key)\s*[:=]\s*[\'\"][^\'\"\r\n]{6,}[\'\"]'),
}


def git(repo: Path, *args: str, data: bytes | None = None) -> bytes:
    return subprocess.check_output(['git', '-C', str(repo), *args], input=data)


def sensitive_name(path: str) -> bool:
    parts = Path(path).parts
    for part in parts:
        lowered = part.lower()
        if lowered in SENSITIVE_NAMES or lowered.startswith('.env.'):
            return True
        if lowered.endswith(SENSITIVE_SUFFIXES) and not lowered.endswith('.rsa.pub'):
            return True
    return bool(parts and parts[0] in {'.alcard-local', '.git', 'dist', 'build'})


def scan(repo: Path) -> tuple[list[tuple[str, str, str, int]], dict[str, int]]:
    if git(repo, 'rev-parse', '--is-inside-work-tree').strip() != b'true':
        raise RuntimeError('not a Git working tree')
    heads = git(repo, 'rev-list', '--all').splitlines()
    if not heads:
        raise RuntimeError('no history reachable from local refs')
    objects = git(repo, 'rev-list', '--objects', '--all').splitlines()
    names: dict[bytes, set[str]] = defaultdict(set)
    for line in objects:
        oid, _, path = line.partition(b' ')
        names[oid].add(path.decode('utf-8', 'replace') if path else '<unnamed Git object>')
    # Include all current filenames, including aliases for a reused blob.
    for entry in git(repo, 'ls-files', '--stage', '-z').split(b'\0'):
        if entry and b'\t' in entry:
            metadata, path = entry.split(b'\t', 1)
            names[metadata.split()[1]].add(path.decode('utf-8', 'replace'))
    types = git(repo, 'cat-file', '--batch-check=%(objectname) %(objecttype)',
                data=b'\n'.join(names) + b'\n').splitlines()
    blobs = [line.split()[0] for line in types if line.endswith(b' blob')]
    findings: list[tuple[str, str, str, int]] = []
    reviews: set[tuple[str, int]] = set()
    for oid in blobs:
        content = git(repo, 'cat-file', 'blob', oid.decode('ascii'))
        path = sorted(names.get(oid, {'<unnamed Git object>'}))[0]
        for label, pattern in SECRETS.items():
            if match := pattern.search(content):
                findings.append(('SECRET', path, label, content.count(b'\n', 0, match.start()) + 1))
        for pattern in REVIEW.values():
            for match in pattern.finditer(content):
                reviews.add((path, content.count(b'\n', 0, match.start()) + 1))
    # Filename review covers both current and historical paths recorded by Git.
    for paths in names.values():
        for path in paths:
            if path != '<unnamed Git object>' and sensitive_name(path):
                findings.append(('FILENAME', path, 'sensitive tracked path', 0))
    return sorted(set(findings)), {'commits': len(heads), 'blobs': len(blobs),
                                    'objects': len(names), 'review_candidates': len(reviews),
                                    'review_locations': sorted(reviews)}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repo', type=Path, default=Path(__file__).resolve().parents[1])
    args = parser.parse_args()
    try:
        findings, counts = scan(args.repo)
    except (subprocess.CalledProcessError, OSError, RuntimeError) as error:
        print(f'BLOCKED: publication history audit could not run ({type(error).__name__})', file=sys.stderr)
        return 2
    for kind, path, label, line in findings:
        # Never print matched secret bytes or file contents.
        print(f'{kind}: {path}:{line} ({label})', file=sys.stderr)
    for path, line in counts['review_locations'][:50]:
        print(f'REVIEW: {path}:{line} (literal assignment, no value disclosed)')
    print('PASS' if not findings else 'FAIL',
          f"publication Git audit: {counts['commits']} reachable commits, "
          f"{counts['blobs']} unique blobs, {counts['objects']} Git objects; "
          f"{counts['review_candidates']} literal-assignment candidates (manual review required)")
    return int(bool(findings))


if __name__ == '__main__':
    raise SystemExit(main())
