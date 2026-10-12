# Security policy

## Supported versions

RIM is pre-release. Until the first tagged release, only `master` is supported: a fix lands
there and nowhere else. Once releases are tagged, this section names the ones that get fixes.

## Reporting a vulnerability

Report it privately through GitHub's private vulnerability reporting on this repository: open
the Security tab and choose "Report a vulnerability". Never open a public issue, discussion or
pull request for a vulnerability, and do not post it in a chat room.

Include what you can of:

- **The version**: the output of `rim_server --version`, or the commit you built.
- **The setup**: the server role (relay, host or both), whether it runs behind a reverse proxy,
  and any setting the problem depends on.
- **The steps**: what to send, in what order, to reproduce it. A script or a failing test is
  welcome.
- **The impact**: what an attacker gains and what they need first (an account, a session, a
  peer server, a position on the network).
- **Disclosure**: whether anyone else knows, and any date you plan to publish on.

Remove secrets, session cookies and personal data from logs before you attach them.

## What happens next

- An acknowledgement within seven days of the report.
- A fix, or a decision with its reasons, within ninety days.
- Credit in `CHANGELOG.md` under `Security` when the fix ships, under the name you give, if you
  want it.

## Scope

This policy covers this repository: the server, its GraphQL and HTTP API, the
server-to-server transport and the crates in this workspace. The desktop and web clients in
`rim-ui` and the command line client in `rim-cli` have their own `SECURITY.md`; report a client
problem there. When you cannot tell which side a problem is on, report it here.

## Under outside review

`docs/security/crypto-review-brief-2026-10.md` is the brief for the outside cryptographic
review of key identity, device linking, recovery and end-to-end encryption. Its section 6 lists
the known gaps and open questions; a report that matches one is still welcome, especially with
an attack the brief does not describe.
