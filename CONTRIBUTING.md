# Contributing

Development is paused until the owner approves activation. Please do not submit
product contributions during preparation.

After activation, discuss substantial changes in an issue before implementation.
Use bug reports for reproducible defects, feature requests for proposed behavior,
and Discussions for questions. Report vulnerabilities privately via `SECURITY.md`.

Branch from `develop`, use a descriptive `feature/`, `fix/`, `docs/`, `chore/`,
`refactor/`, `test/`, `build/`, or `ci/` name, and open a pull request to `develop`.
See `docs/gitflow.md` for release and hotfix exceptions. Maintainers use merge
commits to preserve Gitflow ancestry. Rebase or merge the current target into your
task branch when required; never rewrite a protected branch.

Every pull request should explain the problem, behavior change, validation, and
compatibility or security impact. Keep generated files and private data out of the
patch. All required checks and review conversations must pass before merging.
Changes to ownership, workflows, dependencies, and security need particular care.

Run the commands in `AGENTS.md`. The current checks cover this scaffold only.
Repository language-specific build and test instructions will be added with the
approved source import.

Communicate respectfully, discuss the work rather than the person, and do not
publish private information. Maintainers may moderate abusive contributions.
