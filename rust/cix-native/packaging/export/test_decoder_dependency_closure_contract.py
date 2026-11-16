#!/usr/bin/env python3
"""CIX-422 contract-test SOURCE, AUTHORED_UNRUN in the source-only phase.

Every graph, source byte, artifact pin and receipt below is SYNTHETIC TEST DATA.
No provider is loaded, artifact built, archive decoded or legal term supplied.
The positive specimen tests structural correspondence, never shipping readiness.
Case names reuse all eleven independently accepted CIX-19 expectations.
Execution/import requires a later separately admitted test phase.
"""
from __future__ import annotations

import copy
import hashlib
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path


HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("decoder_closure_subject", HERE / "validate_source_allowlist.py")
assert SPEC is not None and SPEC.loader is not None
SUBJECT = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SUBJECT)

STRUCTURAL_ONLY = "structurally_validated_decoder_closure_not_artifact_qualification"
SOURCE_BYTES = b"SYNTHETIC route census; this is not CIX product source.\n"
NOTICE_BYTES = b'{"unresolved": []}\n'


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def reference(name: str) -> dict:
    payload = ("SYNTHETIC TEST RECEIPT: " + name).encode()
    return {"locator": "synthetic-test://" + name, "bytes": len(payload), "sha256": sha(payload)}


def member(path: str, payload: bytes = b"SYNTHETIC inert member", mode: str = "0644") -> dict:
    return {"path": path, "bytes": len(payload), "sha256": sha(payload), "mode": mode}


def boundary(symbol: str) -> dict:
    return {"path": "src/synthetic-routes.txt", "symbol": symbol, "reference": reference("source/" + symbol)}


def canonical_digest(value: object) -> str:
    # The contract defines this serialization independently of the implementation.
    return sha(json.dumps(value, sort_keys=True, separators=(",", ":"),
                          ensure_ascii=True, allow_nan=False).encode())


def bind_synthetic_acceptance(closure: dict) -> None:
    """Bind fictional references for tests; this does not create genuine receipts."""
    selection = closure["selection"]
    artifact = canonical_digest({"selection": selection, "nodes": closure["nodes"],
                                 "edges": closure["edges"], "assets": closure["assets"],
                                 "notice_sha256": sha(NOTICE_BYTES)})
    acceptance = {}
    for stage in ("source", "artifact", "offline_restore"):
        acceptance[stage] = {"stage": stage, "status": "accepted", "reviewer": "SYNTHETIC distinct reviewer",
                             "subject_sha256": canonical_digest(selection) if stage == "source" else artifact,
                             "receipt": reference("acceptance/" + stage)}
    routes = [r["id"] for r in selection["routes"]]
    acceptance["offline_restore"].update({
        "routes": routes, "fresh_process": True, "offline": True, "archive_only": True, "no_fetch": True,
        "archives": [{"route": rid, "archive_sha256": sha((rid + " archive").encode()),
                      "original_sha256": sha((rid + " original").encode()),
                      "restored_sha256": sha((rid + " original").encode()),
                      "receipt": reference("restore/" + rid)} for rid in routes],
    })
    closure["acceptance"] = acceptance


