# Documentation inventory and scope map

This inventory binds public product source `cix-project/cix` at commit
`7ab4b32950b95c18ce5288cb300697b290a5258d`, tree
`a0f549647376b7a4273dca5147f052db0cfb9b63`. It is a discovery and gap-disposition
map, not a new release gate, executable qualification or assertion that every
source comment, API, example and document has received a semantic review.
CIX maintainers own these product documents. Private operations, research,
execution receipts and independent website/Chronicle source are separate surfaces
and are not reproduced here. Update affected rows when their source changes;
a future branch is not automatically equivalent to this snapshot.

## Enumeration and interpretation

The inspected tree contains **669 tracked paths**, including **72 Markdown
files**, **three CLI manpages**, **21 CMakeLists.txt build definitions** and
**ten other .txt licence/notice files**. Six extensionless LICENSE files carry
additional source terms. There is one root `AGENTS.md`, no nested `AGENTS.md` or
`AI.md`, and no tracked repository-local skill package. This inventory predates
itself. A text-extension search includes build definitions; they are not silently
counted as prose. Headers, source comments and examples are referenced separately.

Each row below identifies exact inspected Git bytes, audience and canonical
location. A source blob is not a test receipt or a licence conclusion. Identical
format-reference copies are retained rather than independently rewritten:
`rust/cix-native/FORMAT-CIXB1.md` and `FORMAT-CIXZ1.md` are byte-identical to their
counterparts under `docs/formats/` at this revision. Prefer the format directory
for protocol navigation and keep any affected duplicate synchronized.

## Category coverage and disposition

