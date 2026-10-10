"""Independent structural verifier for published runtime v2 packages."""
import hashlib
import json
import math


def decode_runtime_json(raw):
    """Read ordinary JSON or the bounded immutable pool used by large packages."""
    assert len(raw) <= 16 * 1024 * 1024, "encoded package exceeds 16 MiB"

    def members(rows):
        result = {}
        for key, value in rows:
            assert key not in result, f"duplicate JSON key: {key}"
            result[key] = value
        return result

    def invalid_constant(value):
        raise AssertionError(f"invalid JSON constant: {value}")

    package = json.loads(raw, object_pairs_hook=members, parse_constant=invalid_constant)
    if not isinstance(package, dict) or "package_encoding" not in package:
        return package
    assert set(package) == {"package_encoding", "root", "pool"}, "invalid pool fields"
    assert package["package_encoding"] == "interned_json_v1", "unknown pool encoding"
    pool = package["pool"]
    assert isinstance(pool, list) and 0 < len(pool) <= 8 * 1024 * 1024, "invalid pool size"
    assert type(package["root"]) is int and package["root"] == len(pool) - 1, "invalid pool root"
    values, lengths, depths, referenced = [], [], [], set()
    for index, entry in enumerate(pool):
        assert isinstance(entry, dict) and len(entry) == 1, "invalid pool entry"
        tag, body = next(iter(entry.items()))

        def child(reference):
            assert type(reference) is int and 0 <= reference < index, "pool reference must point backward"
            referenced.add(reference)
            return values[reference]

        if tag == "v":
            assert not isinstance(body, (dict, list)), "scalar entry contains a container"
            assert not isinstance(body, float) or math.isfinite(body), "nonfinite scalar"
            value = body
            length = len(json.dumps(body, ensure_ascii=False, separators=(",", ":")).encode("utf8"))
            depth = 0
        elif tag == "a":
            assert isinstance(body, list), "invalid array entry"
            assert len(body) <= 100_000, "logical array exceeds 100000 entries"
            value = [child(reference) for reference in body]
            length = 2 + sum(lengths[reference] for reference in body) + max(0, len(body) - 1)
            depth = max((depths[reference] + 1 for reference in body), default=1)
        elif tag == "o":
            assert isinstance(body, list), "invalid object entry"
            assert len(body) <= 100_000, "logical object exceeds 100000 members"
            value, previous, length, depth = {}, None, 2 + len(body) + max(0, len(body) - 1), 1
            for pair in body:
                assert isinstance(pair, list) and len(pair) == 2, "invalid object member"
                key_reference, value_reference = pair
                key, item = child(key_reference), child(value_reference)
                assert isinstance(key, str), "object key must reference a string"
                assert previous is None or previous < key, "duplicate or unsorted object keys"
                value[key], previous = item, key
                length += lengths[key_reference] + lengths[value_reference]
                depth = max(depth, depths[key_reference] + 1, depths[value_reference] + 1)
        else:
            raise AssertionError("unknown pool entry tag")
        assert length <= 64 * 1024 * 1024, "expanded package exceeds 64 MiB"
        assert depth <= 128, "expanded package exceeds depth bound"
        values.append(value)
        lengths.append(length)
        depths.append(depth)
    assert referenced == set(range(len(pool) - 1)), "unreachable pool entry"
    return values[-1]


def cue_media_assets(effects, project, read):
    assets = set()
    pending = [definition["effect"] for definition in effects]
    while pending:
        effect = pending.pop()
        kind = effect["type"]
        if kind == "audio":
            assets.add(effect["asset"])
        elif kind in ("sequence", "parallel_all"):
            pending.extend(definition["effect"] for definition in effect["children"])
        elif kind == "dialogue":
            owner = project["text_owners"][effect["text"]]
            content = read(project["modules"][owner]["static_content"])
            contract = content["text_contracts"][effect["text"]]
            assets.update(image["asset"] for image in contract.get("images", []))
        elif kind == "dialogue_style":
            style = project["theme"]["dialogue_styles"][effect["style"]]
            if style["dialogue"].get("background"):
                assets.add(style["dialogue"]["background"])
        elif kind == "dialogue_decoration" and effect.get("image"):
            assets.add(effect["image"]["asset"])
        elif kind == "stage_present":
            transition = effect.get("transition", {})
            if transition.get("type") == "mask":
                assets.add(transition["asset"])
            owner = project["scene_owners"][effect["scene"]]
            content = read(project["modules"][owner]["static_content"])
            inherited = set(effect.get("inherit_images", []))
            assets.update(node["asset"] for node in content["scenes"][effect["scene"]]
                          if node.get("asset") and node["id"] not in inherited)
    return assets


def function_scope(index, module, signature, modules):
    assert {"module", "signature"} <= set(index) <= {"module", "signature", "execution_module"}
    assert index["module"] == module and index["signature"] == signature, "function index mismatch"
    scope = index.get("execution_module")
    assert scope is None or isinstance(scope, str), "invalid function execution module"
    if scope is None:
        scope = module
    assert scope in modules, "unknown function execution module"
    return scope


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
            cache[identity] = decode_runtime_json(raw)
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
        scopes = set()
        for identity, signature in index["functions"].items():
            assert identity not in functions, f"duplicate function owner: {identity}"
            functions[identity] = module
            scopes.add(function_scope(project["function_index"][identity], module, signature, modules))
            function = code["functions"][identity]
            assert signature["params"] == function["params"]
            assert signature.get("returns") == function.get("returns")
            assert signature["entry"] == function["entry"]
            ops = function["blocks"][function["entry"]]["ops"]
            assert signature["entry_op"] == (ops[0]["id"] if ops else "@terminator")
        assert len(scopes) <= 1, "code package has mixed execution modules"
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
            for definition in body["effects"]:
                assert project["task_owners"][definition["id"]] == module
            assets = cue_media_assets(body["effects"], project, read)
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
