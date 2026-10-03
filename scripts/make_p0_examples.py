#!/usr/bin/env python3
"""Reproducible P0 acceptance fixtures: original neutral content, no third-party
game assets (see docs/NIR-NEXT-P0-BASELINE.md).

Generates examples/reading-lamp (Gate dialogue, voice binding, window and
stage transitions) and examples/replay-atlas (locked replay gallery). Images
and sounds are synthesized deterministically here; the CJK font subset is
produced separately with the vendored HarfBuzz toolchain from the character
lists this script writes next to each font source (reader.chars.txt)."""
from pathlib import Path
import json, math, wave, struct, zlib, hashlib

ROOT = Path(__file__).resolve().parents[1]


def dump(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")


def val(v):
    kind = "bool" if isinstance(v, bool) else "i32" if isinstance(v, int) else "string"
    return {"type": "const", "value": {"type": kind, "value": v}}


def end(name):
    return {"type": "end", "outcome": name}


def activate(cue, nxt):
    return {"type": "activate", "cue": cue, "next": nxt}


def wait(task, nxt, milestone="finished"):
    return {
        "type": "await",
        "conditions": [{"task": task, "milestone": {"type": milestone}}],
        "next": nxt,
        "on_cancelled": "cancelled",
        "on_failed": "failed",
    }


def marker(task, mid, nxt):
    return {
        "type": "await",
        "conditions": [{"task": task, "milestone": {"type": "marker", "id": mid}}],
        "next": nxt,
        "on_cancelled": "cancelled",
        "on_failed": "failed",
    }


def op(i, typ, **kw):
    return {"id": i, "operation": {"type": typ, **kw}}


def block(term, *ops):
    return {"ops": list(ops), "terminator": term}


def goto(name):
    return {"type": "goto", "target": name}


def effect(i, typ, scope="scene", **kw):
    return {"id": i, "scope": scope, "effect": {"type": typ, **kw}}


def node(i, asset, x, y, w, h, **kw):
    return dict(id=i, asset=asset, x=x, y=y, width=w, height=h, **kw)


def faults():
    return {
        "cancelled": block(
            {"type": "fault", "code": "E_CANCELLED", "message": "A required performance was cancelled."}
        ),
        "failed": block(
            {"type": "fault", "code": "E_PERFORMANCE", "message": "A required performance failed."}
        ),
    }


# --- deterministic original artwork (pure stdlib PNG writer) -----------------

def png(path, size, paint):
    w, h = size

    def chunk(tag, data):
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    header = struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(marshal_rows(w, h, paint), 9))
        + chunk(b"IEND", b"")
    )


def marshal_rows(w, h, paint):
    out = bytearray()
    for y in range(h):
        out.append(0)
        base = paint.row(y)
        for x in range(w):
            out += bytes(paint.pixel(x, y, base))
    return bytes(out)


class Gradient:
    """Vertical gradient plus rectangle and glow layers, evaluated lazily."""

    def __init__(self, top, bottom, w, h):
        self.top, self.bottom, self.w, self.h = top, bottom, w, h
        self.rects = []  # (x0,y0,x1,y1, rgba)
        self.glows = []  # (cx,cy,rx,ry, rgba)

    def rect(self, box, color):
        self.rects.append((box, color))

    def glow(self, cx, cy, rx, ry, color):
        self.glows.append((cx, cy, rx, ry, color))

    def row(self, y):
        t = y / max(1, self.h - 1)
        return tuple(int(a + (b - a) * t) for a, b in zip(self.top, self.bottom))

    def pixel(self, x, y, base):
        r, g, b = base
        a = 255
        for (x0, y0, x1, y1), color in self.rects:
            if x0 <= x < x1 and y0 <= y < y1:
                r, g, b = color[0], color[1], color[2]
                a = color[3] if len(color) > 3 else 255
        for cx, cy, rx, ry, color in self.glows:
            d = math.hypot((x - cx) / rx, (y - cy) / ry)
            if d < 1.0:
                k = (1.0 - d) ** 2
                r = int(r + (color[0] - r) * k)
                g = int(g + (color[1] - g) * k)
                b = int(b + (color[2] - b) * k)
        return (r, g, b, a)


def plate(path, w, h, face, edge, hover=False):
    g = Gradient(face, face, w, h)
    g.rect((0, 0, w, 4), edge + (255,))
    g.rect((0, h - 4, w, h), edge + (255,))
    g.rect((0, 0, 4, h), edge + (255,))
    g.rect((w - 4, 0, w, h), edge + (255,))
    if hover:
        g.rect((8, 8, w - 8, h - 8), tuple(min(255, c + 26) for c in face) + (255,))
    png(path, (w, h), g)


def darkened(path, w, h, face):
    g = Gradient((0, 0, 0), (6, 6, 8), w, h)
    g.rect((0, 0, w, 3), (30, 30, 34, 255))
    g.rect((0, h - 3, w, h), (30, 30, 34, 255))
    png(path, (w, h), g)


