"""Independent structural verifier for published runtime v2 packages."""
import hashlib
import json


def verify_runtime(directory, release):
    cache = {}

    def read(identity):
        if identity not in cache:
            assert identity in release["objects"], f"missing runtime object: {identity}"
            descriptor = release["objects"][identity]
            path = (directory / descriptor["path"]).resolve()
            assert path.is_relative_to(directory.resolve() / "objects")
            raw = path.read_bytes()
            assert len(raw) == descriptor["bytes"]
            assert hashlib.sha256(raw).hexdigest() == identity
            cache[identity] = json.loads(raw)
        return cache[identity]

    executable = read(release["program"])
    assert executable["format"] == 2, "expected runtime executable v2"
    project = executable["program"]
    assert project["format"] == 2
    assert not ({"functions", "scenes", "cues", "choices", "texts"} & project.keys()), "root contains content bodies"
    assert isinstance(project["locales"], list), "root locales must be identities only"
    modules = project["modules"]
    assert modules, "missing module indexes"
    functions = {}
    texts = {}
    for module, index in modules.items():
        code = read(index["code"])
        assert code["format"] == 2 and code["module"] == module
        assert set(code["functions"]) == set(index["functions"])
        for identity, signature in index["functions"].items():
            assert identity not in functions, f"duplicate function owner: {identity}"
            functions[identity] = module
            assert project["function_index"][identity] == {"module": module, "signature": signature}
            function = code["functions"][identity]
            assert signature["params"] == function["params"]
            assert signature.get("returns") == function.get("returns")
            assert signature["entry"] == function["entry"]
            ops = function["blocks"][function["entry"]]["ops"]
            assert signature["entry_op"] == (ops[0]["id"] if ops else "@terminator")
        declarations = read(index["static_content"])
        assert declarations["format"] == 2 and declarations["module"] == module
        for table, owners in [("scenes", "scene_owners"), ("cues", "cue_owners"), ("choices", "choice_owners"), ("text_contracts", "text_owners")]:
            expected = {identity for identity, owner in project[owners].items() if owner == module}
            assert set(declarations.get(table, {})) == expected, f"static ownership mismatch: {module}/{table}"
        for identity, contract in declarations.get("text_contracts", {}).items():
            assert identity not in texts
            texts[identity] = module
            summary = project["text_contracts"][identity]
            assert summary["module"] == module
            for field in ["source_revision", "contract_revision", "meaning_revision", "contract_digest"]:
                assert summary[field] == contract[field]
        assert set(index["texts"]) == set(declarations.get("text_contracts", {}))
        assert set(index["locales"]) == (set(project["locales"]) if index["texts"] else set())
        for locale, identity in index["locales"].items():
            bundle = read(identity)
            assert bundle["format"] == 2 and bundle["module"] == module and bundle["locale"] == locale
            assert set(bundle["texts"]) == set(index["texts"])
        recipes = declarations.get("activation_recipes", {})
        assert set(recipes) == set(declarations.get("cues", {}))
        for cue, body in declarations.get("cues", {}).items():
            assets = set()
            for definition in body["effects"]:
                assert project["task_owners"][definition["id"]] == module
                effect = definition["effect"]
                if effect["type"] == "audio":
                    assets.add(effect["asset"])
                elif effect["type"] == "stage_present":
                    nodes = declarations["scenes"][effect["scene"]]
                    assets.update(node["asset"] for node in nodes if node.get("asset"))
            assert set(recipes[cue]) == assets, f"recipe mismatch: {cue}"
    assert project["entry"] in functions
    assert set(functions) == set(project["function_index"])
    assert texts == project["text_owners"]
    assert set(texts) == set(project["text_contracts"])
    asset_ids = set()
    for catalog, identity in project["catalogs"].items():
        package = read(identity)
        assert package["format"] == 2 and package["catalog"] == catalog
        expected = {asset for asset, index in project["assets"].items() if index["catalog"] == catalog}
        assert set(package["assets"]) == expected
        for asset, descriptor in package["assets"].items():
            assert asset not in asset_ids
            asset_ids.add(asset)
            index = project["assets"][asset]
            assert descriptor["kind"] == index["kind"] and descriptor["object"] == index["object"]
            assert descriptor["object"] in release["objects"]
            assert descriptor["bytes"] == release["objects"][descriptor["object"]]["bytes"]
    assert asset_ids == set(project["assets"])
    return project
