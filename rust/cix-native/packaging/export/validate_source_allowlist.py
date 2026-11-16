#!/usr/bin/env python3
"""Validate a proposed native CIX source-export allowlist without exporting files."""
from __future__ import annotations

import argparse
import hashlib
import json
from fnmatch import fnmatchcase
from pathlib import Path
from typing import Any


FULL_ENGINE_PROFILE = "full-engine-rc"
OPTIONAL_PROFILE = "native-core-optional"
ENGINEERING_PROFILE = "engineering-validation"
SOURCE_MEMBERS_PROFILE = "source-members"


def read_manifest(path: Path) -> dict[str, Any]:
    document = json.loads(path.read_text(encoding="utf-8"))
    if document.get("schema") != "cix-native-source-export-allowlist-v2":
        raise ValueError("unsupported allowlist schema")
    return document


def matches(path: str, pattern: str) -> bool:
    if pattern.endswith("/**"):
        return has_prefix(path, pattern[:-3])
    return fnmatchcase(path, pattern)


def has_prefix(path: str, prefix: str) -> bool:
    return path == prefix or path.startswith(f"{prefix}/")


def allowed_below_exclusion(path: str, exclusion: str, allowed: list[str]) -> bool:
    """Permit only an allowlist entry that narrows an excluded directory."""
    for pattern in allowed:
        if not matches(path, pattern):
            continue
        narrowed = pattern[:-3] if pattern.endswith("/**") else pattern
        if has_prefix(narrowed, exclusion) and narrowed != exclusion:
            return True
    return False


def source_members(source_root: Path, manifest: dict[str, Any]) -> list[str]:
    allowed = manifest["allowed_globs"]
    excluded_prefixes = manifest["excluded_path_prefixes"]
    excluded_suffixes = tuple(manifest["excluded_suffixes"])
    unexpected: list[str] = []
    selected: list[str] = []
    for path in sorted(source_root.rglob("*")):
        if not path.is_file():
            continue
        relative = path.relative_to(source_root).as_posix()
        excluded = [prefix for prefix in excluded_prefixes if has_prefix(relative, prefix)]
        if excluded and not all(
            allowed_below_exclusion(relative, prefix, allowed) for prefix in excluded
        ):
            continue
        explicitly_allowed = any(matches(relative, pattern) for pattern in allowed)
        if relative.endswith(excluded_suffixes) and not explicitly_allowed:
            continue
        if any(matches(relative, pattern) for pattern in allowed):
            selected.append(relative)
        else:
            unexpected.append(relative)
    if unexpected:
        raise ValueError("unallowlisted source files: " + ", ".join(unexpected))
    return selected


