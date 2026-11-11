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

Protected integration and release branches require pull requests, resolved
conversations, stale-approval dismissal, and current required checks. Use the
shared project standard of independent review with exact source and validation
evidence. Do not add a separate owner sign-off or formal GitHub approval when
the applicable live branch rules require none. Check those rules at integration
time and satisfy any approvals they actually require. No force pushes are
allowed, and permanent branches cannot be deleted.

The active review ruleset currently requires zero formal GitHub approvals.
Its organization-owner bypass allowance is not needed for a normal merge that
satisfies the rules. Routine independently reviewed work follows the normal
merge path without a separate CIX-only approval step. This documentation grants
no permission to weaken protections or invoke a bypass; mandatory platform
confirmations, required checks, release controls and project-specific scope
restrictions remain in effect.

After release approval, tag the tested `main` commit as `vX.Y.Z`, then create a
GitHub Release with compatibility notes, checksums, and provenance for any assets.
Release tags are protected against updates and deletion; only organization owners
may create them under the intended tag-creation rule. No release automation or
deployment runs during preparation.
