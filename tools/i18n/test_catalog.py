"""Focused schema, escaping and deterministic generation tests; no native compiler."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location("cix_catalog", Path(__file__).with_name("catalog.py"))
catalog = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(catalog)

def entry(parameters=None, pattern="plain", plural=None):
    value = {"context": "test context", "parameters": parameters or {},
             "forms": {"other": pattern}}
    if plural is not None:
        value["plural"] = plural
        value["forms"]["one"] = pattern
    return value

class CatalogTests(unittest.TestCase):
    def test_current_sources_reproduce_generated_bytes(self):
        native = Path(__file__).resolve().parents[2] / "rust" / "cix-native"
        self.assertEqual(catalog.render(catalog.read_messages(native)),
                         (native / "src" / "i18n_catalog.rs").read_bytes())

    def test_duplicate_json_keys_and_cross_fragment_ids_are_rejected(self):
        with self.assertRaises(ValueError):
            json.loads('{"a":1,"a":2}', object_pairs_hook=catalog.unique_object)
        with tempfile.TemporaryDirectory() as directory:
            native = Path(directory)
            (native / "locales" / "en").mkdir(parents=True)
            source = {"schema": "cix-messages-v1", "locale": "en",
                      "messages": {"test.same": entry()}}
            for relative in ["locales/en.json", "locales/en/duplicate.json"]:
                (native / relative).write_text(json.dumps(source), encoding="utf-8")
            with self.assertRaises(ValueError):
                catalog.read_messages(native)

    def test_unknown_schema_and_wrong_locale_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            native = Path(directory)
            (native / "locales").mkdir()
            for schema, locale in [("cix-messages-v2", "en"), ("cix-messages-v1", "fr")]:
                (native / "locales" / "en.json").write_text(
                    json.dumps({"schema": schema, "locale": locale, "messages": {}}),
                    encoding="utf-8")
                with self.assertRaises(ValueError):
                    catalog.read_messages(native)

    def test_named_sets_reordering_repetition_and_braces(self):
        catalog.validate_message("test.named",
            entry({"a": "text", "b": "code"}, "{{literal}} {b} {a} {a}"))
        for pattern in ["{a}", "{a} {b} {extra}", "{a} {b", "{a} {b} }", "{}"]:
            with self.assertRaises(ValueError):
                catalog.validate_message("test.named", entry({"a": "text", "b": "code"}, pattern))

    def test_numeric_plural_and_exact_english_forms(self):
        catalog.validate_message("test.plural", entry({"n": "number"}, "{n}", plural="n"))
        for invalid in [
            entry({"n": "text"}, "{n}", plural="n"),
            entry({"n": "number"}, "{n}", plural="missing"),
            {"context": "x", "parameters": {}, "forms": {"one": "one", "other": "other"}},
            {"context": "x", "parameters": {}, "forms": {"other": "x"}, "extra": "x"},
        ]:
            with self.assertRaises(ValueError):
                catalog.validate_message("test.plural", invalid)

    def test_rust_unicode_newline_delimiter_and_control_escaping(self):
        self.assertEqual(catalog.rust_string("µ\ntext"), 'r#"µ\ntext"#')
        self.assertEqual(catalog.rust_string('a"#b'), 'r##"a"#b"##')
        max_legal = '"' + '#' * 254
        self.assertEqual(catalog.rust_string(max_legal),
                         'r' + '#' * 255 + '"' + max_legal + '"' + '#' * 255)
        needs_cooked = '"' + '#' * 255
        self.assertEqual(catalog.rust_string(needs_cooked),
                         '"\\"' + '#' * 255 + '"')
        self.assertEqual(catalog.rust_string('a\r\0"\\'),
                         '"a\\u{d}\\u{0}\\"\\\\"')
        with self.assertRaises(ValueError):
            catalog.validate_message("test.invalid", entry(pattern="\ud800"))

    def test_generation_order_is_independent_of_source_order(self):
        left = {"test.z": entry(), "test.a": entry({"b": "code", "a": "text"}, "{a}{b}")}
        right = {"test.a": entry({"a": "text", "b": "code"}, "{a}{b}"), "test.z": entry()}
        self.assertEqual(catalog.render(left), catalog.render(right))

if __name__ == "__main__":
    unittest.main()