def validate_dependency_closure(source_root: Path, manifest: dict[str, Any], profile: str) -> str:
    """Check the declared decoder graph, never qualify a binary or authenticate a receipt.

    Full-engine schema: cix-decoder-dependency-closure-v1. The selected profile
    must supply decoder_selection independently of the closure. Both records
    carry the same exact selection (source revision/tree/features/target/applied
    overlays/source-file pins, root, provider bindings, component and route IDs).
    Missing final selection is deliberately blocking; this routine does not
    discover dependencies, inspect binaries, fetch references or run restores.

    References are {locator, bytes, sha256}, not fetched or authenticated here.
    Separate acceptance stages bind a canonical JSON subject digest; even that
    correspondence cannot prove the statements in an arbitrary receipt true.
    A successful result means structural consistency ONLY. The actual artifact,
    source census, complete dependency inspection and offline restore still need
    independently authenticated receipts and separately admitted qualification.
    See the hermetic contract tests for a fully synthetic serialized specimen.
    """
    if profile == SOURCE_MEMBERS_PROFILE:
        return "not_checked_source_members_only"
    profiles = manifest["export_profiles"]
    if profile == OPTIONAL_PROFILE:
        return profiles[OPTIONAL_PROFILE]["disposition"]
    if profile == ENGINEERING_PROFILE:
        return profiles[ENGINEERING_PROFILE]["disposition"]
    if profile != FULL_ENGINE_PROFILE:
        raise ValueError(f"unknown export profile: {profile}")

    def require(condition: bool, field: str) -> None:
        if not condition:
            raise ValueError(f"full-engine RC source export blocked: {field}")

    def obj(value: Any, keys: str, field: str) -> dict[str, Any]:
        require(type(value) is dict and set(value) == set(keys.split()), field + " fields")
        return value

    def text(value: Any, field: str) -> str:
        require(type(value) is str and bool(value.strip()) and value == value.strip(), field)
        require(not any(ord(c) < 32 for c in value), field)
        return value

    def digest(value: Any, field: str, sizes: tuple[int, ...] = (64,)) -> str:
        text(value, field)
        require(len(value) in sizes and all(c in "0123456789abcdef" for c in value), field)
        return value

    def strings(value: Any, field: str, nonempty: bool = False) -> list[str]:
        require(type(value) is list and (not nonempty or bool(value)), field)
        for item in value:
            text(item, field)
        require(len(value) == len(set(value)), field + " duplicates")
        return value

    def ref(value: Any, field: str) -> None:
        obj(value, "locator bytes sha256", field)
        text(value["locator"], field + ".locator")
        require(type(value["bytes"]) is int and value["bytes"] > 0, field + ".bytes")
        digest(value["sha256"], field + ".sha256")

    def relpath(value: Any, field: str) -> str:
        text(value, field)
        require(not value.startswith("/") and "\\" not in value and ":" not in value
                and all(p not in ("", ".", "..") for p in value.split("/")), field)
        return value

    def local_file(value: Any, field: str) -> Path:
        relative = relpath(value, field)
        root = source_root.resolve()
        path = root
        for part in relative.split("/"):
            path = path / part
            require(not path.is_symlink(), field + " symlink")
        require(path.is_file() and path.resolve().is_relative_to(root), field + " missing/outside root")
        return path

    def read_json(path: Path) -> Any:
        def unique(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
            result: dict[str, Any] = {}
            for key, value in pairs:
                require(key not in result, "duplicate JSON field " + key)
                result[key] = value
            return result
        return json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=unique)

    def pin(value: Any, field: str) -> None:
        obj(value, "path bytes sha256 mode", field)
        relpath(value["path"], field + ".path")
        require(type(value["bytes"]) is int and value["bytes"] > 0, field + ".bytes")
        digest(value["sha256"], field + ".sha256")
        mode = value["mode"]
        require(type(mode) is str and len(mode) == 4 and mode[0] == "0"
                and all(c in "01234567" for c in mode[1:]), field + ".mode")

    def unresolved(value: Any, field: str) -> None:
        require(type(value) is list, field + " must be an explicit list")
        for gap in value:
            obj(gap, "field status resolver action", field)
            for key in ("field", "resolver", "action"):
                text(gap[key], field + "." + key)
            require(gap["status"] in ("unsupplied", "not_retained", "unrun", "held"), field + ".status")
        require(not value, field + " unresolved; retain resolver/action before qualification")

    closure_path = local_file(profiles[FULL_ENGINE_PROFILE]["required_dependency_closure"], "closure path")
    closure = read_json(closure_path)
    obj(closure, "schema status selection nodes edges assets unresolved producer acceptance", "closure")
    require(closure["schema"] == "cix-decoder-dependency-closure-v1", "closure.schema")
    require(closure["status"] == "accepted_full_engine_rc", "closure.status")
    unresolved(closure["unresolved"], "closure.unresolved")
    text(closure["producer"], "closure.producer")

    selection = obj(closure["selection"],
                    "identity root providers components routes census_ref", "selection")
    expected = profiles[FULL_ENGINE_PROFILE].get("decoder_selection")
    require(type(expected) is dict and selection == expected, "exact independently selected decoder_selection required")
    ref(selection["census_ref"], "selection.census_ref")
    identity = obj(selection["identity"],
                   "revision tree_sha256 features target overlays source_files", "identity")
    digest(identity["revision"], "identity.revision", (40, 64))
    digest(identity["tree_sha256"], "identity.tree_sha256")
    strings(identity["features"], "identity.features")
    text(identity["target"], "identity.target")
    require(type(identity["overlays"]) is list, "identity.overlays")
    overlay_ids: set[str] = set()
    for overlay in identity["overlays"]:
        obj(overlay, "id patch_sha256 application_ref", "overlay")
        name = text(overlay["id"], "overlay.id")
        require(name not in overlay_ids, "duplicate overlay")
        overlay_ids.add(name)
        digest(overlay["patch_sha256"], "overlay.patch_sha256")
        ref(overlay["application_ref"], "overlay.application_ref")
    require(type(identity["source_files"]) is list and bool(identity["source_files"]), "identity.source_files")
    source_paths: set[str] = set()
    for member in identity["source_files"]:
        pin(member, "source member")
        require(member["path"] not in source_paths, "duplicate source member")
        source_paths.add(member["path"])
        path = local_file(member["path"], "source member")
        data = path.read_bytes()
        require(len(data) == member["bytes"] and hashlib.sha256(data).hexdigest() == member["sha256"]
                and format(path.stat().st_mode & 0o7777, "04o") == member["mode"], "source member pin mismatch")

    def source_boundary(value: Any, field: str) -> None:
        obj(value, "path symbol reference", field)
        text(value["path"], field + ".path")
        require(value["path"] in source_paths, field + " unselected source")
        text(value["symbol"], field + ".symbol")
        ref(value["reference"], field + ".reference")

    required_providers = {"paq_v215", "paq_v216", "paq_joint_discount", "paq_store_state", "jxl", "spatial"}
    providers = selection["providers"]
    require(type(providers) is dict and set(providers) == required_providers, "four PAQ/JXL/spatial bindings required")
    for member in providers.values():
        text(member, "provider member")
    require(len(set(providers.values())) == 6, "distinct provider component identities required")
    component_ids = set(strings(selection["components"], "selection.components", True))
    root_id = text(selection["root"], "selection.root")
    require(root_id in component_ids and set(providers.values()) <= component_ids, "root/provider selection")

    routes = selection["routes"]
    require(type(routes) is list and bool(routes), "selection.routes")
    route_ids: set[str] = set()
    route_roles: set[str] = set()
    for route in routes:
        obj(route, "id role decoder source parameters_ref", "route")
        rid = text(route["id"], "route.id")
        require(rid not in route_ids, "duplicate route")
        route_ids.add(rid)
        role = text(route["role"], "route.role")
        require(role in required_providers | {"native"}, "route.role")
        route_roles.add(role)
        text(route["decoder"], "route.decoder")
        require(route["decoder"] in component_ids, "route.decoder")
        if role in providers:
            require(route["decoder"] == providers[role], "route/provider correspondence")
        source_boundary(route["source"], "route.source")
        ref(route["parameters_ref"], "route.parameters_ref")
    require(required_providers <= route_roles, "mandatory full-engine routes missing")

    require(type(closure["nodes"]) is list and bool(closure["nodes"]), "nodes")
    nodes: dict[str, Any] = {}
    carried_paths: set[str] = set()
    for node in closure["nodes"]:
        require(type(node) is dict, "node")
        disposition = text(node.get("disposition"), "node.disposition")
        extra = {"packaged": "member", "documented-system": "system", "static": "static"}.get(disposition)
        require(extra is not None, "node disposition must be packaged/documented-system/static")
        obj(node, "id component kind version features target disposition original_source current_source "
            "build_ref terms_ref custody_ref inspection dependencies referrers " + extra, "node")
        nid = text(node["id"], "node.id")
        require(nid not in nodes, "duplicate node")
        nodes[nid] = node
        text(node["component"], "node.component")
        require(node["kind"] in ("executable", "decoder", "runtime"), "node.kind")
        text(node["version"], "node.version")
        strings(node["features"], "node.features")
        require(node["target"] == identity["target"], "node target mismatch")
        for key in ("original_source", "current_source", "build_ref", "terms_ref", "custody_ref"):
            ref(node[key], "node." + key)
        inspection = obj(node["inspection"], "reference complete", "inspection")
        ref(inspection["reference"], "inspection.reference")
        require(inspection["complete"] is True, "complete recursive dependency inspection required")
        strings(node["dependencies"], "node.dependencies")
        strings(node["referrers"], "node.referrers")
        if disposition == "packaged":
            pin(node["member"], "packaged member")
            require(node["member"]["path"] not in carried_paths, "duplicate packaged path")
            carried_paths.add(node["member"]["path"])
        elif disposition == "documented-system":
            system = obj(node["system"], "installed_path soname package loader_ref compatibility_ref", "system")
            for key in ("installed_path", "soname", "package"):
                text(system[key], "system." + key)
            require(system["installed_path"].startswith("/") and ".." not in system["installed_path"].split("/"), "system absolute installed path")
            ref(system["loader_ref"], "system.loader_ref")
            ref(system["compatibility_ref"], "system.compatibility_ref")
        else:
            static = obj(node["static"], "container link_ref", "static")
            text(static["container"], "static.container")
            ref(static["link_ref"], "static.link_ref")
    require(set(nodes) == component_ids, "selected component census mismatch")
    require(nodes[root_id]["kind"] == "executable" and nodes[root_id]["disposition"] == "packaged", "root executable must be packaged")
    require(int(nodes[root_id]["member"]["mode"], 8) & 0o111 != 0, "root executable mode")
    for nid in providers.values():
        require(nodes[nid]["kind"] == "decoder", "provider decoder kind")
    for role, nid in providers.items():
        require(nodes[nid]["component"] == role, "provider component correspondence")
    for route in routes:
        require(nodes[route["decoder"]]["kind"] in ("decoder", "executable"), "route decoder kind")

    require(type(closure["edges"]) is list, "edges")
    outgoing: dict[str, set[str]] = {nid: set() for nid in nodes}
    incoming: dict[str, set[str]] = {nid: set() for nid in nodes}
    for edge in closure["edges"]:
        obj(edge, "from to resolution loader_ref link_ref", "edge")
        src, dst = edge["from"], edge["to"]
        text(src, "edge.from")
        text(dst, "edge.to")
        require(src in nodes and dst in nodes and src != dst, "undeclared dependency edge")
        require(dst not in outgoing[src], "duplicate dependency edge")
        outgoing[src].add(dst)
        incoming[dst].add(src)
        required_resolution = {"packaged": "packaged-relative", "documented-system": "documented-system", "static": "static-link"}[nodes[dst]["disposition"]]
        require(edge["resolution"] == required_resolution, "ambient or mismatched dependency resolution")
        ref(edge["loader_ref"], "edge.loader_ref")
        ref(edge["link_ref"], "edge.link_ref")
    for nid, node in nodes.items():
        require(outgoing[nid] == set(node["dependencies"]), "dependency inventory mismatch: " + nid)
        require(incoming[nid] == set(node["referrers"]), "referrer inventory mismatch: " + nid)
        if node["disposition"] == "static":
            container = node["static"]["container"]
            require(container in nodes and nodes[container]["disposition"] == "packaged"
                    and container != nid, "static containing artifact")
            # A dependency of a static component is resolved in the same image.
            for parent in incoming[nid]:
                containing = nodes[parent].get("static", {}).get("container", parent)
                require(containing == container, "static link containing-artifact mismatch")
    reached: set[str] = set()
    pending = [root_id]
    while pending:
        nid = pending.pop()
        if nid not in reached:
            reached.add(nid)
            pending.extend(outgoing[nid])
    require(reached == component_ids, "unreachable selected component")
    # Required source-declared edge: never infer an additional liblzma DSO.
    require(any(nodes[nid]["component"] == "liblzma" for nid in outgoing[providers["jxl"]]),
            "mandatory JXL liblzma dependency missing")
    def descendants(start: str) -> set[str]:
        found: set[str] = set()
        todo = [start]
        while todo:
            item = todo.pop()
            if item not in found:
                found.add(item)
                todo.extend(outgoing[item])
        return found
    for role, required in (
        ("jxl", {"libjxl", "libjxl_threads", "libjxl_cms", "brotlidec", "brotlienc", "brotlicommon", "liblzma"}),
        ("spatial", {"charls", "openjpeg"}),
    ):
        actual = {nodes[nid]["component"] for nid in descendants(providers[role])}
        require(required <= actual, role + " companion correspondence incomplete")

    assets = closure["assets"]
    require(type(assets) is list, "assets")
    asset_routes: set[str] = set()
    distributed: dict[tuple[str, str], Any] = {
        ("installed-runtime", n["member"]["path"]): n["member"]
        for n in nodes.values() if n["disposition"] == "packaged"
    }
    route_decoders = {r["id"]: r["decoder"] for r in routes}
    for record in assets:
        obj(record, "route source review_ref items", "route assets")
        rid = text(record["route"], "asset.route")
        require(rid in route_ids and rid not in asset_routes, "asset route coverage")
        asset_routes.add(rid)
        source_boundary(record["source"], "asset.source")
        ref(record["review_ref"], "asset.review_ref")
        require(type(record["items"]) is list and bool(record["items"]), "explicit asset/model disposition required")
        asset_ids: set[str] = set()
        for asset in record["items"]:
            require(type(asset) is dict, "asset")
            disposition = text(asset.get("disposition"), "asset.disposition")
            extras = {"none": "reason", "archive-contained": "format_ref fields",
                      "compiled": "container source_ref build_ref",
                      "dictionary": "member distribution identity format_ref"}
            require(disposition in extras, "forbidden/evidence-only/undeclared asset dependency")
            obj(asset, "id disposition " + extras[disposition], "asset")
            aid = text(asset["id"], "asset.id")
            require(aid not in asset_ids, "duplicate asset")
            asset_ids.add(aid)
            if disposition == "none":
                require(len(record["items"]) == 1, "no-assets contradicts other assets")
                text(asset["reason"], "asset.reason")
            elif disposition == "archive-contained":
                ref(asset["format_ref"], "asset.format_ref")
                strings(asset["fields"], "archive asset fields", True)
            elif disposition == "compiled":
                container = text(asset["container"], "asset.container")
                reachable = descendants(route_decoders[rid])
                # Dependency edges point into static components; their physical
                # image may be an ancestor, outside this route's descendants.
                # Static containment was validated above. Do not traverse from
                # that image into unrelated sibling decoders or other assets.
                containers = {nid for nid in reachable if nodes[nid]["disposition"] == "packaged"}
                containers.update(nodes[nid]["static"]["container"] for nid in reachable
                                  if nodes[nid]["disposition"] == "static")
                require(container in containers, "compiled asset containing artifact")
                ref(asset["source_ref"], "asset.source_ref")
                ref(asset["build_ref"], "asset.build_ref")
            else:
                pin(asset["member"], "dictionary member")
                distribution = obj(asset["distribution"], "kind location reference", "dictionary distribution")
                require(distribution["kind"] in ("archive", "installed-runtime"), "dictionary distribution kind")
                require(distribution["location"] == asset["member"]["path"], "dictionary distribution location mismatch")
                location = (distribution["kind"], distribution["location"])
                require(location not in distributed or distributed[location] == asset["member"], "conflicting dictionary distribution identity")
                distributed[location] = asset["member"]
                ref(distribution["reference"], "dictionary distribution.reference")
                ident = obj(asset["identity"], "bytes sha256 frame_id carrier", "dictionary identity")
                require(type(ident["bytes"]) is int and ident["bytes"] == asset["member"]["bytes"], "dictionary bytes mismatch")
                digest(ident["sha256"], "dictionary identity.sha256")
                require(ident["sha256"] == asset["member"]["sha256"], "dictionary SHA mismatch")
                require(type(ident["frame_id"]) is int and 0 <= ident["frame_id"] <= 0xffffffff, "dictionary frame_id")
                carrier = obj(ident["carrier"], "kind location sha256 reference", "dictionary identity carrier")
                require(carrier["kind"] in ("archive-field", "installed-manifest", "explicit-api-argument"), "dictionary identity carrier kind")
                text(carrier["location"], "dictionary identity carrier location")
                require(carrier["sha256"] == ident["sha256"], "dictionary identity carrier SHA mismatch")
                ref(carrier["reference"], "dictionary identity carrier.reference")
                ref(asset["format_ref"], "dictionary format_ref")
    require(asset_routes == route_ids, "missing route asset/model disposition")

    notices = local_file("packaging/notices/provider-notices.json", "provider notice manifest")
    notice_document = read_json(notices)
    require(type(notice_document) is dict and "unresolved" in notice_document, "explicit notice unresolved list required")
    unresolved(notice_document["unresolved"], "notice.unresolved")

    # JSON serialization is specified so receipts bind content, not mutable URLs.
    # This is hash correspondence, NOT proof of a signer's identity or acceptance.
    def subject(value: Any) -> str:
        return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":"),
                                         ensure_ascii=True, allow_nan=False).encode("utf-8")).hexdigest()

    artifact_subject = subject({"selection": selection, "nodes": closure["nodes"],
                                "edges": closure["edges"], "assets": assets,
                                "notice_sha256": hashlib.sha256(notices.read_bytes()).hexdigest()})
    acceptance = obj(closure["acceptance"], "source artifact offline_restore", "acceptance")
    stage_refs: set[tuple[str, str]] = set()
    for stage in ("source", "artifact", "offline_restore"):
        keys = "stage status reviewer subject_sha256 receipt"
        if stage == "offline_restore":
            keys += " routes fresh_process offline archive_only no_fetch archives"
        record = obj(acceptance[stage], keys, "acceptance." + stage)
        require(record["stage"] == stage and record["status"] == "accepted", "acceptance stage/status")
        reviewer = text(record["reviewer"], "acceptance.reviewer")
        require(reviewer != closure["producer"], "independent acceptance required")
        require(record["subject_sha256"] == (subject(selection) if stage == "source" else artifact_subject), "acceptance subject mismatch")
        ref(record["receipt"], "acceptance.receipt")
        receipt_key = (record["receipt"]["locator"], record["receipt"]["sha256"])
        require(receipt_key not in stage_refs, "separate acceptance stage receipts required")
        stage_refs.add(receipt_key)
    restore = acceptance["offline_restore"]
    require(set(strings(restore["routes"], "offline restore routes", True)) == route_ids, "offline restore route coverage")
    for key in ("fresh_process", "offline", "archive_only", "no_fetch"):
        require(restore[key] is True, "offline restore " + key)
    require(type(restore["archives"]) is list and bool(restore["archives"]), "offline restore archive receipts required")
    restored: set[str] = set()
    for result in restore["archives"]:
        obj(result, "route archive_sha256 original_sha256 restored_sha256 receipt", "restore result")
        rid = text(result["route"], "restore result.route")
        require(rid in route_ids, "restore result unknown route")
        restored.add(rid)
        for key in ("archive_sha256", "original_sha256", "restored_sha256"):
            digest(result[key], "restore result." + key)
        require(result["original_sha256"] == result["restored_sha256"], "offline restore hash mismatch")
        ref(result["receipt"], "restore result.receipt")
    require(restored == route_ids, "offline restore per-route receipts missing")
    return "structurally_validated_decoder_closure_not_artifact_qualification"


