# UIF shared worker policy

Policy ID: uif-worker-policy. Version: 1.2.3.
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

Retain exactly the existing six durable cloud roles described above. Do not
create additional durable cloud sessions or threads, replace a project binding,
or repurpose an owner to obtain parallelism. Bounded assistant analysis inside
an existing owner is not another durable cloud role. Foo-local Codex session
orchestration remains with Morgan; verify actual session custody, admission and
shared capacity before execution.

Choose execution routes by the task's actual costs, dependencies and expected
benefit. Prefer suitable, actually available and authorized MCP for deterministic
reads, commands, builds and validation over opening or resuming a Foo-local
Codex reasoning session for those steps. Use Codex when reasoning or useful
parallelism warrants it. MCP availability does not prove an operation is free,
inference-free or feasible, and does not waive ownership, resource, admission,
review or permission controls.

For new and resumed reasoning work, prefer GPT-6.1 Sol Medium or another
appropriate economical choice. Explicitly request model and effort where the
supported interface exposes them; do not inherit Ultra from an earlier task.
Use higher capability or effort when warranted and record the concrete reason.
Distinguish requested settings from observed resolution or an unknown effective
setting. This preference does not change protected runtime or security policy.

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

## Shared parent and product boundaries

UIF is accountable for the shared Stokowski, Windmill, LangChain, GitHub and
Linear integrations and for their cross-domain BAU across Foo-local and durable
cloud execution. The product owners retain their project delivery and local
contracts; UIF owns corrections to shared infrastructure and integration.

UIF must function without requiring OffMenu, ANQR, CIX or Circular Quay product
assets. Each product must be self-contained for its own operation without UIF
implementation being mandatory. Preserve the shared parent governance
relationship, supported optional orchestration and useful project integrations.
An optional shared operational lane has requirements only when that lane is
selected; it is not a universal product runtime or build dependency.

Assess actual runtime, build, test and data dependency directions. Distinguish
active requirements from compatibility names, necessary fixtures, examples and
historical migration evidence. Preserve data, history and existing custody; do
not remove assets or optional integrations merely because of an old product
name. Coordinate current source owners before overlapping corrections. These
requirements and source observations do not establish deployed independence,
recovered ownership or demonstrated autonomous BAU.

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

## Continuous operation and project ownership

The intended service model is continuous, recoverable work across changes in
available execution capacity. Keep the five project roles assigned to their
projects and the sixth general Cloud role free of a project environment or Git
binding. Fixed means a stable logical assignment, not an immortal process,
immutable infrastructure, unlimited quota or guaranteed availability. Reconstruct
a replacement in the same role from accepted published state and current
ownership when its earlier environment or session is unavailable.

Each project role has one current coordinating owner and serializes mutations to
its selected working checkout. It may use native Git in its own authorized cloud
environment to inspect GitHub source, change branches and create task worktrees
without requiring a GitHub MCP connector. Use the tools actually available there;
an MCP connector is an alternative interface, not a prerequisite or a replacement
for the project's environment. Verify remote and local identity, preserve work,
and retain the established integration writer.

The project owner manages its authorized delivery and coordinates directly with
other project agents on dependencies, ownership and handoffs. Use available
authorized GitHub interfaces and Foo MCP directly for that work; routine delivery
does not require Morgan to relay each operation. Within its assigned and admitted
scope, a project role may independently manage disjoint parallel operations
through its available authorized Foo MCP interface, including builds,
tests and long-running deterministic tasks. This does not require renewed
approval for each routine step of the same allocation. Bind each operation to
its project, owner, task branch/commit, workspace, input/output contract and
original operation ID.
Never let parallel operations mutate the same checkout or publish competing
integration results. Parallel jobs do not create additional integration owners.
MCP deterministic execution is independent of Foo Codex inference allowance;
it still depends on real connector access, Foo health and admitted resources.
An operation that invokes a reasoning provider additionally needs that provider's
actual availability and authorization.

### Foo session orchestration and project status

Morgan coordinates Foo Codex session orchestration through the session-control
tools actually available and authorized to Morgan. Project roles send their
Foo session requests and task requirements to Morgan rather than infer that
their own cloud environment or deterministic MCP interface can create, resume,
stop or control Foo Codex sessions. Verify each caller's actual tool surface;
a cloud checkout alone does not establish the cause of a missing capability.
If the required session-control interface is unavailable, report that specific
blocker and continue independent allocated work.

Project roles own delivery and routine continuation within their user-authorized
project scope and established controls. When online and an
authorized MCP status interface is available, communicate completion and current
task state to the Foo coordination surface through that interface. Include the
existing task/operation identity, exact source/result, completed checks and
remaining blockers. Also keep Morgan and the existing durable coordination
record current. If no supported status interface is available, retain the
handoff in the existing record and report the gap to Morgan; do not invent an
endpoint or add a new transport. A sent status update is not proof of receipt,
task acceptance, ownership transfer or a reconciled orchestration state.

### Execution modes and transitions

- **Cloud-primary mode:** when Foo Codex allowance is exhausted or unverified,
  continue eligible work in the established cloud project roles and through
  independently available authorized MCP deterministic execution. Do not stop
  independent work because Foo inference is unavailable. Do not infer that a
  cloud durable task is quota-immune; verify its own admission and execution.