def tone(path, secs, make):
    rate = 24000
    frames = bytearray()
    for i in range(int(secs * rate)):
        t = i / rate
        frames += struct.pack("<h", int(max(-1, min(1, make(t, secs))) * 32767))
    path.parent.mkdir(parents=True, exist_ok=True)
    with wave.open(str(path), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(rate)
        w.writeframes(bytes(frames))


# --- shared UI-copy closure --------------------------------------------------

def ui_copy_chars():
    chars = set(chr(i) for i in range(32, 127))
    chars.update(" →←↗•—…–·％１２３４５６７８９０")
    for pattern in ("crates/nir-presentation",):
        base = ROOT / pattern
        for p in base.rglob("*.ftl"):
            chars.update(p.read_text())
        for p in (base / "src").rglob("*.rs"):
            chars.update(p.read_text())
    return chars


def write_chars(fixture, extra):
    chars = ui_copy_chars() | set(extra)
    text = "".join(sorted(c for c in chars if ord(c) >= 32))
    (fixture / "assets/source/reader.chars.txt").parent.mkdir(parents=True, exist_ok=True)
    (fixture / "assets/source/reader.chars.txt").write_text(text + "\n")


def base_tree(fixture, game_id, slug, title, title_scene, scenarios, theme, tokens, player, docs, prose, contracts, extra_notices):
    (fixture / "content/ch01/texts").mkdir(parents=True, exist_ok=True)
    (fixture / "themes/main").mkdir(parents=True, exist_ok=True)
    (fixture / "tests/scenarios").mkdir(parents=True, exist_ok=True)
    (fixture / "credits").mkdir(parents=True, exist_ok=True)
    (fixture / "config").mkdir(parents=True, exist_ok=True)
    (fixture / "assets/fonts").mkdir(parents=True, exist_ok=True)
    (fixture / "game.toml").write_text(
        f'''project_format = 1
[game]
id = "{game_id}"
slug = "{slug}"
title = "{title}"
version = "0.1.0"
source_locale = "zh-Hans"
title_scene = "{title_scene}"
[engine]
api = "nir-player/0.1"
capability_profile = "web-v1"
runtime_preset = "web-standard"
[stage]
width = 1280
height = 720
[inputs]
modules = ["content/ch01/module.toml"]
asset_catalogs = ["assets/catalog.toml"]
theme = "themes/main/theme.toml"
player = "config/player.toml"
locales = "config/locales.toml"
scenarios = [{", ".join(f'"{s}"' for s in scenarios)}]
notices = ["credits/README.md", "assets/fonts/Noto-OFL.txt"{"".join(f', "{n}"' for n in extra_notices)}]
'''
    )
    (fixture / "content/ch01/module.toml").write_text(
        '''module_format = 1
id = "ch01"
sources = ["story.nir.json"]
text_contracts = "texts/contracts.json"
text_revisions = "texts/revisions.json"

[exports]
start = "main"

[text_bundles]
zh-Hans = "texts/zh-Hans.json"
en = "texts/en.json"
'''
    )
    (fixture / "config/locales.toml").write_text(
        '''format = 1
default_ui = "zh-Hans"
default_text = "zh-Hans"

[ui]
zh-Hans = ["font.reader"]
en = ["font.reader"]

[text]
zh-Hans = ["font.reader"]
en = ["font.reader"]
'''
    )
    (fixture / "config/player.toml").write_text(player)
    (fixture / "themes/main/theme.toml").write_text(theme)
    dump(fixture / "themes/main/tokens.json", tokens)
    dump(fixture / "content/ch01/texts/contracts.json", contracts)
    for loc in ("zh-Hans", "en"):
        dump(fixture / f"content/ch01/texts/{loc}.json", docs(loc))
    ledger = {"format": 1, "source_locale": "zh-Hans", "texts": {}}
    for key, contract in contracts.items():
        def digest(value):
            return hashlib.sha256(json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode()).hexdigest()

        contract_digest = digest([1, contract["contract_revision"], contract["meaning_revision"], contract["params"], contract["gates"]])
        ledger["texts"][key] = {
            "source_revision": 1,
            "contract_revision": 1,
            "meaning_revision": 1,
            "source_digest": digest(docs("zh-Hans")[key]["spans"]),
            "contract_digest": contract_digest,
            "shape_digest": digest([contract["params"], contract["gates"]]),
            "reviewed": {"en": digest(docs("en")[key])},
        }
    dump(fixture / "content/ch01/texts/revisions.json", ledger)


def credits(fixture, what):
    (fixture / "credits/README.md").write_text(
        f"""# 素材来源

{what}由仓库 `scripts/make_p0_examples.py` 确定性原创生成，以 CC0-1.0 提供；声音为合成测试音，不是真人配音。

字体子集来自仓库 SDK 模板的 Noto Sans CJK SC Regular 2.004 母本
（`templates/minimal/assets/fonts/`，SIL OFL 1.1，附完整许可）。`assets/source/reader.chars.txt`
列出该字体必须覆盖的字符；子集用与编译器相同的 vendored HarfBuzz 工具链生成
（见仓库 `docs/NIR-NEXT-P0-BASELINE.md` 的重建步骤）。新增文字时必须补充字体覆盖。

本工程是 NIR-NEXT P0 验收样例；不含任何第三方游戏内容。
"""
    )
    shutil_copy = (ROOT / "templates/minimal/assets/fonts/OFL.txt").read_bytes()
    (fixture / "assets/fonts/Noto-OFL.txt").write_bytes(shutil_copy)


# --- fixture A: reading-lamp --------------------------------------------------

def build_reading_lamp():
    out = ROOT / "examples/reading-lamp"
    lines = {
        "speaker.lamp": ("灯", "the lamp"),
        "intro": (
            "台灯亮起来的时候，房间退成一小圈暖色。你翻开摊在桌上的笔记。",
            "When the desk lamp comes on, the room folds back into a small circle of warmth. You open the notebook lying on the desk.",
        ),
        "note": (
            "风从窗缝里挤进来，纸页轻轻响了一声。",
            "Wind slips through the window seam, and the pages rustle softly.",
        ),
        "note.tail": ("你伸手压住那一页，继续读下去。", "You press the page flat and read on."),
        "spoken": (
            "灯芯轻轻一跳：「这页记的，是起风的那天。」",
            "The filament flickers: “This page holds the day the wind rose.”",
        ),
        "spoken.tail": ("「再往后读，天就要亮了。」", "“Read further, and the sky will soon be light.”"),
        "ask": ("天色将明未明。要把这盏灯留到日出吗？", "The night is almost over. Will you keep the lamp lit until sunrise?"),
        "opt.bright": ("让灯亮到日出", "Keep the lamp lit until sunrise"),
        "opt.dim": ("现在熄灯安睡", "Put out the lamp and sleep now"),
        "bright_line": (
            "灯光落在最后一行字上，像一句没有说完的晚安。",
            "The light settles on the last line, like a good-night that was never finished.",
        ),
        "dim_line": ("你合上笔记。黑暗温柔地接管了房间。", "You close the notebook, and the dark gently takes over the room."),
    }
    gates = {"note": ["chime"], "spoken": ["rest"]}
    contracts = {
        k: {"source_revision": 1, "contract_revision": 1, "meaning_revision": 1, "gates": gates.get(k, []), "params": {}}
        for k in lines
        if not k.endswith(".tail")
    }

    def docs(loc):
        i = 0 if loc == "zh-Hans" else 1
        out_docs = {}
        for key, pair in lines.items():
            spans = [{"type": "text", "id": "body", "text": pair[i], "emphasis": False}]
            if key == "note":
                spans += [{"type": "gate", "id": "chime"}, {"type": "text", "id": "tail", "text": lines["note.tail"][i], "emphasis": False}]
            if key == "spoken":
                spans += [{"type": "gate", "id": "rest"}, {"type": "text", "id": "tail", "text": lines["spoken.tail"][i], "emphasis": False}]
            if key in ("note.tail", "spoken.tail"):
                continue
            out_docs[key] = {"source_revision": 1, "contract_revision": 1, "spans": spans}
        return out_docs

    scenes = {
        "desk": [node("background", "bg.desk", 0, 0, 1280, 720)],
        "window": [node("background", "bg.window", 0, 0, 1280, 720)],
    }
    cues = {
        "open": {
            "effects": [
                effect("stage", "stage_present", scene="desk", duration_us="0"),
                effect("music", "audio", scope="session", asset="audio.bgm", bus="bgm", looped=True, gain=0.7),
            ]
        },
        "evening": {"effects": [effect("stage", "stage_present", scene="window", duration_us="900000")]},
        "intro": {"effects": [effect("line", "dialogue", scope="interaction", text="intro", speaker="", reveal_us="36000")]},
        "note": {"effects": [effect("line", "dialogue", scope="interaction", text="note", speaker="", reveal_us="36000")]},
        "chime": {"effects": [effect("chime", "audio", scope="interaction", asset="audio.chime", bus="sfx", looped=False)]},
        "spoken": {
            "effects": [
                effect("line", "dialogue", scope="interaction", text="spoken", speaker="speaker.lamp", reveal_us="40000"),
                effect("voice", "audio", scope="interaction", asset="audio.voice", bus="voice", looped=False),
            ]
        },
        "voice_stop": {"effects": [effect("voice_stop", "audio_stop", scope="session", target="voice", duration_us="50000")]},
        "ask": {"effects": [effect("line", "dialogue", scope="interaction", text="ask", speaker="", reveal_us="36000")]},
        "bright_line": {"effects": [effect("line", "dialogue", scope="interaction", text="bright_line", speaker="speaker.lamp", reveal_us="36000")]},
        "dim_line": {"effects": [effect("line", "dialogue", scope="interaction", text="dim_line", speaker="", reveal_us="36000")]},
    }
    blocks = {
        "start": block(activate("open", "show"), op("window.open", "dialogue_visibility", visible=True, transition={"type": "wipe", "direction": "bottom_to_top"}, duration_us="400000")),
        "show": block(activate("intro", "wait_intro")),
        "wait_intro": block(wait("line", "evening")),
        "evening": block(activate("evening", "wait_evening")),
        "wait_evening": block(wait("stage", "note_page")),
        "note_page": block(activate("note", "gate_chime")),
        "gate_chime": block(marker("line", "chime", "chime_page")),
        "chime_page": block(activate("chime", "wait_chime")),
        "wait_chime": block(wait("chime", "note_resume")),
        "note_resume": block(wait("line", "spoken_page"), op("note.continue", "dialogue_continue", task="line")),
        "spoken_page": block(activate("spoken", "bind_voice")),
        "bind_voice": block(goto("gate_rest"), op("voice.bind", "dialogue_voice", task="line", voice="voice", wait="sampled_remaining")),
        "gate_rest": block(marker("line", "rest", "rest_hide")),
        "rest_hide": block(goto("rest_show"), op("window.hide", "dialogue_visibility", visible=False, transition={"type": "dissolve"}, duration_us="300000")),
        "rest_show": block(goto("spoken_resume"), op("window.reopen", "dialogue_visibility", visible=True, transition={"type": "wipe", "direction": "bottom_to_top"}, duration_us="350000")),
        "spoken_resume": block(wait("line", "voice_out"), op("spoken.continue", "dialogue_continue", task="line")),
        "voice_out": block(activate("voice_stop", "ask_page")),
        "ask_page": block(activate("ask", "wait_ask")),
        "wait_ask": block(wait("line", "choose")),
        "choose": block(
            {
                "type": "interact",
                "choice": "keep",
                "branches": {"bright": "bright_page", "dim": "dim_page"},
                "on_empty": "failed",
                "result": "kept",
            }
        ),
        "bright_page": block(activate("bright_line", "wait_bright")),
        "wait_bright": block(wait("line", "bright_end")),
        "bright_end": block(end("lamp_lit"), op("unlock.sunrise", "profile_merge", key="lamp.sunrise")),
        "dim_page": block(activate("dim_line", "wait_dim")),
        "wait_dim": block(wait("line", "dim_end")),
        "dim_end": block(end("lamp_dimmed")),
        **faults(),
    }
    fragment = {
        "fragment_format": 1,
        "variables": {"kept": {"type": "string", "value": ""}},
        "functions": {"main": {"entry": "start", "blocks": blocks}},
        "scenes": scenes,
        "cues": cues,
        "choices": {
            "keep": {
                "options": [
                    {"id": "bright", "text": "opt.bright", "value": {"type": "string", "value": "sunrise"}},
                    {"id": "dim", "text": "opt.dim", "value": {"type": "string", "value": "rest"}},
                ]
            }
        },
    }
    dump(out / "content/ch01/story.nir.json", fragment)
    theme = '''format = 1
id = "theme.lamp"
base = "builtin.reader"
tokens = "tokens.json"

[slots]
"dialogue.main" = "builtin.dialogue"
"choice.main" = "builtin.choice"

[dialogue]
height = 240.0
padding = 24.0
font_size = 23.0

[choice]
width = 520.0
item_height = 58.0
'''
    player = '''format = 1

[defaults]
font_scale = 1.0
bgm_volume = 0.4
voice_volume = 0.9
sfx_volume = 0.6
reduced_motion = false
auto_delay_policy = "fixed"
auto_delay_us = "2500000"
prefetch_content = true
prefetch_media = true
'''
    tokens = {
        "background": [0.06, 0.05, 0.07, 1],
        "panel": [0.1, 0.08, 0.1, 0.97],
        "accent": [0.95, 0.72, 0.35, 1],
        "text": [0.96, 0.93, 0.87, 1],
        "muted": [0.66, 0.6, 0.55, 1],
    }
    scenarios = {}
    for sid, option, outcome, kept in [
        ("sunrise", "bright", "lamp_lit", "sunrise"),
        ("rest", "dim", "lamp_dimmed", "rest"),
    ]:
        scenarios[sid] = f'''format = 1
id = "{sid}"
entry = "main"
text_locale = "zh-Hans"

[[steps]]
action = "await_choice"
id = "keep"

[[steps]]
action = "choose"
option_id = "{option}"

[expect]
outcome = "{outcome}"

[expect.variables]
kept = {{ type = "string", value = "{kept}" }}
'''
    for sid, body in scenarios.items():
        (out / f"tests/scenarios/{sid}.toml").parent.mkdir(parents=True, exist_ok=True)
        (out / f"tests/scenarios/{sid}.toml").write_text(body)
    base_tree(
        out,
        "org.nir.reading-lamp",
        "reading-lamp",
        "夜灯书页 · Reading Lamp",
        "desk",
        ["tests/scenarios/sunrise.toml", "tests/scenarios/rest.toml"],
        theme,
        tokens,
        player,
        docs,
        lines,
        contracts,
        [],
    )
    credits(out, "背景与测试音效")
    # artwork: a warm desk at night, then a window towards dawn
    desk = Gradient((26, 20, 24), (12, 9, 12), 1280, 720)
    desk.glow(905, 250, 430, 330, (214, 158, 84))
    desk.rect((0, 620, 1280, 720), (44, 30, 26))
    desk.rect((60, 560, 460, 620), (58, 40, 32))
    desk.rect((700, 470, 760, 620), (38, 34, 30))
    png(out / "assets/source/desk.png", (1280, 720), desk)
    window = Gradient((24, 30, 48), (58, 66, 92), 1280, 720)
    window.glow(990, 180, 260, 220, (196, 205, 228))
    window.rect((760, 120, 1000, 420), (16, 20, 34))
    window.rect((876, 120, 884, 420), (64, 58, 48))
    window.rect((760, 262, 1000, 270), (64, 58, 48))
    window.rect((0, 640, 1280, 720), (30, 32, 44))
    png(out / "assets/source/window.png", (1280, 720), window)
    tone(out / "assets/source/bgm.wav", 6.0, lambda t, s: sum(math.sin(2 * math.pi * f * t) * 0.02 for f in (174.6, 220.0, 261.6)) * math.sin(math.pi * t / s) ** 2)
    tone(out / "assets/source/voice.wav", 1.6, lambda t, s: math.sin(2 * math.pi * (293.7 + 30 * math.sin(t * 7)) * t) * 0.09 * math.sin(math.pi * t / s) ** 2)
    tone(out / "assets/source/chime.wav", 0.7, lambda t, s: (math.sin(2 * math.pi * 1174 * t) + 0.35 * math.sin(2 * math.pi * 1760 * t)) * 0.2 * math.exp(-t * 6) * min(1, t * 120))
    assets = ['''format = 1

[[assets]]
id = "bg.desk"
kind = "image"
source = "source/desk.png"
rights = "CC0-1.0"
expected_size = [1280, 720]

[[assets]]
id = "bg.window"
kind = "image"
source = "source/window.png"
rights = "CC0-1.0"
expected_size = [1280, 720]

[[assets]]
id = "audio.bgm"
kind = "audio"
source = "source/bgm.wav"
rights = "CC0-1.0"

[[assets]]
id = "audio.voice"
kind = "audio"
source = "source/voice.wav"
rights = "CC0-1.0"

[[assets]]
id = "audio.chime"
kind = "audio"
source = "source/chime.wav"
rights = "CC0-1.0"

[[assets]]
id = "font.reader"
kind = "font"
source = "source/reader.otf"
rights = "OFL-1.1"
[assets.font]
mode = "subset"
face_index = 0
extra_characters = ""
license = "fonts/Noto-OFL.txt"
''']
    (out / "assets/catalog.toml").write_text("".join(assets))
    (out / "README.md").write_text(
        """# P0 验收样例：夜灯书页 · Reading Lamp

NIR-NEXT P0 验收样例之一（对白/语音/转场 + Gate）。完整原创中性内容；覆盖：

- 页内 Gate（风声 chime、灯语 rest 两个 marker），Gate 后继续同一对白；
- 显式语音绑定 `dialogue_voice`（`sampled_remaining` 等待策略）与页尾 50 ms 语音淡出停止；
- 消息窗口样式化显隐（`dialogue_visibility`：wipe 揭示 / dissolve 隐藏）与舞台场景转场；
- 非单位事件增益（BGM 0.7）与固定 Auto 等待政策；
- 类型化选择结果：选项把声明的值写入 `kept` 变量后再分支。

```sh
novelc check --locked
novelc test
novelc build --locked
```

两条剧情路线（sunrise / rest）对应 `tests/scenarios/`。素材与字体由 `scripts/make_p0_examples.py` 重建；验收映射见 `docs/NIR-NEXT-P0-BASELINE.md`。
"""
    )
    write_chars(out, "夜灯书页" + "".join(zh + en for zh, en in lines.values()))
    return out


# --- fixture B: replay-atlas ---------------------------------------------------

def build_replay_atlas():
    out = ROOT / "examples/replay-atlas"
    lines = {
        "intro": (
            "长廊尽头的画框还蒙着布。你在第一幅前停下脚步。",
            "The frames at the end of the corridor are still veiled. You stop before the first one.",
        ),
        "grant": (
            "布被揭开，北岸的芦苇在画里弯下腰。它进入了你的回想。",
            "The cloth lifts; reeds on the north shore bend inside the picture. It joins your memories.",
        ),
        "rn1": ("北岸的风把画里的芦苇吹弯了。", "The north wind bends the painted reeds."),
        "rn2": ("你记得那天，水鸟贴着浪飞。", "You remember seabirds skimming the waves that day."),
        "rs1": ("南径的岔路口，石阶还留着雨色。", "At the south fork, the stone steps still hold the rain."),
    }
    contracts = {
        k: {"source_revision": 1, "contract_revision": 1, "meaning_revision": 1, "gates": [], "params": {}}
        for k in lines
    }

    def docs(loc):
        i = 0 if loc == "zh-Hans" else 1
        return {
            k: {
                "source_revision": 1,
                "contract_revision": 1,
                "spans": [{"type": "text", "id": "body", "text": v[i], "emphasis": False}],
            }
            for k, v in lines.items()
        }

    scenes = {
        "hall": [node("background", "bg.hall", 0, 0, 1280, 720)],
        "north": [node("background", "bg.north", 0, 0, 1280, 720)],
        "south": [node("background", "bg.south", 0, 0, 1280, 720)],
        # The title screen is the image menu's own background; the scene graph
        # still needs the named scene to exist (same convention as imports).
        "title": [],
    }
    cues = {
        "open": {
            "effects": [
                effect("stage", "stage_present", scene="hall", duration_us="0"),
                effect("music", "audio", scope="session", asset="audio.bgm", bus="bgm", looped=True, gain=0.9),
            ]
        },
        "intro": {"effects": [effect("line", "dialogue", scope="interaction", text="intro", speaker="", reveal_us="36000")]},
        "grant": {"effects": [effect("line", "dialogue", scope="interaction", text="grant", speaker="", reveal_us="36000")]},
        "rn_open": {"effects": [effect("stage", "stage_present", scene="north", duration_us="600000")]},
        "rn1": {"effects": [effect("line", "dialogue", scope="interaction", text="rn1", speaker="", reveal_us="36000")]},
        "rn2": {"effects": [effect("line", "dialogue", scope="interaction", text="rn2", speaker="", reveal_us="36000")]},
        "rs_open": {"effects": [effect("stage", "stage_present", scene="south", duration_us="600000")]},
        "rs1": {"effects": [effect("line", "dialogue", scope="interaction", text="rs1", speaker="", reveal_us="36000")]},
    }
    main_blocks = {
        "start": block(activate("open", "intro_page")),
        "intro_page": block(activate("intro", "wait_intro")),
        "wait_intro": block(wait("line", "grant_page")),
        "grant_page": block(activate("grant", "wait_grant"), op("unlock.north", "profile_merge", key="atlas.north")),
        "wait_grant": block(wait("line", "done")),
        "done": block(end("completed")),
        **faults(),
    }
    north_blocks = {
        "start": block(activate("rn_open", "wait_open")),
        "wait_open": block(wait("stage", "page1")),
        "page1": block(activate("rn1", "wait1")),
        "wait1": block(wait("line", "page2")),
        "page2": block(activate("rn2", "wait2")),
        "wait2": block(wait("line", "done")),
        "done": block(end("replay_completed")),
        **faults(),
    }
    south_blocks = {
        "start": block(activate("rs_open", "wait_open")),
        "wait_open": block(wait("stage", "page1")),
        "page1": block(activate("rs1", "wait1")),
        "wait1": block(wait("line", "done")),
        "done": block(end("replay_completed")),
        **faults(),
    }
    fragment = {
        "fragment_format": 1,
        "variables": {},
        "functions": {
            "main": {"entry": "start", "blocks": main_blocks},
            "replay_north": {"entry": "start", "blocks": north_blocks},
            "replay_south": {"entry": "start", "blocks": south_blocks},
        },
        "scenes": scenes,
        "cues": cues,
        "choices": {},
    }
    dump(out / "content/ch01/story.nir.json", fragment)
    theme = '''format = 1
id = "theme.atlas"
base = "builtin.reader"
tokens = "tokens.json"
return_to_title = true

[slots]
"dialogue.main" = "builtin.dialogue"
"choice.main" = "builtin.choice"

[dialogue]
height = 240.0
padding = 24.0
font_size = 23.0

[choice]
width = 520.0
item_height = 58.0

[image_menus.title]
background = "bg.hall"
builtin_navigation = true
buttons = []
effects = { click = "audio.click" }

[[image_menus.title.elements]]
id = "begin"
rect = [520.0, 296.0, 240.0, 64.0]
content = { type = "button", label = "新的开始", asset = "ui.begin", hover_asset = "ui.begin.hover", action = { type = "new_game" } }

[[image_menus.title.elements]]
id = "gallery"
rect = [520.0, 386.0, 240.0, 64.0]
content = { type = "button", label = "回想图集", asset = "ui.gallery", hover_asset = "ui.gallery.hover", action = { type = "menu", menu = "gallery" } }

[image_menus.gallery]
background = "bg.gallery"
builtin_navigation = true
buttons = []
[image_menus.gallery.effects]
click = "audio.click"
[image_menus.gallery.effects.enter]
fade_us = "250000"
[image_menus.gallery.effects.enter.style]
type = "wipe"
direction = "left_to_right"
[image_menus.gallery.effects.close]
fade_us = "200000"
[image_menus.gallery.effects.music]
asset = "audio.gallery"
bus = "bgm"
[[image_menus.gallery.effects.elements]]
element = "north"
property = "opacity"
from = 0.0
duration_us = "400000"
[[image_menus.gallery.effects.elements]]
element = "south"
property = "opacity"
from = 0.0
delay_us = "120000"
duration_us = "400000"
[[image_menus.gallery.effects.elements]]
element = "back"
property = "offset_y"
from = 24.0
duration_us = "300000"

[[image_menus.gallery.elements]]
id = "north"
rect = [180.0, 180.0, 300.0, 200.0]
content = { type = "button", label = "北岸", asset = "ui.north", locked_asset = "ui.north.locked", action = { type = "replay", function = "replay_north" }, requires = "atlas.north" }

[[image_menus.gallery.elements]]
id = "south"
rect = [520.0, 180.0, 300.0, 200.0]
content = { type = "button", label = "南径", asset = "ui.south", locked_asset = "ui.south.locked", action = { type = "replay", function = "replay_south" }, requires = "atlas.south" }

[[image_menus.gallery.elements]]
id = "back"
rect = [900.0, 560.0, 200.0, 56.0]
content = { type = "button", label = "返回", asset = "ui.back", action = { type = "back" } }
'''
    player = '''format = 1

[defaults]
font_scale = 1.0
bgm_volume = 0.4
voice_volume = 0.9
sfx_volume = 0.6
reduced_motion = false
auto_delay_us = "1200000"
prefetch_content = true
prefetch_media = true
'''
    tokens = {
        "background": [0.05, 0.055, 0.07, 1],
        "panel": [0.09, 0.1, 0.12, 0.97],
        "accent": [0.62, 0.74, 0.68, 1],
        "text": [0.94, 0.95, 0.92, 1],
        "muted": [0.58, 0.62, 0.6, 1],
    }
    (out / "tests/scenarios/tour.toml").parent.mkdir(parents=True, exist_ok=True)
    (out / "tests/scenarios/tour.toml").write_text(
        '''format = 1
id = "tour"
entry = "main"
text_locale = "zh-Hans"
steps = []

[expect]
outcome = "completed"
'''
    )
    base_tree(
        out,
        "org.nir.replay-atlas",
        "replay-atlas",
        "回想图集 · Replay Atlas",
        "title",
        ["tests/scenarios/tour.toml"],
        theme,
        tokens,
        player,
        docs,
        lines,
        contracts,
        [],
    )
    credits(out, "背景、按钮与音效")
    # artwork: a veiled gallery wall, then two remembered places
    hall = Gradient((30, 28, 34), (18, 17, 22), 1280, 720)
    hall.rect((0, 560, 1280, 720), (46, 36, 30))
    for i, x in enumerate((140, 480, 820)):
        hall.rect((x, 150, x + 300, 420), (52, 50, 58))
        hall.rect((x + 14, 164, x + 286, 406), (24, 24, 30))
    png(out / "assets/source/hall.png", (1280, 720), hall)
    gallery = Gradient((34, 33, 40), (20, 20, 26), 1280, 720)
    gallery.rect((0, 580, 1280, 720), (58, 50, 40))
    gallery.glow(640, 130, 620, 180, (72, 84, 78))
    png(out / "assets/source/gallery.png", (1280, 720), gallery)
    north = Gradient((56, 74, 84), (150, 158, 148), 1280, 720)
    north.rect((0, 470, 1280, 720), (94, 118, 116))
    for x in range(40, 1240, 46):
        north.rect((x, 420 + (x % 90) // 3, x + 6, 500), (70, 104, 96))
    png(out / "assets/source/north.png", (1280, 720), north)
    south = Gradient((44, 52, 66), (96, 96, 100), 1280, 720)
    south.rect((0, 520, 1280, 720), (70, 68, 64))
    for i in range(6):
        south.rect((180 + i * 160, 560 - i * 8, 236 + i * 160, 720), (96, 94, 88))
    png(out / "assets/source/south.png", (1280, 720), south)
    plate(out / "assets/source/begin.png", 240, 64, (62, 58, 66), (150, 145, 132))
    plate(out / "assets/source/begin.hover.png", 240, 64, (62, 58, 66), (196, 190, 168), hover=True)
    plate(out / "assets/source/gallery.png", 240, 64, (62, 58, 66), (150, 145, 132))
    plate(out / "assets/source/gallery.hover.png", 240, 64, (62, 58, 66), (196, 190, 168), hover=True)
    plate(out / "assets/source/back.png", 200, 56, (58, 54, 60), (140, 135, 124))
    for name in ("north", "south"):
        src = {"north": ((72, 104, 96), (216, 208, 180)), "south": ((104, 96, 84), (226, 214, 186))}[name]
        art = Gradient(src[1], src[0], 300, 200)
        art.rect((0, 140, 300, 200), src[0])
        png(out / f"assets/source/{name}.thumb.png", (300, 200), art)
        darkened(out / f"assets/source/{name}.locked.png", 300, 200, src[0])
    tone(out / "assets/source/bgm.wav", 6.0, lambda t, s: sum(math.sin(2 * math.pi * f * t) * 0.018 for f in (196.0, 246.9, 293.7)) * math.sin(math.pi * t / s) ** 2)
    tone(out / "assets/source/click.wav", 0.12, lambda t, s: math.sin(2 * math.pi * 1568 * t) * 0.16 * math.exp(-t * 40) * min(1, t * 400))
    tone(out / "assets/source/page.wav", 4.0, lambda t, s: math.sin(2 * math.pi * 164.8 * t) * 0.02 * math.sin(math.pi * t / s) ** 2 + math.sin(2 * math.pi * 220 * t) * 0.012 * math.sin(math.pi * t / s) ** 2)
    assets = []
    for i, (id, kind, source, rights) in enumerate(
        [
            ("bg.hall", "image", "hall.png", "CC0-1.0"),
            ("bg.gallery", "image", "gallery.png", "CC0-1.0"),
            ("bg.north", "image", "north.png", "CC0-1.0"),
            ("bg.south", "image", "south.png", "CC0-1.0"),
            ("ui.begin", "image", "begin.png", "CC0-1.0"),
            ("ui.begin.hover", "image", "begin.hover.png", "CC0-1.0"),
            ("ui.gallery", "image", "gallery.png", "CC0-1.0"),
            ("ui.gallery.hover", "image", "gallery.hover.png", "CC0-1.0"),
            ("ui.back", "image", "back.png", "CC0-1.0"),
            ("ui.north", "image", "north.thumb.png", "CC0-1.0"),
            ("ui.north.locked", "image", "north.locked.png", "CC0-1.0"),
            ("ui.south", "image", "south.thumb.png", "CC0-1.0"),
            ("ui.south.locked", "image", "south.locked.png", "CC0-1.0"),
            ("audio.bgm", "audio", "bgm.wav", "CC0-1.0"),
            ("audio.click", "audio", "click.wav", "CC0-1.0"),
            ("audio.gallery", "audio", "page.wav", "CC0-1.0"),
        ]
    ):
        assets.append(f'[[assets]]\nid = "{id}"\nkind = "{kind}"\nsource = "source/{source}"\nrights = "{rights}"\n')
    assets.append(
        '''[[assets]]
id = "font.reader"
kind = "font"
source = "source/reader.otf"
rights = "OFL-1.1"
[assets.font]
mode = "subset"
face_index = 0
extra_characters = ""
license = "fonts/Noto-OFL.txt"
'''
    )
    (out / "assets/catalog.toml").write_text("format = 1\n\n" + "\n".join(assets))
    (out / "README.md").write_text(
        """# P0 验收样例：回想图集 · Replay Atlas

NIR-NEXT P0 验收样例之二（锁定回想菜单）。完整原创中性内容；覆盖：

- 标题图片菜单（自绘按钮/hover 图）与回想图集子菜单；
- 回想条目以 profile key 守卫：剧情授予 `atlas.north`，`atlas.south` 始终未授予（保持锁定态展示）；
- 回想入口函数以 `replay_completed` 结束并返回标题；锁定条目在播放器层拒绝执行（见仓库 Player 回归）；
- 菜单页效果：点击音、循环页面音乐、进入/关闭转场与进入边界逐元素动画。

```sh
novelc check --locked
novelc test
novelc build --locked
```

剧情路线 tour 对应 `tests/scenarios/`；回想入口由仓库集成测试直接驱动。素材与字体由 `scripts/make_p0_examples.py` 重建；验收映射见 `docs/NIR-NEXT-P0-BASELINE.md`。
"""
    )
    labels = "新的开始回想图集北岸南径返回"
    write_chars(out, "回想图集" + "".join(zh + en for zh, en in lines.values()) + labels)
    return out


# --- fixture C: voyage-log -----------------------------------------------------

def build_voyage_log():
    out = ROOT / "examples/voyage-log"
    lines = {
        "log1": (
            "暮色里船离了岸。你在掌灯前坐下，翻开这本空白的夜航日志。",
            "The boat leaves the shore at dusk. You sit before the lamp and open the blank logbook.",
        ),
        "log2": ("第一行先记风：东南，二级，浪不高。", "The first line records the wind: southeast, force two, low swells."),
        "log3": ("灯芯偶尔一跳，把你的影子钉在舱壁上。", "Now and then the filament jumps, pinning your shadow to the cabin wall."),
        "log4": ("过半程的时候，岸上的灯火只剩下三粒。", "Past the halfway mark, only three grains of shore light remain."),
        "log5": ("天将亮未亮。你合上日志，靠岸的方向已经有雾。", "The night is almost over. You close the log; mist already waits toward the shore."),
    }
    contracts = {
        k: {"source_revision": 1, "contract_revision": 1, "meaning_revision": 1, "gates": [], "params": {}}
        for k in lines
    }

    def docs(loc):
        i = 0 if loc == "zh-Hans" else 1
        return {
            k: {
                "source_revision": 1,
                "contract_revision": 1,
                "spans": [{"type": "text", "id": "body", "text": v[i], "emphasis": False}],
            }
            for k, v in lines.items()
        }

    scenes = {
        "deck": [node("background", "bg.deck", 0, 0, 1280, 720)],
        # The title screen is the image menu's own background; the scene graph
        # still needs the named scene to exist (same convention as imports).
        "title": [],
    }
    cues = {
        "open": {"effects": [effect("stage", "stage_present", scene="deck", duration_us="0")]},
        **{
            f"log{i}": {
                "effects": [
                    effect("line", "dialogue", scope="interaction", text=k, speaker="", reveal_us="36000")
                ]
            }
            for i, k in enumerate(lines, 1)
        },
    }
    blocks = {"start": block(activate("open", "page1"))}
    for i in range(1, len(lines) + 1):
        blocks[f"page{i}"] = block(activate(f"log{i}", f"wait{i}"))
        nxt = f"page{i + 1}" if i < len(lines) else "done"
        blocks[f"wait{i}"] = block(wait("line", nxt))
    blocks["done"] = block(end("completed"))
    blocks.update(faults())
    fragment = {
        "fragment_format": 1,
        "variables": {},
        "functions": {"main": {"entry": "start", "blocks": blocks}},
        "scenes": scenes,
        "cues": cues,
        "choices": {},
    }
    dump(out / "content/ch01/story.nir.json", fragment)
    theme = '''format = 1
id = "theme.log"
base = "builtin.reader"
tokens = "tokens.json"
return_to_title = true
menu_overlay = "system"

[slots]
"dialogue.main" = "builtin.dialogue"
"choice.main" = "builtin.choice"

[dialogue]
height = 240.0
padding = 24.0
font_size = 23.0

[choice]
width = 520.0
item_height = 58.0

[image_menus.title]
background = "bg.title"
builtin_navigation = true
buttons = []

[[image_menus.title.elements]]
id = "begin"
rect = [520.0, 280.0, 240.0, 64.0]
content = { type = "button", label = "开始航行", asset = "ui.begin", hover_asset = "ui.begin.hover", action = { type = "new_game" } }

[[image_menus.title.elements]]
id = "system"
rect = [520.0, 376.0, 240.0, 64.0]
content = { type = "button", label = "手账面板", asset = "ui.system", hover_asset = "ui.system.hover", action = { type = "push_menu", menu = "system" } }

[image_menus.system]
background = "ui.panel"
builtin_navigation = true
buttons = []

[image_menus.system.locals.tab]
type = "enum"
initial = "设置"
values = ["设置", "存档", "历史"]

[image_menus.system.locals.slot]
type = "int"
initial = 0
min = 0
max = 2

[image_menus.system.locals.offset]
type = "int"
initial = 0
min = 0
max = 999

[image_menus.system.locals.lamp]
type = "bool"
initial = true

[image_menus.system.locals.glow]
type = "int"
initial = 80
min = 0
max = 100

[[image_menus.system.elements]]
id = "tab-settings"
rect = [60.0, 60.0, 180.0, 56.0]
content = { type = "hit_region", label = "设置页签", action = { type = "set_local", local = "tab", value = "设置" } }

[[image_menus.system.elements]]
id = "tab-saves"
rect = [250.0, 60.0, 180.0, 56.0]
content = { type = "hit_region", label = "存档页签", action = { type = "set_local", local = "tab", value = "存档" } }

[[image_menus.system.elements]]
id = "tab-history"
rect = [440.0, 60.0, 180.0, 56.0]
content = { type = "hit_region", label = "历史页签", action = { type = "set_local", local = "tab", value = "历史" } }

[[image_menus.system.elements]]
id = "tab-caption"
rect = [900.0, 60.0, 320.0, 56.0]
text_local = "tab"
content = { type = "text", text = "设置", size = 34.0, color = [0.94, 0.95, 0.92, 1.0] }

[[image_menus.system.elements]]
id = "back"
rect = [990.0, 630.0, 230.0, 56.0]
content = { type = "button", label = "返回", asset = "ui.back", action = { type = "close" } }

[[image_menus.system.elements]]
id = "page-settings"
rect = [0.0, 140.0, 1280.0, 470.0]
visible_when = [{ type = "local", name = "tab", equals = "设置" }]
content = { type = "group" }

[[image_menus.system.elements]]
id = "rows"
parent = "page-settings"
rect = [60.0, 20.0, 900.0, 440.0]
content = { type = "stack", gap = 14.0 }

[[image_menus.system.elements]]
id = "row-speed"
parent = "rows"
rect = [0.0, 0.0, 900.0, 74.0]
content = { type = "group" }

[[image_menus.system.elements]]
id = "row-speed-label"
parent = "row-speed"
rect = [0.0, 12.0, 240.0, 48.0]
content = { type = "text", text = "字速", size = 30.0, color = [0.94, 0.95, 0.92, 1.0] }

[[image_menus.system.elements]]
id = "speed"
parent = "row-speed"
rect = [260.0, 8.0, 420.0, 56.0]
content = { type = "range", label = "字速滑条", binding = { type = "preference", field = "text_speed" }, min = 0.25, max = 4.0, step = 0.25 }

[[image_menus.system.elements]]
id = "speed-caption"
parent = "row-speed"
rect = [720.0, 12.0, 160.0, 48.0]
text_preference = "text_speed"
content = { type = "text", text = "1.00", size = 30.0, color = [0.94, 0.95, 0.92, 1.0] }

[[image_menus.system.elements]]
id = "row-bgm"
parent = "rows"
rect = [0.0, 0.0, 900.0, 74.0]
content = { type = "group" }

[[image_menus.system.elements]]
id = "row-bgm-label"
parent = "row-bgm"
rect = [0.0, 12.0, 240.0, 48.0]
content = { type = "text", text = "音乐音量", size = 30.0, color = [0.94, 0.95, 0.92, 1.0] }

[[image_menus.system.elements]]
id = "volume"
parent = "row-bgm"
rect = [260.0, 8.0, 420.0, 56.0]
content = { type = "range", label = "音乐音量滑条", binding = { type = "preference", field = "bgm_volume" }, min = 0.0, max = 1.0, step = 0.1 }

[[image_menus.system.elements]]
id = "volume-caption"
parent = "row-bgm"
rect = [720.0, 12.0, 160.0, 48.0]
text_preference = "bgm_volume"
content = { type = "text", text = "0.40", size = 30.0, color = [0.94, 0.95, 0.92, 1.0] }

[[image_menus.system.elements]]
id = "row-motion"
parent = "rows"
rect = [0.0, 0.0, 900.0, 74.0]
content = { type = "group" }

[[image_menus.system.elements]]
id = "row-motion-label"
parent = "row-motion"
rect = [0.0, 12.0, 240.0, 48.0]
content = { type = "text", text = "动画减弱", size = 30.0, color = [0.94, 0.95, 0.92, 1.0] }

[[image_menus.system.elements]]
id = "motion"
parent = "row-motion"
rect = [260.0, 8.0, 420.0, 56.0]
content = { type = "toggle", label = "动画减弱开关", binding = { type = "reduced_motion" } }

[[image_menus.system.elements]]
id = "row-lamp"
parent = "rows"
rect = [0.0, 0.0, 900.0, 74.0]
content = { type = "group" }

[[image_menus.system.elements]]
id = "row-lamp-label"
parent = "row-lamp"
rect = [0.0, 12.0, 240.0, 48.0]
content = { type = "text", text = "舱灯提示", size = 30.0, color = [0.94, 0.95, 0.92, 1.0] }

[[image_menus.system.elements]]
id = "lamp"
parent = "row-lamp"
rect = [260.0, 8.0, 420.0, 56.0]
content = { type = "toggle", label = "舱灯提示开关", binding = { type = "local", name = "lamp" } }

[[image_menus.system.elements]]
id = "row-glow"
parent = "rows"
rect = [0.0, 0.0, 900.0, 74.0]
content = { type = "group" }

[[image_menus.system.elements]]
id = "row-glow-label"
parent = "row-glow"
rect = [0.0, 12.0, 240.0, 48.0]
content = { type = "text", text = "面板亮度", size = 30.0, color = [0.94, 0.95, 0.92, 1.0] }

[[image_menus.system.elements]]
id = "glow"
parent = "row-glow"
rect = [260.0, 8.0, 420.0, 56.0]
content = { type = "range", label = "面板亮度滑条", binding = { type = "local", name = "glow" }, min = 0.0, max = 100.0, step = 10.0 }

[[image_menus.system.elements]]
id = "glow-caption"
parent = "row-glow"
rect = [720.0, 12.0, 160.0, 48.0]
text_local = "glow"
content = { type = "text", text = "80", size = 30.0, color = [0.94, 0.95, 0.92, 1.0] }

[[image_menus.system.elements]]
id = "page-saves"
rect = [0.0, 140.0, 1280.0, 470.0]
visible_when = [{ type = "local", name = "tab", equals = "存档" }]
content = { type = "group" }

[[image_menus.system.elements]]
id = "slot-caption"
parent = "page-saves"
rect = [60.0, 40.0, 600.0, 48.0]
text_slot = { type = "local", name = "slot" }
content = { type = "text", text = "0", size = 30.0, color = [0.94, 0.95, 0.92, 1.0] }

[[image_menus.system.elements]]
id = "slot-0"
parent = "page-saves"
rect = [60.0, 110.0, 180.0, 56.0]
content = { type = "hit_region", label = "选择槽位一", action = { type = "set_local", local = "slot", value = 0 } }

[[image_menus.system.elements]]
id = "slot-1"
parent = "page-saves"
rect = [260.0, 110.0, 180.0, 56.0]
content = { type = "hit_region", label = "选择槽位二", action = { type = "set_local", local = "slot", value = 1 } }

[[image_menus.system.elements]]
id = "slot-2"
parent = "page-saves"
rect = [460.0, 110.0, 180.0, 56.0]
content = { type = "hit_region", label = "选择槽位三", action = { type = "set_local", local = "slot", value = 2 } }

[[image_menus.system.elements]]
id = "save"
parent = "page-saves"
rect = [60.0, 220.0, 300.0, 64.0]
content = { type = "button", label = "保存到当前槽位", asset = "ui.save", action = { type = "save_slot", slot = { type = "local", name = "slot" } } }

[[image_menus.system.elements]]
id = "load"
parent = "page-saves"
rect = [400.0, 220.0, 300.0, 64.0]
content = { type = "button", label = "读取当前槽位", asset = "ui.load", action = { type = "load_slot", slot = { type = "local", name = "slot" } } }

[[image_menus.system.elements]]
id = "saves-note"
parent = "page-saves"
rect = [60.0, 320.0, 900.0, 48.0]
content = { type = "text", text = "页签与槽位都保存在手账里；离开页面即失效。", size = 24.0, color = [0.58, 0.62, 0.6, 1.0] }

[[image_menus.system.elements]]
id = "page-history"
rect = [0.0, 140.0, 1280.0, 470.0]
visible_when = [{ type = "local", name = "tab", equals = "历史" }]
content = { type = "group" }

[[image_menus.system.elements]]
id = "records"
parent = "page-history"
rect = [60.0, 20.0, 1160.0, 220.0]
content = { type = "history_window", offset_local = "offset", limit = 2, row_height = 92.0, size = 28.0, color = [0.94, 0.95, 0.92, 1.0] }

[[image_menus.system.elements]]
id = "older"
parent = "page-history"
rect = [60.0, 260.0, 300.0, 56.0]
content = { type = "button", label = "更早", asset = "ui.older", action = { type = "history_page", window = "records", delta = 1 } }

[[image_menus.system.elements]]
id = "newer"
parent = "page-history"
rect = [400.0, 260.0, 300.0, 56.0]
content = { type = "button", label = "更近", asset = "ui.newer", action = { type = "history_page", window = "records", delta = -1 } }
'''
    player = '''format = 1

[defaults]
font_scale = 1.0
bgm_volume = 0.4
voice_volume = 0.9
sfx_volume = 0.6
reduced_motion = false
prefetch_content = true
prefetch_media = true
'''
    tokens = {
        "background": [0.05, 0.055, 0.08, 1],
        "panel": [0.1, 0.11, 0.14, 0.97],
        "accent": [0.56, 0.66, 0.8, 1],
        "text": [0.94, 0.95, 0.92, 1],
        "muted": [0.58, 0.62, 0.6, 1],
    }
    (out / "tests/scenarios/tour.toml").parent.mkdir(parents=True, exist_ok=True)
    (out / "tests/scenarios/tour.toml").write_text(
        '''format = 1
id = "tour"
entry = "main"
text_locale = "zh-Hans"
steps = []

[expect]
outcome = "completed"
'''
    )
    base_tree(
        out,
        "org.nir.voyage-log",
        "voyage-log",
        "夜航日志 · Voyage Log",
        "title",
        ["tests/scenarios/tour.toml"],
        theme,
        tokens,
        player,
        docs,
        lines,
        contracts,
        [],
    )
    credits(out, "背景与按钮")
    # artwork: a night crossing under a low moon, then the cabin panel
    title = Gradient((14, 20, 34), (30, 38, 56), 1280, 720)
    title.glow(1010, 170, 150, 130, (214, 216, 210))
    title.glow(220, 300, 320, 240, (52, 66, 84))
    title.rect((0, 560, 1280, 720), (18, 26, 40))
    for i in range(9):
        title.rect((60 + i * 130, 590 + (i % 3) * 12, 96, 4), (44, 58, 76))
    title.rect((760, 120, 1060, 400), (10, 14, 24))
    png(out / "assets/source/title.png", (1280, 720), title)
    deck = Gradient((22, 26, 34), (14, 16, 22), 1280, 720)
    deck.glow(250, 250, 300, 260, (188, 148, 92))
    deck.rect((0, 620, 1280, 720), (40, 34, 28))
    deck.rect((120, 540, 420, 600), (54, 44, 36))
    png(out / "assets/source/deck.png", (1280, 720), deck)
    panel = Gradient((22, 26, 34), (16, 18, 24), 1280, 720)
    panel.rect((0, 0, 1280, 5), (70, 84, 100, 255))
    panel.rect((0, 715, 1280, 720), (70, 84, 100, 255))
    png(out / "assets/source/panel.png", (1280, 720), panel)
    plate(out / "assets/source/begin.png", 240, 64, (48, 56, 70), (140, 158, 178))
    plate(out / "assets/source/begin.hover.png", 240, 64, (48, 56, 70), (188, 204, 218), hover=True)
    plate(out / "assets/source/system.png", 240, 64, (48, 56, 70), (140, 158, 178))
    plate(out / "assets/source/system.hover.png", 240, 64, (48, 56, 70), (188, 204, 218), hover=True)
    plate(out / "assets/source/back.png", 230, 56, (44, 50, 62), (128, 144, 162))
    plate(out / "assets/source/save.png", 300, 64, (52, 62, 60), (128, 168, 150))
    plate(out / "assets/source/load.png", 300, 64, (60, 56, 72), (150, 140, 178))
    plate(out / "assets/source/older.png", 300, 56, (46, 52, 60), (128, 144, 162))
    plate(out / "assets/source/newer.png", 300, 56, (46, 52, 60), (128, 144, 162))
    assets = []
    for i, (id, kind, source, rights) in enumerate(
        [
            ("bg.title", "image", "title.png", "CC0-1.0"),
            ("bg.deck", "image", "deck.png", "CC0-1.0"),
            ("ui.panel", "image", "panel.png", "CC0-1.0"),
            ("ui.begin", "image", "begin.png", "CC0-1.0"),
            ("ui.begin.hover", "image", "begin.hover.png", "CC0-1.0"),
            ("ui.system", "image", "system.png", "CC0-1.0"),
            ("ui.system.hover", "image", "system.hover.png", "CC0-1.0"),
            ("ui.back", "image", "back.png", "CC0-1.0"),
            ("ui.save", "image", "save.png", "CC0-1.0"),
            ("ui.load", "image", "load.png", "CC0-1.0"),
            ("ui.older", "image", "older.png", "CC0-1.0"),
            ("ui.newer", "image", "newer.png", "CC0-1.0"),
        ]
    ):
        assets.append(f'[[assets]]\nid = "{id}"\nkind = "{kind}"\nsource = "source/{source}"\nrights = "{rights}"\n')
    assets.append(
        '''[[assets]]
id = "font.reader"
kind = "font"
source = "source/reader.otf"
rights = "OFL-1.1"
[assets.font]
mode = "subset"
face_index = 0
extra_characters = ""
license = "fonts/Noto-OFL.txt"
'''
    )
    (out / "assets/catalog.toml").write_text("format = 1\n\n" + "\n".join(assets))
    (out / "README.md").write_text(
        """# P0 验收样例：夜航日志 · Voyage Log

NIR-NEXT P0 验收样例之三（页签设置/存档/历史页面）。完整原创中性内容；覆盖：

- 单页三页签（设置/存档/历史）的局部状态页：enum/int/bool 局部值与 `set_local` 切换，
  `visible_when` 条件页，`text_local`/`text_preference`/`text_slot` 三种动态文字；
- 值控件：偏好滑条（字速/音乐音量）、`reduced_motion` 开关、局部 bool 开关与局部有界
  int 滑条（面板亮度），内置绘制，无图片依赖；
- 设置行以 `stack` 容器紧凑纵向排列（`ui.menu-stack.v1`）；
- 存档槽：局部槽位选择 + `save_slot`/`load_slot`（局部槽位解析，3 槽上限）；
- 历史页：`history_window` 分页窗口 + `history_page` 更早/更近；
- 标题页 `push_menu` 进入面板（`ui.menu-navigation.v1`），`menu_overlay` 使剧情内
  菜单键直达同一面板（`ui.menu-services.v1`）。

```sh
novelc check --locked
novelc test
novelc build --locked
```

剧情路线 tour 对应 `tests/scenarios/`；player 级驱动（页签、值提交、历史分页）见仓库
集成测试。素材与字体由 `scripts/make_p0_examples.py` 重建；验收映射见 `docs/NIR-NEXT-P0-BASELINE.md`。
"""
    )
    labels = "开始航行手账面板设置存档历史页签返回字速音乐音量滑条动画减弱开关舱灯提示面板亮度选择槽位一二三保存到当前读取更早更近与都保存在手账里离开页面即失效"
    write_chars(out, "夜航日志" + "".join(zh + en for zh, en in lines.values()) + labels)
    return out


if __name__ == "__main__":
    import sys

    fixture = sys.argv[1] if len(sys.argv) > 1 else "all"
    if fixture in ("all", "reading-lamp"):
        build_reading_lamp()
    if fixture in ("all", "replay-atlas"):
        build_replay_atlas()
    if fixture in ("all", "voyage-log"):
        build_voyage_log()
    print("regenerated:", fixture)