def synthetic_full_engine() -> dict:
    """Complete serialized specimen of fields, not a real shipping dependency graph."""
    providers = {role: role for role in ("paq_v215", "paq_v216", "paq_joint_discount", "paq_store_state", "jxl", "spatial")}
    dependencies = {
        "cix": ["paq_v215", "paq_v216", "paq_joint_discount", "paq_store_state", "jxl", "spatial", "libc"],
        "paq_v215": [], "paq_v216": [], "paq_joint_discount": [], "paq_store_state": [],
        "jxl": ["libjxl", "libjxl_threads", "liblzma"],
        "libjxl": ["libjxl_cms", "brotlidec", "brotlienc"],
        "libjxl_threads": [], "libjxl_cms": [],
        "brotlidec": ["brotlicommon"], "brotlienc": ["brotlicommon"], "brotlicommon": [],
        "spatial": ["charls", "openjpeg"], "charls": [], "openjpeg": [], "liblzma": [], "libc": [],
    }
    identity = {"revision": "1" * 40, "tree_sha256": "2" * 64,
                "features": ["synthetic-full-engine"], "target": "synthetic-linux-x86_64",
                "overlays": [{"id": "synthetic-overlay", "patch_sha256": "3" * 64,
                              "application_ref": reference("applied-overlay")}],
                "source_files": [member("src/synthetic-routes.txt", SOURCE_BYTES)]}
    routes = [{"id": "route-" + role, "role": role, "decoder": nid,
               "source": boundary(role), "parameters_ref": reference("archive-parameters/" + role)}
              for role, nid in {"native": "cix", **providers}.items()]
    selection = {"identity": identity, "root": "cix", "providers": providers,
                 "components": list(dependencies), "routes": routes,
                 "census_ref": reference("complete-source-route-member-census")}
    nodes = []
    for nid, children in dependencies.items():
        disposition = "static" if nid == "liblzma" else "documented-system" if nid == "libc" else "packaged"
        node = {"id": nid, "component": nid, "kind": "executable" if nid == "cix" else "decoder" if nid in providers else "runtime",
                "version": "SYNTHETIC-1", "features": [], "target": identity["target"], "disposition": disposition,
                "original_source": reference(nid + "/original"), "current_source": reference(nid + "/current"),
                "build_ref": reference(nid + "/build"), "terms_ref": reference(nid + "/terms-not-legal-clearance"),
                "custody_ref": reference(nid + "/custody"),
                "inspection": {"reference": reference(nid + "/dependency-inspection"), "complete": True},
                "dependencies": children, "referrers": [parent for parent, deps in dependencies.items() if nid in deps]}
        if disposition == "static":
            node["static"] = {"container": "jxl", "link_ref": reference("liblzma/static-containing-jxl")}
        elif disposition == "documented-system":
            node["system"] = {"installed_path": "/synthetic-runtime/lib/libc.so", "soname": "libc.synthetic.so",
                              "package": "SYNTHETIC system runtime package", "loader_ref": reference("libc/loader"),
                              "compatibility_ref": reference("libc/target-compatibility")}
        else:
            node["member"] = member("bin/cix" if nid == "cix" else "lib/cix/" + nid + ".so",
                                    ("SYNTHETIC " + nid).encode(), "0755" if nid == "cix" else "0644")
        nodes.append(node)
    edges = [{"from": src, "to": dst,
              "resolution": "static-link" if dst == "liblzma" else "documented-system" if dst == "libc" else "packaged-relative",
              "loader_ref": reference(src + "/loader/" + dst), "link_ref": reference(src + "/link/" + dst)}
             for src, children in dependencies.items() for dst in children]
    assets = [{"route": r["id"], "source": copy.deepcopy(r["source"]), "review_ref": reference(r["id"] + "/asset-model-census"),
               "items": [{"id": "none", "disposition": "none", "reason": "SYNTHETIC reviewed route requires no external model or asset"}]}
              for r in routes]
    closure = {"schema": "cix-decoder-dependency-closure-v1", "status": "accepted_full_engine_rc",
               "selection": selection, "nodes": nodes, "edges": edges, "assets": assets,
               "unresolved": [], "producer": "SYNTHETIC producer", "acceptance": {}}
    bind_synthetic_acceptance(closure)
    return closure



def synthetic_static_native(nested: bool = False) -> dict:
    """A native decoder and its tables share cix; every value is synthetic."""
    closure = synthetic_full_engine()
    selected = closure["selection"]
    root = next(n for n in closure["nodes"] if n["id"] == "cix")
    root["dependencies"].append("native_static")
    static_ids = ["native_static", "native_tables"] if nested else ["native_static"]
    for nid in static_ids:
        parent = "cix" if nid == "native_static" else "native_static"
        children = ["native_tables"] if nested and nid == "native_static" else []
        closure["nodes"].append({
            "id": nid, "component": nid, "kind": "decoder" if nid == "native_static" else "runtime",
            "version": "SYNTHETIC-1", "features": [], "target": selected["identity"]["target"],
            "disposition": "static", "original_source": reference(nid + "/original"),
            "current_source": reference(nid + "/current"), "build_ref": reference(nid + "/build"),
            "terms_ref": reference(nid + "/terms-not-legal-clearance"),
            "custody_ref": reference(nid + "/custody"),
            "inspection": {"reference": reference(nid + "/dependency-inspection"), "complete": True},
            "dependencies": children, "referrers": [parent],
            "static": {"container": "cix", "link_ref": reference(nid + "/static-containing-cix")},
        })
        selected["components"].append(nid)
        closure["edges"].append({
            "from": parent, "to": nid, "resolution": "static-link",
            "loader_ref": reference(parent + "/loader/" + nid),
            "link_ref": reference(parent + "/link/" + nid),
        })
    route = next(r for r in selected["routes"] if r["role"] == "native")
    route["decoder"] = "native_static"
    assets = next(a for a in closure["assets"] if a["route"] == route["id"])
    assets["items"] = [{
        "id": "compiled-native-tables", "disposition": "compiled", "container": "cix",
        "source_ref": reference(static_ids[-1] + "/compiled-tables/source"),
        "build_ref": reference(static_ids[-1] + "/compiled-tables/build"),
    }]
    bind_synthetic_acceptance(closure)
    return closure

