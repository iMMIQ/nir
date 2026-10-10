"""Independent package verifier regressions; no source game materials required."""
import json
import unittest
from verify_runtime import cue_media_assets, decode_runtime_json, function_scope


def encoded(pool, **fields):
    return json.dumps({"package_encoding": "interned_json_v1", "root": len(pool) - 1,
                       "pool": pool, **fields}).encode()


class RuntimePoolTests(unittest.TestCase):
    def test_plain_and_shared_unicode_objects_have_same_content(self):
        pool = [{"v": "label"}, {"v": "例"}, {"o": [[0, 1]]}, {"a": [2, 2]}]
        expected = [{"label": "例"}, {"label": "例"}]
        self.assertEqual(decode_runtime_json(encoded(pool)), expected)
        self.assertEqual(decode_runtime_json(json.dumps(expected).encode()), expected)

    def test_invalid_references_fields_keys_and_reachability_are_rejected(self):
        cases = [
            encoded([{"a": [0]}]), encoded([{"a": [-1]}]),
            encoded([{"v": 1}, {"a": [True]}]),
            encoded([{"v": 1}, {"v": 2}]),
            encoded([{"v": []}]), encoded([{"x": 1}]),
            encoded([{"v": "key"}, {"o": [[0, 0], [0, 0]]}]),
            encoded([{"v": "z"}, {"v": "a"}, {"o": [[0, 1], [1, 0]]}]),
            encoded([{"v": 3}, {"o": [[0, 0]]}]),
            encoded([{"v": 1}], root=True), encoded([{"v": 1}], extra=0),
            encoded([{"v": 1e999}]),
            b'{"x":1,"x":2}', b'{"x":NaN}',
        ]
        for raw in cases:
            with self.subTest(raw=raw), self.assertRaises(AssertionError):
                decode_runtime_json(raw)

    def test_expansion_depth_and_input_bounds_are_checked_without_expansion(self):
        pool = [{"v": "x"}]
        pool += [{"a": [index - 1, index - 1]} for index in range(1, 25)]
        with self.assertRaisesRegex(AssertionError, "64 MiB"):
            decode_runtime_json(encoded(pool))
        pool = [{"v": 0}] + [{"a": [index - 1]} for index in range(1, 130)]
        with self.assertRaisesRegex(AssertionError, "depth"):
            decode_runtime_json(encoded(pool))
        with self.assertRaisesRegex(AssertionError, "16 MiB"):
            decode_runtime_json(b' ' * (16 * 1024 * 1024 + 1))
        with self.assertRaisesRegex(AssertionError, "logical array"):
            decode_runtime_json(encoded([{"v": 0}, {"a": [0] * 100_001}]))


class RuntimeRecipeTests(unittest.TestCase):
    def test_code_packages_keep_a_known_scope_and_matching_signature(self):
        modules = {"shared": {}, "code": {}}
        signature = {"entry": "body"}
        index = {"module": "code", "signature": signature, "execution_module": "shared"}
        self.assertEqual(function_scope(index, "code", signature, modules), "shared")
        self.assertEqual(function_scope({"module": "code", "signature": signature},
                                        "code", signature, modules), "code")
        for changes in [{"execution_module": "unknown"}, {"execution_module": False},
                        {"execution_module": ""}, {"signature": {}},
                        {"module": "shared"}, {"extra": 0}]:
            with self.subTest(changes=changes), self.assertRaises(AssertionError):
                function_scope({**index, **changes}, "code", signature, modules)

    def test_text_style_decoration_composition_and_inherited_images(self):
        project = {
            "modules": {"shared": {"static_content": "shared-static"}},
            "text_owners": {"line": "shared"}, "scene_owners": {"stage": "shared"},
            "theme": {"dialogue_styles": {"window": {"dialogue": {"background": "box"}}}},
        }
        content = {"shared-static": {
            "text_contracts": {"line": {"images": [{"asset": "inline"}]}},
            "scenes": {"stage": [{"id": "old", "asset": "inherited"},
                                  {"id": "new", "asset": "picture"}]},
        }}
        effects = [{"effect": effect} for effect in [
            {"type": "parallel_all", "children": [
                {"effect": {"type": "dialogue", "text": "line"}},
                {"effect": {"type": "audio", "asset": "music"}},
                {"effect": {"type": "dialogue_style", "style": "window"}},
                {"effect": {"type": "dialogue_decoration", "image": {"asset": "portrait"}}},
            ]},
            {"type": "stage_present", "scene": "stage", "inherit_images": ["old"],
             "transition": {"type": "mask", "asset": "mask"}},
            {"type": "dialogue_decoration", "image": None},
        ]]
        expected = {"inline", "music", "box", "portrait", "picture", "mask"}
        self.assertEqual(cue_media_assets(effects, project, content.__getitem__), expected)
        effects[1]["effect"]["inherit_images"] = []
        self.assertEqual(cue_media_assets(effects, project, content.__getitem__), expected | {"inherited"})


if __name__ == "__main__":
    unittest.main()