| Category | Canonical reference / present source | Scope and remaining work |
| --- | --- | --- |
| Agent instructions | [AGENTS](../AGENTS.md), [capability routing](agent-capabilities.md), [contributing](../CONTRIBUTING.md) and [Gitflow](gitflow.md) | Project source, privacy, review and release controls remain local. Shared maintenance policy/tools are optional. Read the actual executor's pinned bundle/profile and current instructions; a URL does not prove installation or access. Active approval/preparation prose requires correspondence with current owner records and controls, not inference from a source import or green check. |
| Project architecture and build | [Root overview](../README.md), [native guide](../rust/cix-native/README.md), workspace/native Cargo manifests and Makefiles | Native CLI/SDK plus a narrower portable crate. The root lockfile-generation example is a release-export step; ordinary consumers retain the selected lockfile and use the applicable locked build. No standalone ADR collection is tracked; do not invent an adopted whole-product architecture. |
| Native CLI / configuration | Native guide, [cix manpage](../rust/cix-native/man/cix.1), [uncix](../rust/cix-native/man/uncix.1), [cixcat](../rust/cix-native/man/cixcat.1), `src/cli.rs` and `src/i18n.rs` | Three declared native binaries, option/resource/file semantics and source-local diagnostics. A manpage is not measured streaming, all-locale or platform qualification. Read selected option/parser/caller code for a behavioral change. |
| SDK and code interfaces | [SDK install guide](../rust/cix-native/packaging/sdk/README.md), [Rust consumers](../rust/cix-native/packaging/sdk/RUST.md), five public headers and source modules below | Buffer, CIXG1 incremental stream, ordinary formats and Zstd dictionary APIs have distinct contracts. The current edition has no native runtime capability-query function in `cix.h` or `core`; do not borrow another edition's API. Path-level discovery is complete here; exhaustive symbol/example semantic coverage is not asserted. |
| Language and host bindings | Five binding README files and adapter guides in the full inventory | Optional dependencies and native acquisition are binding-specific. Python's pure binding does not bundle a DSO; its selected Zarr/Numcodecs receipts do not qualify every tuple. Source manifests/recipes do not establish registry ownership, platform execution or a published package. |
| Wire and full-engine contracts | [Compatibility](../rust/cix-native/docs/formats/COMPATIBILITY.md), three named format documents, [job protocol](../rust/cix-native/docs/formats/JOB-PROTOCOL.md), bridge guides | CLI/full-engine, process-free SDK and portable subset are different capabilities. Recognized container names do not prove every encoder/provider path, legacy decoder, dependency closure or option combination is qualified. |
| Portable/WASM | [Portable adapter](../rust/cix-native/adapters/wasm/README.md), `rust/cix-portable/src/lib.rs`, browser/Node helpers | Bounded CIXG1 raw/RLE/selected route-2 subset, not a frontend application or full native selector. Three native source modules are referenced by path; documented shared-crate extraction/distribution work remains separate. The tiny HTML file loads a real browser contract module; source presence does not re-execute or broaden its historical browser receipts. |
| Optional services / providers | [Network adapters](../rust/cix-native/adapters/network/README.md), [gRPC provisioning](../rust/cix-native/adapters/network/grpc/PROVISIONING.md), optional Cargo `server` feature and native job protocol | Declared service dependencies and adapters are not an installed standalone service, configured identity, approved exposure or completed full-engine provider closure. No app frontend or Wrangler/Netlify build configuration is declared in this native product checkout. |
| Tests, examples and fixture history | Cargo targets, `rust/cix-native/tests`, 15 files in `rust/cix-native/examples`, fixture README files below | Preserve each original source/result binding. Source examples, fixture descriptions, historical accepted checks and a newly executed exact-branch check are different facts. Do not rerun accepted suites simply to fill inventory rows. |
| Security, rights and provenance | Root/native security policies, LICENSE files, notices, vendor upstream records and [notice staging](../rust/cix-native/packaging/notices/STAGING-INPUT.md) | Dependency terms, source obligations, selected provider notices and publication rights require their actual records. Included text is not proof every distribution tuple is legally/technically closed or a completed security audit. |
| Packages, platforms and release | [Release procedure](../rust/cix-native/docs/release/RELEASING.md), package READMEs, Gitflow and native platform section | Linux/macOS/Windows/Homebrew/SDK recipes and staged installs remain distinct from built/target-tested artifacts and released registries. Freeze exact source/artifacts, use only supported-platform claims and follow current release review. No production publication or rollback readiness is established by this map. |
| User/support and operations | Root/native support/security documents, conduct and moderation guides | Public support and source contribution are not private deployment runbooks. Keep provider secrets, account/runtime receipts and private operational history out of public source. |
| Implicit state and feature completeness | Source modules and current issue-specific requirements | No exhaustive implicit-state inventory or full code-documentation semantic acceptance is claimed. Continue source-bound machine/API/example reviews separately; a document count cannot satisfy those obligations. |

## Source and interface entry points

All paths below exist in the pinned tree; inspect the matching implementation and
selected test/consumer before changing an interface. They are navigation, not an
exported-symbol count or whole-code review certificate.

| Surface | Source entry points | Contract boundary |
| --- | --- | --- |
| Rust library | `rust/cix-native/src/lib.rs`, `core.rs`, `incremental.rs`, `standard_stream.rs` | Current source API/feature gates and buffer/stream limits, not a foreign ABI or every full-engine route. |
| Native CLI | `rust/cix-native/src/main.rs`, `uncix.rs`, `cixcat.rs`, `cli.rs`, `cli_full_engine.rs` | Native argv, file/pipe and full-engine orchestration; read callers and selected resource limits. |
| C/C++ SDK | `rust/cix-native/include/cix.h`, `cix_stream.h`, `cix_formats.h`, `cix_dictionary.h`, `cix.hpp`; `src/c_api.rs`, `c_stream_api.rs`, `c_format_api.rs`, `c_dictionary_api.rs` | Four C contract families and C++ convenience wrapper; no full-engine/runtime-query inference from version0.1.0. |
| Language bindings | `rust/cix-native/bindings/{python,node,java,go,dotnet}` | Each has its own source, dependency/acquisition and consumer requirements. No Swift/Dart package implementation is invented from a backlog title. |
| Portable subset | `rust/cix-portable/src/{lib,cixg1,limits,wasm}.rs`, `rust/cix-native/adapters/wasm` | Deliberately limited codec/profile; native shared-source references are implementation bridges. |
| Full engine and protocol | `rust/cix-native/src/full_engine`, `job_cli.rs`, `full_engine/job_protocol.rs`, `cli_full_engine.rs` | Provider/worker/environment and archive dispatch have separate closure and qualification. |
| External host adapters | `rust/cix-native/adapters` and associated CMake/Make/generation files | Host/plugin/kernel/network interfaces are opt-in, not automatic installation or upstream acceptance. |