def dictionary() -> dict:
    pin = member("dictionaries/synthetic.zdict", b"SYNTHETIC full dictionary bytes")
    return {"id": "zstd-dictionary", "disposition": "dictionary", "member": pin,
            "distribution": {"kind": "installed-runtime", "location": pin["path"], "reference": reference("dictionary/distribution")},
            "identity": {"bytes": pin["bytes"], "sha256": pin["sha256"], "frame_id": 0,
                         "carrier": {"kind": "explicit-api-argument", "location": "cix_dictionary_decode declared SHA argument",
                                     "sha256": pin["sha256"], "reference": reference("dictionary/identity-carrier")}},
            "format_ref": reference("dictionary/raw-frame-plus-separate-identity-contract")}


class DecoderDependencyClosureContract(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / "rust/cix-native"
        (self.root / "src").mkdir(parents=True)
        (self.root / "src/synthetic-routes.txt").write_bytes(SOURCE_BYTES)
        (self.root / "src/synthetic-routes.txt").chmod(0o644)
        (self.root / "packaging/export").mkdir(parents=True)
        (self.root / "packaging/notices").mkdir()
        (self.root / "packaging/notices/provider-notices.json").write_bytes(NOTICE_BYTES)
        self.closure = synthetic_full_engine()
        self.manifest = {
            "source_root": "rust/cix-native", "export_prefix": "cix-{version}",
            "required_exact_files": ["src/synthetic-routes.txt"], "allowed_globs": ["src/**", "packaging/**"],
            "excluded_path_prefixes": [], "excluded_suffixes": [],
            "export_profiles": {"full-engine-rc": {"required_dependency_closure": "packaging/export/final-dependency-closure.json",
                                                   "decoder_selection": copy.deepcopy(self.closure["selection"])},
                                "native-core-optional": {"disposition": "optional_subset_not_full_engine_rc"},
                                "engineering-validation": {"disposition": "engineering_validation_not_release"}},
        }

    def check(self, profile: str = "full-engine-rc") -> str:
        path = self.root / "packaging/export/final-dependency-closure.json"
        path.write_text(json.dumps(self.closure), encoding="utf-8")
        return SUBJECT.validate_dependency_closure(self.root, self.manifest, profile)

    def node(self, name: str) -> dict:
        return next(n for n in self.closure["nodes"] if n["id"] == name)

    def rebind(self) -> None:
        bind_synthetic_acceptance(self.closure)

    def test_status_only(self) -> None:
        self.closure = {"status": "accepted_full_engine_rc"}
        with self.assertRaises(ValueError):
            self.check()

    def test_missing_lzma(self) -> None:
        # Remove it consistently from every declared census: hard required edge
        # must still reject a graph that merely agrees with its own omission.
        self.closure["nodes"] = [n for n in self.closure["nodes"] if n["id"] != "liblzma"]
        self.closure["edges"] = [e for e in self.closure["edges"] if e["to"] != "liblzma"]
        self.node("jxl")["dependencies"].remove("liblzma")
        self.closure["selection"]["components"].remove("liblzma")
        self.manifest["export_profiles"]["full-engine-rc"]["decoder_selection"] = copy.deepcopy(self.closure["selection"])
        self.rebind()
        with self.assertRaisesRegex(ValueError, "liblzma"):
            self.check()

    def test_static_lzma(self) -> None:
        self.assertNotIn("member", self.node("liblzma"))
        self.assertEqual(self.check(), STRUCTURAL_ONLY)
        self.node("liblzma")["static"]["container"] = "spatial"
        self.rebind()
        with self.assertRaisesRegex(ValueError, "containing-artifact"):
            self.check()

    def test_compiled_static_native_containing_artifact(self) -> None:
        # Both a selected static decoder and its transitive static tables live
        # in cix. No fabricated edge back to cix or extra DSO is needed.
        for nested in (False, True):
            with self.subTest(transitive_static_tables=nested):
                self.closure = synthetic_static_native(nested)
                self.manifest["export_profiles"]["full-engine-rc"]["decoder_selection"] = copy.deepcopy(self.closure["selection"])
                self.assertNotIn("member", self.node("native_static"))
                self.assertNotIn("cix", self.node("native_static")["dependencies"])
                if nested:
                    self.assertNotIn("member", self.node("native_tables"))
                self.assertEqual(self.check(), STRUCTURAL_ONLY)

    def test_compiled_static_native_rejects_unrelated_artifact(self) -> None:
        self.closure = synthetic_static_native(True)
        self.manifest["export_profiles"]["full-engine-rc"]["decoder_selection"] = copy.deepcopy(self.closure["selection"])
        asset = next(a for a in self.closure["assets"] if a["route"] == "route-native")["items"][0]
        self.assertEqual(self.check(), STRUCTURAL_ONLY)
        # These images are globally selected, even dependencies of cix, but
        # neither is this static decoder's container or dependency image.
        for wrong in ("spatial", "jxl", "libc", "native_static", "undeclared-image"):
            with self.subTest(container=wrong):
                asset["container"] = wrong
                self.rebind()
                with self.assertRaisesRegex(ValueError, "compiled asset containing artifact"):
                    self.check()

    def test_compiled_packaged_dependency_containing_artifact(self) -> None:
        assets = next(a for a in self.closure["assets"] if a["route"] == "route-jxl")
        assets["items"] = [{
            "id": "compiled-jxl-tables", "disposition": "compiled", "container": "libjxl",
            "source_ref": reference("libjxl/compiled-tables/source"),
            "build_ref": reference("libjxl/compiled-tables/build"),
        }]
        self.rebind()
        self.assertEqual(self.check(), STRUCTURAL_ONLY)
        # Root reachability alone cannot make an unrelated package admissible.
        assets["items"][0]["container"] = "spatial"
        self.rebind()
        with self.assertRaisesRegex(ValueError, "compiled asset containing artifact"):
            self.check()

    def test_system_runtime(self) -> None:
        self.assertNotIn("member", self.node("libc"))
        self.assertEqual(self.check(), STRUCTURAL_ONLY)
        del self.node("libc")["system"]["compatibility_ref"]
        self.rebind()
        with self.assertRaises(ValueError):
            self.check()

    def test_ambient_member(self) -> None:
        self.node("brotlidec")["dependencies"].append("undeclared-workstation-library")
        self.rebind()
        with self.assertRaisesRegex(ValueError, "dependency inventory"):
            self.check()
        self.node("brotlidec")["dependencies"].pop()
        self.closure["edges"][0]["resolution"] = "ambient-PATH"
        self.rebind()
        with self.assertRaisesRegex(ValueError, "ambient"):
            self.check()

    def test_dictionary_id0(self) -> None:
        good = dictionary()
        for corrupt in ("sha", "distribution", "carrier", "bytes"):
            with self.subTest(corrupt=corrupt):
                asset = copy.deepcopy(good)
                if corrupt == "sha":
                    asset["identity"]["sha256"] = "f" * 64
                elif corrupt == "bytes":
                    asset["identity"]["bytes"] += 1
                else:
                    if corrupt == "carrier":
                        del asset["identity"]["carrier"]
                    else:
                        del asset["distribution"]
                self.closure["assets"][0]["items"] = [asset]
                self.rebind()
                with self.assertRaises(ValueError):
                    self.check()

    def test_dictionary_explicit(self) -> None:
        self.closure["assets"][0]["items"] = [dictionary()]
        self.rebind()
        self.assertEqual(self.check(), STRUCTURAL_ONLY)
        # Asset correctness is not a substitute for an actual restore receipt.
        self.closure["acceptance"]["offline_restore"]["status"] = "unrun"
        with self.assertRaises(ValueError):
            self.check()

    def test_palette_archive(self) -> None:
        self.closure["assets"][0]["items"] = [{"id": "record-order-palette", "disposition": "archive-contained",
                                               "format_ref": reference("record-order/palette"), "fields": ["palette", "counts"]}]
        self.rebind()
        self.assertEqual(self.check(), STRUCTURAL_ONLY)
        self.closure["assets"][0]["items"][0]["external_palette"] = "workstation/palette"
        self.rebind()
        with self.assertRaises(ValueError):
            self.check()

    def test_evidence_only(self) -> None:
        for dependency in ("corpus-cache", "source-file", "result-certificate", "network-fetch", "pretrained-model", "training-samples"):
            with self.subTest(dependency=dependency):
                self.closure["assets"][0]["items"] = [{"id": "forbidden", "disposition": dependency}]
                self.rebind()
                with self.assertRaisesRegex(ValueError, "evidence-only"):
                    self.check()

    def test_subset(self) -> None:
        for role in ("paq_v215", "paq_v216", "paq_joint_discount", "paq_store_state", "jxl", "spatial"):
            with self.subTest(role=role):
                self.closure = synthetic_full_engine()
                self.closure["selection"]["routes"] = [r for r in self.closure["selection"]["routes"] if r["role"] != role]
                self.manifest["export_profiles"]["full-engine-rc"]["decoder_selection"] = copy.deepcopy(self.closure["selection"])
                self.rebind()
                with self.assertRaisesRegex(ValueError, "mandatory full-engine routes"):
                    self.check()
        self.closure = {"status": "incomplete"}
        self.assertEqual(self.check("source-members"), "not_checked_source_members_only")
        self.assertEqual(self.check("native-core-optional"), "optional_subset_not_full_engine_rc")
        self.assertEqual(self.check("engineering-validation"), "engineering_validation_not_release")

    def test_offline_restore(self) -> None:
        self.assertEqual(self.check(), STRUCTURAL_ONLY)
        pristine = copy.deepcopy(self.closure)
        for flaw in ("missing", "unrun", "same-reviewer", "no-fetch", "hash", "route", "subject"):
            with self.subTest(flaw=flaw):
                self.closure = copy.deepcopy(pristine)
                restore = self.closure["acceptance"]["offline_restore"]
                if flaw == "missing":
                    del self.closure["acceptance"]["offline_restore"]
                elif flaw == "unrun":
                    restore["status"] = "unrun"
                elif flaw == "same-reviewer":
                    restore["reviewer"] = self.closure["producer"]
                elif flaw == "no-fetch":
                    restore["no_fetch"] = False
                elif flaw == "hash":
                    restore["archives"][0]["restored_sha256"] = "f" * 64
                elif flaw == "route":
                    restore["archives"].pop()
                else:
                    restore["subject_sha256"] = "f" * 64
                with self.assertRaises(ValueError):
                    self.check()

    def test_current_identity_and_applied_overlay(self) -> None:
        for key, bad in (("target", "another-target"), ("features", []), ("overlays", []), ("revision", "9" * 40)):
            with self.subTest(field=key):
                self.closure = synthetic_full_engine()
                self.closure["selection"]["identity"][key] = bad
                self.rebind()
                with self.assertRaisesRegex(ValueError, "decoder_selection"):
                    self.check()
        self.closure = synthetic_full_engine()
        (self.root / "src/synthetic-routes.txt").write_bytes(b"changed source")
        with self.assertRaisesRegex(ValueError, "source member pin mismatch"):
            self.check()

    def test_empty_success_and_wrong_types(self) -> None:
        for field, value in (("nodes", []), ("assets", []), ("unresolved", False), ("unresolved", None),
                             ("unresolved", [{"field": "artifact", "status": "unsupplied", "resolver": "actual owner", "action": "supply exact retained tuple"}])):
            with self.subTest(field=field, value=value):
                self.closure = synthetic_full_engine()
                self.closure[field] = value
                self.rebind()
                with self.assertRaises(ValueError):
                    self.check()
        self.closure = synthetic_full_engine()
        self.node("jxl")["member"]["bytes"] = True
        self.rebind()
        with self.assertRaises(ValueError):
            self.check()

    def test_outer_allowlist_and_source_root_checks_remain(self) -> None:
        self.check()
        result = SUBJECT.validate(self.root, self.manifest, "full-engine-rc")
        self.assertEqual(result["dependency_closure_status"], STRUCTURAL_ONLY)
        self.assertEqual(result["status"], "validated_source_members_only_not_exported")
        (self.root / "unexpected.txt").write_text("unexpected")
        with self.assertRaisesRegex(ValueError, "unallowlisted"):
            SUBJECT.validate(self.root, self.manifest, "full-engine-rc")
        (self.root / "unexpected.txt").unlink()
        self.manifest["required_exact_files"].append("missing-required")
        with self.assertRaisesRegex(ValueError, "required source files missing"):
            SUBJECT.validate(self.root, self.manifest, "source-members")
        with self.assertRaisesRegex(ValueError, "source root must be"):
            SUBJECT.validate(Path(self.temp.name), self.manifest, "source-members")


if __name__ == "__main__":
    unittest.main()
