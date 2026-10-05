# Gitflow

| Branch | Starts from | Pull request target | Purpose |
| --- | --- | --- | --- |
| `main` | Initial scaffold | — | Public release history; default branch |
| `develop` | `main` | — | Integration for the next release |
| `feature/<topic>` | `develop` | `develop` | New behavior |
| `fix/<topic>` and other task prefixes | `develop` | `develop` | Routine changes |
| `release/<version>` | `develop` | `main`, then `main` to `develop` | Stabilization and release |
| `hotfix/<topic>` | `main` | `main`, then `main` to `develop` | Urgent released-version fix |

Release and hotfix promotions must originate in this repository. External
contributors use forks for ordinary task branches. Maintainers may target a
`release/*` branch with a `fix/*`, `hotfix/*`, `docs/*`, `chore/*`, or Dependabot
branch. Dependabot security updates may target `main`; back-merge them into
`develop` afterward. This exception is restricted to GitHub's Dependabot identity.

`main` and `develop` are created at setup. Short-lived branches are created only
when there is approved work; empty example branches are unnecessary. Nested task
names are supported. Rules target both `release/**/*` and `hotfix/**/*`.

Use merge commits for promotions and back-merges. Squash-only merging and linear
history enforcement are incompatible with this chosen Gitflow topology. Automatic
branch deletion is disabled so merging a back-merge cannot delete `main`. Delete
finished task branches deliberately after checking their work was integrated.

Protected integration and release branches require pull requests, one approval,
resolved conversations, stale-approval dismissal, and current required checks.
No force pushes are allowed. Permanent branches cannot be deleted. Reviews are
required on protected release and hotfix branches once they exist.

The intended review ruleset allows organization owners a **pull-request-only**
review bypass for the initial single-maintainer period. It does not bypass the
separate mandatory-checks, force-push, or deletion rulesets. Use this only for an
explicitly reviewed solo-maintainer decision and explain it in the PR. Remove the
exception once a second trusted reviewer is available. Codex must not invoke it
without explicit authorization for that PR.

After release approval, tag the tested `main` commit as `vX.Y.Z`, then create a
GitHub Release with compatibility notes, checksums, and provenance for any assets.
Release tags are protected against updates and deletion; only organization owners
may create them under the intended tag-creation rule. No release automation or
deployment runs during preparation.