## Complete Markdown inventory

Audience/category labels are navigation judgments. They do not turn a fixture or
historical upstream note into a current operational instruction. All rows are
maintained by the CIX product source owner within their existing upstream terms.

| Document | Audience / category | Inspected Git blob |
| --- | --- | --- |
| [.github/pull_request_template.md](../.github/pull_request_template.md) | Governance/community; maintainers / contributors | `52b0d4bc1e62a48d5b2e803e104ffc81f9f1b2e5` |
| [AGENTS.md](../AGENTS.md) | Agent/maintenance instructions; agents | `e29b2b4f452b0658ede811426defc4195958b1db` |
| [CODE_OF_CONDUCT.md](../CODE_OF_CONDUCT.md) | Governance/community; maintainers / contributors | `23f7ed94ec1872fc4559d3e20d8e8ac397a62ac4` |
| [CONTRIBUTING.md](../CONTRIBUTING.md) | Governance/community; maintainers / contributors | `6ead50624abd1e64b595912bf336070b7601832e` |
| [README.md](../README.md) | Project/build overview; contributors / users | `1d4192ea911c7ceeb4ddf7d9f70f0831131532bb` |
| [SECURITY.md](../SECURITY.md) | Governance/community; maintainers / contributors | `74611131233a171e0795fef341022c90b53fdcc8` |
| [SUPPORT.md](../SUPPORT.md) | Governance/community; maintainers / contributors | `e9fcb602b0c8143ae7883ea5043aff3714ae40f9` |
| [THIRD_PARTY_NOTICES_RUST.md](../THIRD_PARTY_NOTICES_RUST.md) | Source terms and attribution; maintainers / distributors | `d02dab96443b66cb1d1711dd84bee181adb402d3` |
| [docs/activation.md](../docs/activation.md) | Governance/community; maintainers / contributors | `37938a4bca0aeeae2bee7e7294457b34670c6d86` |
| [docs/agent-capabilities.md](../docs/agent-capabilities.md) | Agent/maintenance instructions; agents | `35059ab8c004c69fd08feb1a8000895164810dac` |
| [docs/gitflow.md](../docs/gitflow.md) | Governance/community; maintainers / contributors | `2290e91753d3085b1e8d74aabd65984673b57ef8` |
| [docs/moderation.md](../docs/moderation.md) | Governance/community; maintainers / contributors | `347c86950a356c5d6bad9ecc84de4a372513ab33` |
| [rust/cix-native/FORMAT-CIXB1.md](../rust/cix-native/FORMAT-CIXB1.md) | Wire/protocol contract; codec implementers | `fd92dce0a5f4c4f0b3de94ecb5af7a56b26f7527` |
| [rust/cix-native/FORMAT-CIXZ1.md](../rust/cix-native/FORMAT-CIXZ1.md) | Wire/protocol contract; codec implementers | `4625ff77e2b6a940ccab218fb5155bfcd6782b88` |
| [rust/cix-native/README.md](../rust/cix-native/README.md) | Project/build overview; contributors / users | `6c7a6575ba283e1350a02f30311963014c93d581` |
| [rust/cix-native/SECURITY.md](../rust/cix-native/SECURITY.md) | Governance/community; maintainers / contributors | `a17a3cc7bc9bd896aa3043735db7946b8bcde0ee` |
| [rust/cix-native/THIRD_PARTY_NOTICES_RUST.md](../rust/cix-native/THIRD_PARTY_NOTICES_RUST.md) | Source terms and attribution; maintainers / distributors | `d02dab96443b66cb1d1711dd84bee181adb402d3` |
| [rust/cix-native/adapters/blosc2/README.md](../rust/cix-native/adapters/blosc2/README.md) | Optional host integration; adapter developers | `024a6356123b76106fa11065689870073521ccb2` |
| [rust/cix-native/adapters/gio/README.md](../rust/cix-native/adapters/gio/README.md) | Optional host integration; adapter developers | `09d79f3c41e8a8e385649737c8ecb0eefe174cb8` |
| [rust/cix-native/adapters/gstreamer/README.md](../rust/cix-native/adapters/gstreamer/README.md) | Optional host integration; adapter developers | `44874d1430656c67e02f861f202f3a1e63059c17` |
| [rust/cix-native/adapters/hadoop/README.md](../rust/cix-native/adapters/hadoop/README.md) | Optional host integration; adapter developers | `869cf20f102c6a29f22287bc11fd64db3acf079c` |
| [rust/cix-native/adapters/hdf5/README.md](../rust/cix-native/adapters/hdf5/README.md) | Optional host integration; adapter developers | `bab9f746347bd167ce785a85143e0c234792e9d1` |
| [rust/cix-native/adapters/libarchive/README.md](../rust/cix-native/adapters/libarchive/README.md) | Optional host integration; adapter developers | `9d8d33244223dd6470759ea8bfded8c16eb0a239` |
| [rust/cix-native/adapters/network/README.md](../rust/cix-native/adapters/network/README.md) | Optional host integration; adapter developers | `58c62aae139db46eeb5a86d532f8e3aff2fbe6f8` |
| [rust/cix-native/adapters/network/grpc/PROVISIONING.md](../rust/cix-native/adapters/network/grpc/PROVISIONING.md) | Optional host integration; adapter developers | `2fa19db19318c18a8f43a724141774ba696b5a77` |
| [rust/cix-native/adapters/rocksdb/README.md](../rust/cix-native/adapters/rocksdb/README.md) | Optional host integration; adapter developers | `874719d47582f141520bde9a9cb67631b4231b2a` |
| [rust/cix-native/adapters/sevenzip/README.md](../rust/cix-native/adapters/sevenzip/README.md) | Optional host integration; adapter developers | `a3e569d1dea20fa832f8fdaf4c0394b7385bb4d7` |
| [rust/cix-native/adapters/squashfs/kernel/README.md](../rust/cix-native/adapters/squashfs/kernel/README.md) | Optional host integration; adapter developers | `329bef9eb2cebfff95ffe2c571637202099b5de7` |
| [rust/cix-native/adapters/squashfs/profile/README.md](../rust/cix-native/adapters/squashfs/profile/README.md) | Optional host integration; adapter developers | `f6c93517a3ccfd497a56f74e2a2f11bc217792fa` |
| [rust/cix-native/adapters/squashfs/profile/SPEC.md](../rust/cix-native/adapters/squashfs/profile/SPEC.md) | Optional host integration; adapter developers | `7a060cd0abd8d4340ad9f6b539e79a2bbfafd3a6` |
| [rust/cix-native/adapters/squashfs/profile/tests/native_differential.md](../rust/cix-native/adapters/squashfs/profile/tests/native_differential.md) | Optional host integration; adapter developers | `c2340a841d3fc14324151110bcc2021aaca6e7a2` |
| [rust/cix-native/adapters/squashfs/tools/README.md](../rust/cix-native/adapters/squashfs/tools/README.md) | Optional host integration; adapter developers | `55a987c8d8b583a13f26bea491c64ae159528f79` |
| [rust/cix-native/adapters/squashfs/userspace/README.md](../rust/cix-native/adapters/squashfs/userspace/README.md) | Optional host integration; adapter developers | `1ab06c434b48961963b887fd7e802077f0970096` |
| [rust/cix-native/adapters/storage-examples/README.md](../rust/cix-native/adapters/storage-examples/README.md) | Optional host integration; adapter developers | `a648f72145578af9fcc14310f997b998c2ec86b3` |
| [rust/cix-native/adapters/wasm/README.md](../rust/cix-native/adapters/wasm/README.md) | Optional host integration; adapter developers | `17665ea2b48ecb8d5d2cd25f988a1014f94531f1` |
| [rust/cix-native/adapters/wiredtiger/README.md](../rust/cix-native/adapters/wiredtiger/README.md) | Optional host integration; adapter developers | `c918f78a6b9579b043dfdb1327fae147db70b239` |
| [rust/cix-native/bindings/dotnet/README.md](../rust/cix-native/bindings/dotnet/README.md) | Language/host API; SDK consumers | `5ea2b20f530e77f728af5289f9cbbe010df3a590` |
| [rust/cix-native/bindings/go/README.md](../rust/cix-native/bindings/go/README.md) | Language/host API; SDK consumers | `25fc50785290a15c3760d5e2d8bd48d96609e84c` |
| [rust/cix-native/bindings/java/README.md](../rust/cix-native/bindings/java/README.md) | Language/host API; SDK consumers | `715fcaf37d46d379069f8ad874e2ee34f910a447` |
| [rust/cix-native/bindings/node/README.md](../rust/cix-native/bindings/node/README.md) | Language/host API; SDK consumers | `bdcaeda6394145fab173d9050c0ac40b652734cb` |
| [rust/cix-native/bindings/python/README.md](../rust/cix-native/bindings/python/README.md) | Language/host API; SDK consumers | `3057fc8f1f3c8e6aee17a56687587478ae3fe0eb` |
| [rust/cix-native/bindings/python/docs/NUMCODECS-ZARR.md](../rust/cix-native/bindings/python/docs/NUMCODECS-ZARR.md) | Language/host API; SDK consumers | `5b9957dd984667b1806a47e67b0139e279a21445` |
| [rust/cix-native/codecs/jxl/README.md](../rust/cix-native/codecs/jxl/README.md) | Native bridge contract; engine developers | `3873635b8fd3f91d979672aaac004739b804dadf` |
| [rust/cix-native/codecs/spatial/README.md](../rust/cix-native/codecs/spatial/README.md) | Native bridge contract; engine developers | `1cdd6dda0743b05f52c0a40285628f0f1eeb25e9` |
| [rust/cix-native/docs/bridges/PAQ-V215.md](../rust/cix-native/docs/bridges/PAQ-V215.md) | Native bridge contract; engine developers | `46acdeeb823f68fa162fb426daaacea4518b191e` |
| [rust/cix-native/docs/formats/COMPATIBILITY.md](../rust/cix-native/docs/formats/COMPATIBILITY.md) | Wire/protocol contract; codec implementers | `2da36285902e04b5caaba64bcb673bdbe5fa6ef8` |
| [rust/cix-native/docs/formats/FORMAT-CIXB1.md](../rust/cix-native/docs/formats/FORMAT-CIXB1.md) | Wire/protocol contract; codec implementers | `fd92dce0a5f4c4f0b3de94ecb5af7a56b26f7527` |
| [rust/cix-native/docs/formats/FORMAT-CIXW1.md](../rust/cix-native/docs/formats/FORMAT-CIXW1.md) | Wire/protocol contract; codec implementers | `32b21af0ab939b8a6b1b48af96b4ec2ca8cf06ce` |
| [rust/cix-native/docs/formats/FORMAT-CIXZ1.md](../rust/cix-native/docs/formats/FORMAT-CIXZ1.md) | Wire/protocol contract; codec implementers | `4625ff77e2b6a940ccab218fb5155bfcd6782b88` |
| [rust/cix-native/docs/formats/JOB-PROTOCOL.md](../rust/cix-native/docs/formats/JOB-PROTOCOL.md) | Wire/protocol contract; codec implementers | `0476c077a619bb7809f2787b8f80b150bb56624e` |
| [rust/cix-native/docs/release/RELEASING.md](../rust/cix-native/docs/release/RELEASING.md) | Release/support process; maintainers | `67fe5961e70ab6ba9d627ea2b480b5b789777f2f` |
| [rust/cix-native/docs/release/SUPPORT.md](../rust/cix-native/docs/release/SUPPORT.md) | Release/support process; maintainers | `021c4dcbbd9ae6982c7810e57e43090d270c07f1` |
| [rust/cix-native/packaging/homebrew/README.md](../rust/cix-native/packaging/homebrew/README.md) | Install/distribution contract; packagers | `a981228e3dccccf2b026fc29558b2525907da215` |
| [rust/cix-native/packaging/macos/README.md](../rust/cix-native/packaging/macos/README.md) | Install/distribution contract; packagers | `0ee12138f1746d622ceb8f7f001733f1873526c8` |
| [rust/cix-native/packaging/notices/STAGING-INPUT.md](../rust/cix-native/packaging/notices/STAGING-INPUT.md) | Install/distribution contract; packagers | `4361b20ad303814a33c539c99e92f6ea5ed43a6f` |
| [rust/cix-native/packaging/runtime/README.md](../rust/cix-native/packaging/runtime/README.md) | Install/distribution contract; packagers | `38cd5ea00b4a1ca4dfc887d996bdd91ea0e3eb96` |
| [rust/cix-native/packaging/sdk/README.md](../rust/cix-native/packaging/sdk/README.md) | Install/distribution contract; packagers | `ab158686cfb7da48491d1ba779e57c1858271cca` |
| [rust/cix-native/packaging/sdk/RUST.md](../rust/cix-native/packaging/sdk/RUST.md) | Install/distribution contract; packagers | `04e72ade8748ac260011b6446c85b3a664d32b9f` |
| [rust/cix-native/packaging/sdk/WINDOWS-LAYOUT-TEST.md](../rust/cix-native/packaging/sdk/WINDOWS-LAYOUT-TEST.md) | Install/distribution contract; packagers | `dd323f0aa1246a9d0cdaf3f05785d8d2a72382d0` |
| [rust/cix-native/packaging/squash/README.md](../rust/cix-native/packaging/squash/README.md) | Install/distribution contract; packagers | `b788e429781598e95ed62e0835868544e023ee15` |
| [rust/cix-native/packaging/windows/README.md](../rust/cix-native/packaging/windows/README.md) | Install/distribution contract; packagers | `bbe5d0ef2fcb5102aeba4614c645deb6f64f088c` |
| [rust/cix-native/tests/fixtures/bounded_values/README.md](../rust/cix-native/tests/fixtures/bounded_values/README.md) | Fixture provenance / compatibility; test reviewers | `1fa3e495570545f7ace03c92bb7f9437e924c76d` |
| [rust/cix-native/tests/fixtures/conditional_values/README.md](../rust/cix-native/tests/fixtures/conditional_values/README.md) | Fixture provenance / compatibility; test reviewers | `e2d9b531a2c14905941f723487c054a0f07ae29d` |
| [rust/cix-native/tests/fixtures/grid_counts/README.md](../rust/cix-native/tests/fixtures/grid_counts/README.md) | Fixture provenance / compatibility; test reviewers | `0714bf06db5f3febdef0f7fa4f2e5c32f64bfd0e` |
| [rust/cix-native/tests/fixtures/grid_feedback/README.md](../rust/cix-native/tests/fixtures/grid_feedback/README.md) | Fixture provenance / compatibility; test reviewers | `754f9938c7f776cbb0a4b8c36d7cd31f19c3b974` |
| [rust/cix-native/tests/fixtures/grid_interval/README.md](../rust/cix-native/tests/fixtures/grid_interval/README.md) | Fixture provenance / compatibility; test reviewers | `89acf5f98320dff20718f20289e70c38c0a3e0aa` |
| [rust/cix-native/tests/fixtures/native_ports/README.md](../rust/cix-native/tests/fixtures/native_ports/README.md) | Fixture provenance / compatibility; test reviewers | `bae83a24efadd2031b4bc6e611dc3729a9673562` |
| [rust/cix-native/tests/fixtures/record_stride/README.md](../rust/cix-native/tests/fixtures/record_stride/README.md) | Fixture provenance / compatibility; test reviewers | `6e7790201d51637dfa5cd99beeee37a3eca18f79` |
| [rust/cix-native/tests/fixtures/regions/README.md](../rust/cix-native/tests/fixtures/regions/README.md) | Fixture provenance / compatibility; test reviewers | `c9c8110e6331a7b42c32e4692d6dfac5c809e43b` |
| [rust/cix-native/tests/fixtures/spatial_frames/README.md](../rust/cix-native/tests/fixtures/spatial_frames/README.md) | Fixture provenance / compatibility; test reviewers | `84baaf029d5a2fb3afdaddac0ddc6a149656831a` |
| [rust/cix-native/vendor/libbsc/UPSTREAM.md](../rust/cix-native/vendor/libbsc/UPSTREAM.md) | Source terms and attribution; maintainers / distributors | `74f79dfcad180e20fceb83fac8c0e371d25e0e69` |
| [rust/cix-native/vendor/libzpaq715/UPSTREAM.md](../rust/cix-native/vendor/libzpaq715/UPSTREAM.md) | Source terms and attribution; maintainers / distributors | `45de48244fe7e703f44ed232f7f1f26cf3ac6884` |