def validate(source_root: Path, manifest: dict[str, Any], profile: str = SOURCE_MEMBERS_PROFILE) -> dict[str, Any]:
    source_root = source_root.resolve()
    if not source_root.is_dir():
        raise ValueError(f"source root is not a directory: {source_root}")
    expected_root = manifest["source_root"]
    exported_root = manifest["export_prefix"].format(
        version=manifest.get("version", "0.1.0")
    )
    if not (source_root.as_posix().endswith(expected_root) or
            source_root.name == exported_root):
        raise ValueError(
            f"source root must be {expected_root} in the maintained tree or "
            f"{exported_root} after export: {source_root}"
        )

    required = manifest["required_exact_files"]
    missing_required = [item for item in required if not (source_root / item).is_file()]
    if missing_required:
        raise ValueError(f"required source files missing: {', '.join(missing_required)}")

    closure_status = validate_dependency_closure(source_root, manifest, profile)
    members = source_members(source_root, manifest)
    return {
        "schema": "cix-native-source-export-allowlist-report-v2",
        "status": "validated_source_members_only_not_exported",
        "profile": profile,
        "dependency_closure_status": closure_status,
        "source_root": str(source_root),
        "selected_member_count": len(members),
        "selected_members": members,
        "missing_required": [],
        "unexpected_members": []
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-root", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--profile", default=SOURCE_MEMBERS_PROFILE, choices=(SOURCE_MEMBERS_PROFILE, OPTIONAL_PROFILE, ENGINEERING_PROFILE, FULL_ENGINE_PROFILE))
    parser.add_argument("--report", type=Path, help="optional report destination; no archive is created")
    args = parser.parse_args()
    report = validate(args.source_root, read_manifest(args.manifest), args.profile)
    rendered = json.dumps(report, indent=2) + "\n"
    if args.report:
        args.report.write_text(rendered, encoding="utf-8")
    else:
        print(rendered, end="")


if __name__ == "__main__":
    main()
