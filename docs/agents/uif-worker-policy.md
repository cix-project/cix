# UIF shared worker policy

Policy ID: uif-worker-policy. Version: 1.1.1.
Maintained in maldous/uif at docs/agents/uif-worker-policy.md.

## Scope and precedence

This user-owned policy applies to coordinators, native assistant subagents,
cloud durable tasks, MCP callers and independently started project work on Foo
or future compute. It does not modify assistant system instructions or grant
access. Follow current user directions, mandatory platform safety requirements,
project-specific contracts and applicable nested instructions. Preserve narrower
project holds, ownership, review, tests, data boundaries and publication controls.
Do not substitute deliverables, change requested scope/design, or add delivery
gates. Reuse valid authorization for the same scope and necessary corrections;
mandatory confirmations and access restrictions still apply. Documentation is
not permission to resume historical workers or change production configuration.


## Fixed project agents and the general Cloud role

The operating model has five dedicated project-agent roles: UIF, ANQR, OffMenu,
Circular Quay and CIX. Each project role remains assigned to its own project
and authorized repository selection; do not silently repurpose a role or replace
its repository binding with another project's checkout. CIX product and
operations repositories remain separate surfaces of the CIX role. Verify current
visibility before publication; intended visibility is not actual visibility and
does not authorize disclosing private operations material.

Each dedicated project agent uses its own cloud Git checkout, CPU allocation
and storage, independent of Foo. Cloud files and execution do not live on Foo
merely because that agent can call a Foo MCP tool. Isolation is an execution
boundary, not a claim of dedicated physical hardware, guaranteed capacity,
technical immutability or uninterrupted availability. Verify actual provisioning,
admission and resource limits; a configured role is not proof of a running agent.

For project agents, `develop` is expected when available and is the normal
merge destination under the project's established review and integration controls.
After a merge, return the project agent's working checkout to current `develop`
when available, preserving any unrelated or recoverable work. This keeps the
integration baseline available for a subagent that needs a new worktree. Create
that worktree on its own task branch from current `develop` when available.
If `develop` is unavailable, record the actual project baseline and continue
permitted work under the project's established Git flow; its absence is not an
unconditional gate. Do not overwrite divergent work or bypass ownership, review
or admission controls to establish this working practice.

A sixth general Cloud role has no Git configuration or repository binding. It handles
authorized general coordination or non-repository work using its actual available
tools and storage. Keep project-specific Git work with its dedicated project owner, or use an
explicitly scoped authorized MCP target identifying the project, repository, owner
and branch without adding Git configuration or a repository binding to Cloud. It does not become a sixth project or a replacement integration
writer.

When needed and authorized, either role may use the user's MCP gateway for
deterministic commands, builds/tests or host-configuration work. Before a command,
verify the target host, project repository, worktree, branch/commit, ownership and
applicable permissions. Keep cloud reasoning/checkouts and remote execution
receipts distinct. Gateway availability is optional for independent cloud work
and does not grant root authority or permission for unrelated host changes.
Follow required platform confirmations and never use the gateway to bypass
denied access or an exhausted reasoning route.

Additional Foo-local Codex sessions may join the execution pool after allowance
is demonstrably restored and their project-specific startup and ownership checks
pass. They supplement the fixed cloud roles; they do not replace them or resume
suspended historical sessions automatically. Preserve accepted cloud work and
reconcile superseded local work before any local admission.

Treat this operating model as required startup knowledge for root and nested
project agents. The root entrypoint and this local versioned copy apply alongside
narrower project instructions. Pass the same version/hash through handoffs,
including to independently started Foo sessions. Report configuration or adoption
mismatches explicitly; publishing this policy does not configure accounts, create
agents, change repository bindings, activate services or prove universal adoption.

## Discover the real execution pool

Keep these independent dimensions explicit in each task's current routing record:

- Native assistant/cloud subagent availability: actual admission and running
  state through the available assistant tools. These are not Codex durable
  sessions and do not consume a declared Foo Codex worker slot merely by existing.
