# RFCs

Substantial changes go through the RFC process before implementation.
ADRs ([`docs/adr/`](../docs/adr/)) record decisions; RFCs solicit input on
proposals.

## When an RFC is required

Open an RFC if your change touches any of:

- DynamoDB wire-protocol response shapes or new API operations
- The `Storage` trait or any storage backend contract
- SigV4 authentication or the authorization model
- On-disk format, schema, or migration semantics
- Public CLI flags or the configuration file format

An RFC is *not* required for bug fixes, dependency bumps, internal refactors
that preserve public behavior, documentation, or tests.

If you are unsure, open an issue describing the change. A maintainer will tell
you whether an RFC, an ADR, or neither is needed.

## Process

1. **Draft.** Copy `0000-template.md` to `rfcs/0000-my-proposal.md` (keep `0000`
   until accepted). Open a PR. The PR is the discussion thread.
2. **Under Review.** A maintainer assigns reviewers from
   [`.github/CODEOWNERS`](../.github/CODEOWNERS). Discussion happens inline on
   the PR.
3. **Final Comment Period (FCP).** When reviewers are aligned, a maintainer
   announces a 7-day FCP in the PR and updates `FCP ends:` in the RFC. If no
   new objections surface, the RFC is accepted at the end of FCP.
4. **Accepted.** The PR is merged with a real number assigned by the maintainer
   (next unused `NNNN`). Implementation PRs reference the RFC by number.
5. **Rejected or Withdrawn.** Merged into `rfcs/` with `Status: Rejected` or
   `Status: Withdrawn` so the rationale is preserved for future contributors.

## Index

<!-- Add accepted RFCs here as they land. -->

_(none yet)_
