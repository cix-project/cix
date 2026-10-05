# Owner activation

The repository is reserved and configured only. Its existence is not permission
to start using it. The owner must explicitly approve activation.

1. Record the owner's approval, its scope, and date in this document via a reviewed
   setup change. Never infer approval from elapsed time or completion of setup.
2. Confirm the exact source checkout and history to import. Review secrets, private
   data, licenses, third-party notices, and artifact sizes before uploading them.
3. Establish real product build, test, fuzzing, and reproducible benchmark commands.
   Require the applicable checks; a passing scaffold check is insufficient.
4. Confirm security scanning and branch rules from GitHub read-back evidence.
5. Only after the approval is recorded, an owner may set repository Actions variable
   `CIX_DEVELOPMENT_APPROVED=true` and update the paused notices.
6. Connect Codex to this repository with only the required repository access and
   select the intended development environment. Existing remotes remain unchanged
   until source migration is specifically approved.

The Gitflow policy check fails pull requests while the repository variable is
unset or false. Push checks still verify setup files. The variable is a workflow
guard, not a substitute for GitHub access control; authorized administrators can
change it. No CI job has permission to activate the repository.

## Approval record

Status: **not approved**.