- Cloud durable task admission: the actual accepted task identity, provisioning
  result, environment, dependencies, ownership and subsequent activity. A create
  request, saved configuration or queued task alone is not a running worker.
  A cloud label does not imply quota immunity: admission can succeed before
  execution fails for quota. Do not repeatedly dispatch into a known exhausted
  route; a reset estimate is not evidence of restored availability.
- MCP deterministic execution: the permitted connector, target host/workspace,
  script or operation identity, input contract and terminal receipt. An MCP
  connection does not itself provide a reasoning model or a Codex session.
- Codex inference on Foo or another executor: supported current provider/quota
  evidence, session admission and activity, plus that executor's applicable
  repository, tool, storage, service and resource prerequisites. A connected Foo
  host is not evidence that Codex allowance is available; quota is not host health.

Use eligible native/cloud/MCP capacity independently of Foo Codex allowance.
Expand the pool when additional Codex or future-compute capacity is actually
available and authorized, with current admission and ownership checks. Remove
unavailable capacity from new assignments when quota, access or health changes;
reconcile existing operation outcomes and preserve checkpoints before reassignment.
Do not cancel an independent worker because another provider exhausted quota.
Do not infer an outage from silence or retry an uncertain admission. Check the
same operation/task ID first. Record unknowns rather than inventing slots, quota,
provider/model resolution or successful work. A ceiling is not a utilization target.

A provider-specific limitation blocks only work requiring that provider. Continue
eligible independent work without bypassing a denied operation through another
route. Quota recovery alone never revives stale ownership or suspended jobs.
Use the project's supported admission and aggregate resource rules; do not create
a parallel scheduler, change limits or introduce a universal new readiness gate.

## Startup and handoff, including independent Foo sessions

Read this repository's AGENTS.md, this local policy copy, the local policy
manifest and relevant project/nested instructions at startup and reassignment.
Confirm the selected repository, current branch/commit, common Git directory,
checkout owner and actual task scope. Consult current authoritative issue,
accepted source and operation evidence through permitted interfaces. If remote
access is blocked, state the local baseline and limitation; do not claim it current.

Resolve task and write ownership before mutation. Preserve dirty/untracked work,
local-only commits, custody mappings and project-specific historical holds.
Use a task-specific clean branch/worktree and the established integration writer.
No reset, clean, deletion, force push, stale replay or conflicting writer is
implied by standardization. Applicable project build/test/review requirements
remain intact; do not turn a focused check into whole-project acceptance.

Pass each child the exact task, repository/revision, applicable instruction paths
and policy version/hash, owned paths, current authorization/holds, actual executor,
resource budget, operation identities and required outcome. A child must resolve
these inputs in its own environment: parent filesystem paths are not transfers.
If a required instruction is inaccessible, report that narrow dependency and
continue unrelated authorized work. Do not claim all agents loaded instructions
merely because a parent linked a document.

Before replacing or expanding workers, reconcile leases and prior outcomes using
supported project controls. Keep code checkpoints, exact test results, pending
operations and next steps recoverable in permitted storage. Local commits are
not remote publication; verify remote readback before claiming a pushed checkpoint.
Keep secrets and raw datasets out of source commits and handoff exports.

## Portable distribution, not implicit deployment

Each participating repository carries this same versioned local file and a
manifest with its SHA-256 and its own repository identity. A repo-root AGENTS.md
references the local copy so a standalone clone needs no sibling UIF checkout,
absolute Foo path, network fetch, submodule initialization or symlink traversal.
UIF is the maintenance source; copies change together in reviewed source changes.
The manifest pins bytes, not proof of approval or runtime enforcement. Preserve
project-specific instructions rather than overwriting them with this policy.

Existing worktrees and already-running chats do not automatically adopt a change
on another branch. The coordinator must deliver the updated policy to active
workers and record actual source adoption separately from prepared candidates.
New independent sessions must use an adopted checkout and read its entrypoint.
Do not edit frozen historical snapshots or vendored third-party agent contracts.
A stale/missing copy is a reported instruction-distribution gap, not authority to
rewrite history or impose a new completion gate on unrelated product work.

MCP/Windmill instruction references are advisory unless a supported, authorized
execution boundary actually validates them. Never claim enforcement from prose,
a prompt_for_ai field, a schema example, a local checker or an undeployed patch.