## Manpages and source-term records

The three manpages are the versioned CLI references; licence/notice text is
retained under its own terms, not reformatted into a new legal conclusion.
CMakeLists.txt files are executable build definitions and are intentionally not
listed as prose contracts.

| Record | Kind | Inspected Git blob |
| --- | --- | --- |
| [rust/cix-native/man/cix.1](../rust/cix-native/man/cix.1) | CLI reference | `ac39d651a112c4eb6cbdf21b772d53976dfbf1f2` |
| [rust/cix-native/man/cixcat.1](../rust/cix-native/man/cixcat.1) | CLI reference | `77e6cd6c267d22ee050d531f35ab8316afea53f4` |
| [rust/cix-native/man/uncix.1](../rust/cix-native/man/uncix.1) | CLI reference | `3e60ed882275bd69b00d6475529ae40d45279770` |
| [LICENSE](../LICENSE) | Source terms / notice | `30b937b2714023e83deebc026e69f755c9ab9f79` |
| [THIRD_PARTY_NOTICES.txt](../THIRD_PARTY_NOTICES.txt) | Source terms / notice | `3826e2d988f498c1e3bb089d08691ce9413f963b` |
| [rust/cix-native/LICENSE](../rust/cix-native/LICENSE) | Source terms / notice | `30b937b2714023e83deebc026e69f755c9ab9f79` |
| [rust/cix-native/THIRD_PARTY_NOTICES.txt](../rust/cix-native/THIRD_PARTY_NOTICES.txt) | Source terms / notice | `3826e2d988f498c1e3bb089d08691ce9413f963b` |
| [rust/cix-native/bindings/python/LICENSE](../rust/cix-native/bindings/python/LICENSE) | Source terms / notice | `30b937b2714023e83deebc026e69f755c9ab9f79` |
| [rust/cix-native/packaging/notices/licenses/Brotli.txt](../rust/cix-native/packaging/notices/licenses/Brotli.txt) | Source terms / notice | `33b7cdd2dbaeddce1e35aa1a13f63d71154dc42f` |
| [rust/cix-native/packaging/notices/licenses/CharLS-Debian-copyright.txt](../rust/cix-native/packaging/notices/licenses/CharLS-Debian-copyright.txt) | Source terms / notice | `c66f3e07ddbb5ff0508a2a95f7d11138fc9c30eb` |
| [rust/cix-native/packaging/notices/licenses/GPL-2.0.txt](../rust/cix-native/packaging/notices/licenses/GPL-2.0.txt) | Source terms / notice | `d159169d1050894d3ea3b98e1c965c4058208fe1` |
| [rust/cix-native/packaging/notices/licenses/GPL-3.0.txt](../rust/cix-native/packaging/notices/licenses/GPL-3.0.txt) | Source terms / notice | `f288702d2fa16d3cdf0035b15a9fcbc552cd88e7` |
| [rust/cix-native/packaging/notices/licenses/Highway.txt](../rust/cix-native/packaging/notices/licenses/Highway.txt) | Source terms / notice | `f49a4e16e68b128803cc2dcea614603632b04eac` |
| [rust/cix-native/packaging/notices/licenses/JPEG-XL.txt](../rust/cix-native/packaging/notices/licenses/JPEG-XL.txt) | Source terms / notice | `c66034b105f23d108a51b45c846d5b5c194cc229` |
| [rust/cix-native/packaging/notices/licenses/OpenJPEG-Debian-copyright.txt](../rust/cix-native/packaging/notices/licenses/OpenJPEG-Debian-copyright.txt) | Source terms / notice | `e309d8a256d48f145c7a398c877f88dfff1ebd9c` |
| [rust/cix-native/packaging/notices/licenses/skcms.txt](../rust/cix-native/packaging/notices/licenses/skcms.txt) | Source terms / notice | `6c7c5be360194ef93331dfb2b0c01114ad9c263f` |
| [rust/cix-native/vendor/libbsc/LICENSE](../rust/cix-native/vendor/libbsc/LICENSE) | Source terms / notice | `d645695673349e3947e8e5ae42332d0ac3164cd7` |
| [rust/cix-native/vendor/libbsc/libbsc/bwt/libcubwt/LICENSE](../rust/cix-native/vendor/libbsc/libbsc/bwt/libcubwt/LICENSE) | Source terms / notice | `261eeb9e9f8b2b4b0d119366dda99c6fd7d35c64` |
| [rust/cix-native/vendor/libbsc/libbsc/bwt/libsais/LICENSE](../rust/cix-native/vendor/libbsc/libbsc/bwt/libsais/LICENSE) | Source terms / notice | `d645695673349e3947e8e5ae42332d0ac3164cd7` |