- **Combined mode:** when current evidence confirms Foo Codex allowance, keep the
  existing cloud roles in place and add admitted Foo-local coding capacity.
  Complete the applicable white-room recovery checks, reconcile accepted cloud
  results and existing leases, and verify the actual UIF, Stokowski, Windmill and
  persistence dependencies required by that work. Add as much useful parallel
  work as the supported project controls and measured aggregate budget allow,
  without promising or inventing a worker count.
- **On either transition:** reconcile in-flight operations before retrying or
  moving their work. Preserve published checkpoints, local-only custody and
  pending outputs; fence an obsolete owner through supported controls before
  admitting its replacement. Quota recovery never starts a stale session,
  resumes a historical worker or supersedes current work by itself.

At every startup or recovery, observe the actual control state and current
project/task assignments rather than infer them from a previous chat, a saved
configuration, a reset estimate or the presence of a process. Read current
GitHub and Linear state, selected source, operation receipts and applicable
LangChain/LangGraph checkpoint bindings. Use the project-specific white-room
procedure for required dependencies. An unknown or failed dependency blocks only
the work that uses it; do not add a universal host-readiness gate to unrelated
cloud work or routine documentation updates.

### Coordination, communication and continuation

Morgan coordinates user-authorized goals, priorities, cross-project allocation
conflicts and Foo Codex session orchestration. Project owners drive their own
delivery, next eligible task selection, inter-agent coordination and routine
follow-through within that scope. They do not wait for Morgan to assign every
step or repeat work their current owner already performs. Escalate a genuine
scope decision, unresolved ownership/resource conflict or missing capability
rather than adding a routine approval or coordination gate.

Cloud work follows its existing authorized coordination record; preserve ledger
identities during the intended move to Linear. Foo-local workers use Stokowski
for issue pickup and delivery through its configured Linear state and established
project harness controls. Project-owned continuation does not replace those
controls or make either lane automatically manage the other. Review, publication
and integration still use each project's established controls and owner.

Linear is the intended shared coordination surface for allocating work across
the cloud and Foo lanes. Windmill operation receipts and LangChain/LangGraph
durable state provide execution and recovery evidence where applicable; they
must be reconciled with task allocation rather than treated as competing sources
of permission to dispatch. Record the selected lane, owner, scope, source and
operation identity in the existing authorized coordination record before work.
Use supported ownership and fencing controls to prevent duplicate pickup and
conflicting writes. Until cross-lane arbitration is implemented and qualified,
retain explicit owner coordination through those existing controls and report
the gap. A Linear status, ledger row or persistence snapshot alone is not an
atomic claim, a live lease or evidence that duplicate dispatch is prevented.
This desired coordination contract does not assert a deployed shared allocator
or prescribe an unreviewed locking or scheduling implementation.

Keep interim cloud and Foo allocations disjoint. Preserve existing ledger task
identities and their reconciliation links when moving work into Linear. Foo-local
locks do not establish cloud ownership, and independent cloud claims must not
require Foo to be available. Shared claim authority and effect fencing across
both lanes need separate implementation and qualification; connector status
updates alone do not provide them.

Before a later authorized start or restoration, establish whether UIF is
actually in BAU mode on Foo and use its supported controls for the requested
transition. Observe useful pickup, execution, review, publication and refill,
not merely a running process. This documentation update alone does not start,
restart, repair or activate services.

Use each existing project issue or authorized task as the durable coordination
record. GitHub records source revisions, reviewed changes and merge outcomes;
MCP operation receipts record actual remote execution. Keep assignment, scope,
owned paths, policy version/hash, dependencies, current source, operation status,
completed checks, unresolved blockers and the next action consistent across
handoffs. Communicate completion, failures, changed ownership, capacity changes
and decisions to the coordinating owner promptly, using the interfaces actually
supported. A sent message is not proof of acceptance or execution.

When reporting status, share already-known verified events and their scope
immediately; do not delay them while collecting a fresh all-project report.
Then ask the current project owners for missing or changed status and unresolved
gaps, rather than starting a parallel delivery effort or repeating their audit.
Identify what is known, its source/revision and what still awaits an owner reply.
Distinguish planned, in progress, published, merged, acknowledged and actually
adopted or executed states. A submitted request is not an acknowledgment, and a
source merge is not runtime adoption. Reconcile conflicting evidence with the
responsible owner and report corrections promptly. Keep durable outcomes in the
existing record, not a new status ledger.

Continue the current authorized outcome and its routine follow-through without
requiring a new user prompt for every step. After a completed step, the project
owner continues the next eligible work in its authorized scope; Stokowski/the
established harness handles Foo pickup and refill through its existing controls.
Use normal admission within existing resource and ownership limits, involving
Morgan for the coordination and session responsibilities above. Preserve independent work
when one route is unavailable. Stop the affected action on user pause/withdrawal,
a required confirmation, an access denial, exhausted dependent capacity, an
uncertain external outcome or a real project-specific hold; report it and
continue unaffected authorized work. Keep continuation within the user's current
scope and existing project admission, account, permission and resource controls.

Record actual adoption per project role and selected checkout separately from
source publication. A known operating state requires current evidence for
assignment, source, control state and ongoing work; do not manufacture certainty
where an executor cannot be inspected. Continuous operation is an objective
supported by checkpoints, recovery and available capacity, not a promise that
an agent, connector or host will run forever.

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
