# Agent capability routing

This guide covers optional maintenance assistance. It does not add a CIX build
or runtime dependency, authorize product activation, or replace the selected
repository instructions, current issue scope, source custody or release controls.

## Bind the task and environment

Read [AGENTS.md](../AGENTS.md), the current issue and the applicable source
contract. Record the repository, branch, full commit/tree, owned paths, executor,
instruction and skill source/version or digest, and actual verification result
in the existing coordination record. Preserve accepted checks at their exact
still-valid source bindings and reconcile original operations before a retry.

Verify each cloud, Foo and CI environment separately. A path on another machine
is not a transferred file or an installed tool. Keep these observations distinct:

- **Readable:** the actual instruction or skill file can be opened.
- **Discoverable:** the executing caller's catalog or supported discovery lists it.
- **Executable:** the intended binary or supported command ran successfully here.
- **Authenticated:** the intended account was verified through the selected route.
- **Authorized:** the current task permits this operation and destination.

The first four do not establish the fifth. Record a precise missing capability
and continue independent permitted work, without creating a universal product
gate, installing a substitute service, borrowing credentials or weakening controls.

## Start with the existing product documentation

| Task | Source-owned entry point |
| --- | --- |
| Product purpose, build and platform boundary | [Root README](../README.md), [native README](../rust/cix-native/README.md), root/native Makefiles and Cargo manifests |
| Governance, security and contribution flow | [Contributing](../CONTRIBUTING.md), [security](../SECURITY.md), [Gitflow](gitflow.md) and [activation](activation.md) |
| Rust buffer and standard-stream consumers | [Rust guide](../rust/cix-native/packaging/sdk/RUST.md), `core`, `standard_stream` and the registered `native_buffer` example |
| C and C++ native interfaces | [SDK guide](../rust/cix-native/packaging/sdk/README.md), the shipped headers under `rust/cix-native/include`, and their matching FFI implementation |
| Optional language bindings | The selected language's guide and manifest under `rust/cix-native/bindings`; a language package does not supply an arbitrary native library |
| Host and storage integrations | The selected guide and contract under `rust/cix-native/adapters`; distinguish an adapter source from qualified host behavior |
| Formats and full-engine protocol | `rust/cix-native/docs/formats`, the selected source implementation and its actual compatibility evidence |
| Package and release work | `rust/cix-native/packaging`, [release procedure](../rust/cix-native/docs/release/RELEASING.md), notices and the selected artifact/provider/member requirements |

Do not substitute a historical SDK edition, example or version string for the
actual selected API. Source presence, a generated manifest and an accepted
documentary association are distinct from compilation, installation, consumer
execution, platform qualification and release approval. Keep public product
source and private operations evidence in their respective custody.

## Select only relevant capabilities

| Capability | CIX use and boundary |
| --- | --- |
| repository-docs-review | Use the shared revision-bound workflow where actually available. Resolve it through the executing environment's catalog or verified explicit file route and read its SKILL.md. Use tracked-file enumeration and scoped source searches; keep disposable incremental indexes outside the shipped repository. An index is a retrieval aid, not semantic or completeness proof. Report a distribution gap without creating a competing copy or external indexing service. |
| Rust, C/C++ and binding-language guidance | Match the selected source language, declared toolchain and lockfiles. The native source requires Rust 1.98+ and C++17 plus its declared codec libraries. Select binding-specific tooling only for that binding. Use actual project commands and report their exact scope; do not invent a lint target or treat an ambient compiler as the pinned toolchain. |
| Format, style and security guidance | Use Markdown/source review for repository prose and format-specific document/PDF skills for those artifacts. Apply relevant security guidance to an actual security question or affected boundary, without relabeling ordinary docs work as a completed security audit. |
| Impeccable | Use official guidance for a selected presentation surface. This native CLI/SDK tree has no application frontend or web build/start target; documentation presentation is Read-mode work, not permission to invent an app. Verify readable skill, launcher/engine and target separately. Preserve the selected visual brief; never enable an unsafe fallback or relax sandbox/CSP to demonstrate a tool. Static detection does not prove rendered accessibility or device behavior. |
| Context7 | Use current official documentation for an actual external library/API and selected version when the supported connector is available. It is not a private-repository index or source-correctness authority. If unavailable, use official primary documentation through an existing permitted route; do not install or authenticate a new service implicitly. |
| Cloudflare and Netlify | Use only for a separately selected real CIX website, domain or infrastructure task. This native product tree declares no Wrangler/Netlify hosting configuration. Verify the actual account, resource, source/artifact and publisher before effects; MCP and CLI authentication are separate. Preserve existing project pins and do not migrate hosting or deploy as a tooling check. |
| GitHub and Linear | Use exact source/review/check/publication evidence and the existing coordination issues. Keep current writer/publisher custody, immutable identities and original operation receipts. Native HTTPS Git credentials and an authenticated connector are separate capabilities. Do not export private operations records into public source or comments. |

The independent Chronicle/website and private operations surfaces retain their
own instructions, source selections, access and publisher. Their existence does
not make a website, provider account, orchestration service or design engine a
dependency of the CLI, library or SDK.

## Deterministic checks and recovery

Follow the actual selected environment's shared check workflow and deployed
catalog, not a copied list of presumed capabilities. For a supported published
source-branch route, discover the branch's full commit/tree, require that exact
immutable expected commit at execution, and inspect the original terminal inner
result. A moved source, successful wrapper, queued task or running host job is
not a passed check. Reconcile uncertain outcomes under the original identity;
never repeat accepted work merely to relabel its executor.

The governance Python checks in AGENTS.md do not establish native correctness.
Native build, focused unit/integration tests, installed consumers, service
features, external providers, platforms, packages and benchmarks have distinct
inputs and acceptance. Choose necessary changed-path checks and declared bounds;
report unsupported or unrun stages without a blanket permanent exclusion.

Keep shared skills, indexes, private receipts and environment policy outside
product runtime dependencies. Documentation changes do not install a tool,
register a skill globally, unpause admissions, transfer a claim, grant access,
change an approval variable, activate a service or publish a release.