## Verification limits and maintenance

Inventory identities come from committed Git enumeration and scoped original-file
inspection. Markdown/links and blob/path checks establish navigation and source
correspondence, not executable command safety, successful build, installed-host
compatibility, rendered usability, accessibility or a new test run. A disposable
revision-bound documentation index can help retrieval, but also includes .txt
build definitions and excludes manpage/source types; use the original files.

Refresh only changed rows from the accepted Git diff. Keep historical source
notes pinned and distinguish proposed, implemented, independently reviewed,
executed and deployed facts. Genuine gaps stay with their current product owner
and requirements; they do not become a duplicate release checklist, a common
cross-project runtime or permission to activate a service.

## Cross-project interface comparison

The inspected shared maintenance interface is UIF policy 2.0.1 at commit
`ebaeafef0f08c5335f5a208c718948544f6d178d` in `maldous/uif`, policy digest
`7b54579a72c16c504bf4a82d875a0ca770d851787ba62035e446ec673e6db54f`.
Its [project integration contract](https://github.com/maldous/uif/blob/ebaeafef0f08c5335f5a208c718948544f6d178d/docs/project-integration.md)
keeps product source, toolchain, privacy and acceptance with each project and
execution policy with the selected maintenance environment. The pinned CIX
snapshot still names policy2.0.0 in AGENTS; that active reference update is a
separate source unit, not a claim that2.0.1 is already installed or acknowledged
on every executor. A source policy review does not deploy an ownership adapter.

For a peer comparison, [ANQR's agent instructions](https://github.com/maldous/anqr-v3/blob/745d522cd79ba097611cbc6950071fc729e221bd/AGENTS.md)
and [capability guide](https://github.com/maldous/anqr-v3/blob/745d522cd79ba097611cbc6950071fc729e221bd/docs/agent-capabilities.md)
use the same distinction between readable, discovered, executed, authenticated
and authorized tools and optional maintenance policy. ANQR's selected product is
an in-process TypeScript/Node QR artifact package; CIX is a native Rust/C++
CLI/SDK with separate language and host adapters. A shared discovery vocabulary
neither makes those APIs interchangeable nor requires a common toolchain,
frontend, runtime or peer release dependency. Each project's original source,
rights and qualification limits remain its own.
