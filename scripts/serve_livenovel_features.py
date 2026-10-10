#!/usr/bin/env python3
"""Build a neutral fixture for capabilities used by the LiveNovel adapter."""
import json
import os
import shutil
import struct
import subprocess
import zlib
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CLI = Path(os.environ.get("NIR_TEST_COMPILER", ROOT / "dist/novelc"))
SDK = Path(os.environ.get("NIR_TEST_SDK", ROOT / "dist/sdk"))
PROJECT = Path(os.environ.get("NIR_TEST_PROJECT", ROOT / "reports/livenovel-features/project"))
shutil.copytree(ROOT / "examples/rain-letters", PROJECT,
                ignore=shutil.ignore_patterns("dist", ".nir", "reports"), dirs_exist_ok=True)


def write(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")


def run(*args):
    subprocess.run([str(CLI), "--sdk", str(SDK), "-p", str(PROJECT), *args], check=True)


def png(color, width=32, height=24, row=None):
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xffffffff)
    rows = (b"\0" + (row if row is not None else bytes(color) * width)) * height
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b"")


for name, color in [("icon", [255, 0, 255, 255]), ("hover", [0, 255, 255, 255]), ("inline", [255, 0, 255, 255])]:
    (PROJECT / f"assets/source/{name}.png").write_bytes(png(color))
    with (PROJECT / "assets/catalog.toml").open("a") as out:
        out.write(f'\n[[assets]]\nid = "fixture.{name}"\nkind = "image"\nsource = "source/{name}.png"\nrights = "CC0-1.0"\nexpected_size = [32, 24]\n')

digit_colors = [[40, 40, 255, 255], [255, 40, 40, 255], [40, 255, 40, 255]] + [[80, 80, 80, 255]] * 7
atlas_row = b"".join(bytes(color) * 16 for color in digit_colors)
(PROJECT / "assets/source/digits.png").write_bytes(png(None, 160, 24, atlas_row))
with (PROJECT / "assets/catalog.toml").open("a") as out:
    out.write('\n[[assets]]\nid = "fixture.digits"\nkind = "image"\nsource = "source/digits.png"\nrights = "CC0-1.0"\nexpected_size = [160, 24]\n')

image = {"asset": "fixture.inline", "width": 32, "height": 24, "align": "center", "margins": [2, 3, 0, 0]}
spans = [{"type": "ruby", "id": "annotated", "text": "Reader", "reading": "reading"},
         {"type": "image", "id": "icon", "image": image},
         {"type": "pause", "id": "pause"},
         {"type": "text", "id": "after", "text": " continues."}]
texts = PROJECT / "content/ch01/texts"
contract_path = texts / "contracts.json"
contracts = json.loads(contract_path.read_text())
contracts["intro"]["images"] = [{"id": "icon", "asset": "fixture.inline"}]
contracts["intro"]["pauses"] = [{"id": "pause", "timeout_us": None}]
write(contract_path, contracts)
for locale in ["zh-Hans", "en"]:
    path = texts / f"{locale}.json"
    docs = json.loads(path.read_text())
    docs["intro"]["spans"] = spans
    write(path, docs)
run("text", "update", "--id", "intro", "--meaning", "bump")
run("text", "review", "--id", "intro", "--locale", "en")

story_path = PROJECT / "content/ch01/story.nir.json"
story = json.loads(story_path.read_text())
story["scenes"]["station"] = [{"id": "black", "x": 0, "y": 0, "width": 1280, "height": 720, "color": [0, 0, 0, 1]}]
for name in ["icon", "hover"]:
    story["scenes"]["station"].append({"id": f"preload-{name}", "asset": f"fixture.{name}", "opacity": 0,
                                      "x": 0, "y": 0, "width": 32, "height": 24})
story["cues"]["intro"]["effects"][0]["effect"]["reveal_us"] = "0"
story["cues"]["arrival"]["effects"] = [{"id": "line", "scope": "interaction", "effect": {
    "type": "dialogue", "text": "arrival", "speaker": "", "reveal_us": "0"}}]
story["choices"]["route"]["options"][0]["image"] = {"asset": "fixture.icon", "hover_asset": "fixture.hover", "rect": [120, 120, 200, 150]}
story["choices"]["route"]["options"][1]["image"] = {"asset": "fixture.icon", "hover_asset": "fixture.hover", "rect": [620, 120, 200, 150]}


def op(name, **fields):
    return {"id": name, "operation": fields}


def block(terminator, ops=None):
    return {"ops": ops or [], "terminator": terminator}


def wait(next_block):
    return {"type": "await", "conditions": [{"task": "line", "milestone": {"type": "finished"}}],
            "next": next_block, "on_cancelled": "cancelled", "on_failed": "failed"}


story["variables"]["profile_flag"] = {"type":"i32", "value":0}
story["variables"]["profile_value"] = {"type":"i32", "value":0}
story["variables"]["hud_number"] = {"type":"i32", "value":12}
story["variables"]["hud_capture"] = {"type":"i32", "value":0}
story["scenes"]["title"] = json.loads(json.dumps(story["scenes"]["station"]))
manifest_path = PROJECT / "game.toml"
manifest_path.write_text(manifest_path.read_text().replace('title_scene = "station"', 'title_scene = "title"'))
story["scenes"]["station"].append({"id":"digit-hud","asset":"fixture.digits","x":100,"y":100,"width":0,"height":0,"order":200,
    "bitmap_text":{"slot":"hud_capture","alphabet":"0123456789","cell":[16,24],"line_spacing":0,"align":"start","x_anchor":"start","y_anchor":"start"}})
story["scenes"]["station"].append({"id":"moving-picture","asset":"fixture.hover","x":400,"y":100,"width":32,"height":24,"order":200})
story["scenes"]["overlaid"] = json.loads(json.dumps(story["scenes"]["station"]))
story["scenes"]["overlaid"][-1]["preserve_pose"] = ["y"]
story["scenes"]["overlaid"][-1]["y"] = 500
story["cues"]["motion"] = {"effects":[
    {"id":"motion","scope":"session","effect":{"type":"clip","node":"moving-picture","property":"y","to":500,"duration_us":"30000000"}},
    {"id":"phase","scope":"frame","effect":{"type":"delay","duration_us":"100000"}}]}
story["cues"]["overlay"] = {"effects":[{"id":"overlay","scope":"session","effect":{"type":"stage_present","scene":"overlaid","duration_us":"10000000"}}]}
story["functions"] = {"main": {"entry": "start", "blocks": {
    "start": block({"type": "activate", "cue": "opening", "next": "intro"}),
    "intro": block({"type": "activate", "cue": "intro", "next": "wait_intro"}, [op("initial-pause", type="audio_pause", bus="bgm", paused=True), op("read-profile", type="profile_read", target="profile_flag", key="fixture.finished"), op("read-value", type="profile_value_read", target="profile_value", key="fixture.mutable")]),
    "wait_intro": block(wait("guarded")),
    "guarded": block({"type": "activate", "cue": "arrival", "next": "wait_guarded"}, [
        op("menu-off", type="menu_access", enabled=False),
        op("pause-bgm", type="audio_pause", bus="bgm", paused=True)]),
    "wait_guarded": block(wait("choose")),
    "choose": block({"type": "interact", "choice": "route", "branches": {"walk": "walk", "stay": "stay"}, "on_empty": "failed"}, [
        op("menu-on", type="menu_access", enabled=True), op("read-profile-again", type="profile_read", target="profile_flag", key="fixture.finished"),
        op("resume-bgm", type="audio_pause", bus="bgm", paused=False),
        op("read-value-again", type="profile_value_read", target="profile_value", key="fixture.mutable")]),
    "walk": block({"type": "end", "outcome": "walk"}, [op("finish-profile", type="profile_merge", key="fixture.finished"), op("value-on", type="profile_value_assign", target="profile_value", key="fixture.mutable", value={"type":"const","value":{"type":"i32","value":1}})]),
    "stay": block({"type": "end", "outcome": "stay"}, [op("value-off", type="profile_value_assign", target="profile_value", key="fixture.mutable", value={"type":"const","value":{"type":"i32","value":0}})]),
    "cancelled": block({"type": "end", "outcome": "cancelled"}),
    "failed": block({"type": "fault", "code": "E_FIXTURE", "message": "Neutral fixture failed"})}}}
# One ulp above 1 in x87 precision is invisible in an f64 round trip.
story["variables"]["extended"] = {"type":"f80", "value":"3fff8000000000000001"}
story["variables"]["extended_delta"] = {"type":"f80", "value":"00000000000000000000"}
story["functions"]["main"]["blocks"]["intro"]["ops"].append(op("extended-subtract", type="assign", target="extended_delta", value={
    "type":"binary", "op":"sub", "left":{"type":"var", "name":"extended"},
    "right":{"type":"to_f80", "value":{"type":"const", "value":{"type":"i32", "value":1}}}}))
# Force real code-package transitions after starting session music. The reader
# owns no declarations: its cues, text and task handles stay in ch01.
reader = story["functions"]["main"]
reader["entry"] = "intro"
reader["blocks"]["intro"]["ops"].append(op("change-hud-source", type="assign", target="hud_number", value={"type":"const","value":{"type":"i32","value":34}}))
del reader["blocks"]["start"]
story["functions"] = {
    "main": {"entry": "start", "blocks": {
        "start": block({"type": "activate", "cue": "opening", "next": "motion"}, [op("capture-hud",type="assign",target="hud_capture",value={"type":"var","name":"hud_number"})]),
        "motion": block({"type":"activate","cue":"motion","next":"phase"}),
        "phase": block({"type":"await","conditions":[{"task":"phase","milestone":{"type":"finished"}}],"next":"overlay","on_cancelled":"done","on_failed":"done"}),
        "overlay": block({"type":"activate","cue":"overlay","next":"read"}),
        "read": block({"type": "call", "function": "reader", "args": {}, "next": "done"}),
        "done": block({"type": "end", "outcome": "returned"})}},
    "reader": reader}
story["variables"]["padding"] = {"type": "string", "value": ""}
for index in range(24):
    story["functions"][f"padding{index:02}"] = {"entry": "body", "blocks": {
        "body": block({"type": "return"}, [op(f"padding{index}_{at}", type="assign", target="padding",
            value={"type": "const", "value": {"type": "string", "value": "x" * 1024}}) for at in range(100)])}}
if os.environ.get("NIR_TEST_QUAKE") == "1":
    # A separate neutral run gives the test enough time to freeze a visible
    # offset. Commercial imports retain their authored finite duration.
    reader["blocks"]["intro"]["ops"] = [o for o in reader["blocks"]["intro"]["ops"] if o["id"] != "initial-pause"]
    reader["blocks"]["intro"]["terminator"]["next"] = "quake"
    reader["blocks"]["quake"] = block({"type":"activate","cue":"quake","next":"quake_wait"})
    reader["blocks"]["quake_wait"] = block({"type":"await","conditions":[{"task":"quake","milestone":{"type":"finished"}}],
        "next":"wait_intro","on_cancelled":"cancelled","on_failed":"failed"})
    story["cues"]["quake"] = {"effects":[{"id":"quake","scope":"session","effect":{"type":"dialogue_shake","spec":{
        "amplitude":[60,40],"step_us":"250000","duration_us":"10000000","randomize":True}}}]}
if os.environ.get("NIR_TEST_SPRITE_WAVE") == "1":
    story["cues"]["overlay"]["effects"][0]["effect"]["duration_us"] = "0"
    story["scenes"]["overlaid"].append({"id":"wave-marker","asset":"fixture.icon",
        "x":700,"y":100,"width":32,"height":24,"order":201})
    reader["blocks"]["intro"]["ops"] = [o for o in reader["blocks"]["intro"]["ops"] if o["id"] != "initial-pause"]
    reader["blocks"]["intro"]["terminator"]["next"] = "sprite_wave"
    reader["blocks"]["sprite_wave"] = block({"type":"activate","cue":"sprite_wave","next":"sprite_wave_wait"})
    reader["blocks"]["sprite_wave_wait"] = block({"type":"await","conditions":[{"task":"sprite_wave","milestone":{"type":"finished"}}],
        "next":"wait_intro","on_cancelled":"cancelled","on_failed":"failed"})
    story["cues"]["sprite_wave"] = {"effects":[{"id":"sprite_wave","scope":"scene","effect":{
        "type":"sprite_wave","nodes":["moving-picture","wave-marker"],"spec":{
            "amplitude":[60,40],"step_us":"250000","duration_us":"10000000"}}}]}
if os.environ.get("NIR_TEST_SPRITE_SHAKE") == "1" or os.environ.get("NIR_TEST_SPRITE_QUAKE") == "1":
    story["cues"]["overlay"]["effects"][0]["effect"]["duration_us"] = "0"
    for name, x in [("shake-left", 700), ("shake-right", 950)]:
        story["scenes"]["overlaid"].append({"id":name,"asset":"fixture.icon",
            "x":x,"y":100,"width":32,"height":24,"order":201})
    reader["blocks"]["intro"]["ops"] = [o for o in reader["blocks"]["intro"]["ops"] if o["id"] != "initial-pause"]
    reader["blocks"]["intro"]["terminator"]["next"] = "sprite_shake"
    reader["blocks"]["sprite_shake"] = block({"type":"activate","cue":"sprite_shake","next":"sprite_shake_wait"})
    reader["blocks"]["sprite_shake_wait"] = block({"type":"await","conditions":[{"task":"sprite_shake","milestone":{"type":"finished"}}],
        "next":"wait_intro","on_cancelled":"cancelled","on_failed":"failed"})
    story["cues"]["sprite_shake"] = {"effects":[{"id":"sprite_shake","scope":"scene","effect":{
        "type":"sprite_shake","mode":"quake" if os.environ.get("NIR_TEST_SPRITE_QUAKE") == "1" else "bound","nodes":["shake-left","shake-right"],"spec":{
            "amplitude":[60,40],"step_us":"250000","duration_us":"10000000","randomize":True}}}]}
if os.environ.get("NIR_TEST_STOCK_MOTION") == "1":
    story["cues"]["overlay"]["effects"][0]["effect"]["duration_us"] = "0"
    reader["blocks"]["intro"]["ops"] = [o for o in reader["blocks"]["intro"]["ops"] if o["id"] != "initial-pause"]
    for curve,x in [("inc",700),("dec",950)]:
        node={"id":"source-"+curve,"asset":"fixture.icon","x":x,"y":100,"width":32,"height":24,"order":201}
        story["scenes"]["station"].append(node)
        later=dict(node,x=x+50,preserve_pose=["x"])
        story["scenes"]["overlaid"].append(later)
        story["cues"]["motion"]["effects"].append({"id":"source_"+curve,"scope":"session","effect":{
            "type":"source_motion","node":"source-"+curve,"property":"x","to":x+50,
            "duration_us":"10000000","curve":curve}})
if os.environ.get("NIR_TEST_SOURCE_OPACITY") == "1":
    story["cues"]["overlay"]["effects"][0]["effect"]["duration_us"]="0"
    reader["blocks"]["intro"]["ops"]=[o for o in reader["blocks"]["intro"]["ops"] if o["id"]!="initial-pause"]
    node={"id":"opacity-picture","asset":"fixture.icon","x":700,"y":100,"width":32,"height":24,"order":201}
    story["scenes"]["station"].append(node)
    story["scenes"]["overlaid"].append(dict(node,opacity=0.4,preserve_pose=["opacity"]))
    story["cues"]["motion"]["effects"].append({"id":"source_opacity","scope":"session","effect":{
        "type":"source_motion","node":"opacity-picture","property":"opacity","to":64,
        "duration_us":"8000000","curve":"opacity_linear"}})
if os.environ.get("NIR_TEST_WINDOW_FLIP") == "1":
    story["cues"]["overlay"]["effects"][0]["effect"]["duration_us"] = "0"
    reader["blocks"]["intro"]["ops"] = [o for o in reader["blocks"]["intro"]["ops"] if o["id"] != "initial-pause"]
    reader["blocks"]["intro"]["terminator"]["next"] = "before_flip"
    reader["blocks"]["before_flip"] = block({"type":"activate","cue":"before_flip","next":"before_flip_wait"})
    reader["blocks"]["before_flip_wait"] = block({"type":"await","conditions":[{"task":"before_flip","milestone":{"type":"finished"}}],
        "next":"window_flip","on_cancelled":"cancelled","on_failed":"failed"})
    reader["blocks"]["window_flip"] = block({"type":"activate","cue":"window_flip","next":"window_flip_wait"})
    reader["blocks"]["window_flip_wait"] = block({"type":"await","conditions":[{"task":"window_flip","milestone":{"type":"finished"}}],
        "next":"wait_intro","on_cancelled":"cancelled","on_failed":"failed"})
    story["cues"]["before_flip"] = {"effects":[{"id":"before_flip","scope":"frame","effect":{"type":"delay","duration_us":"2000000"}}]}
    story["cues"]["window_flip"] = {"effects":[{"id":"window_flip","scope":"session","effect":{
        "type":"stage_present","scene":"overlaid","dialogue_visible":False,"duration_us":"4000000"}}]}
if os.environ.get("NIR_TEST_SPRITE_TRANSFORM") == "1":
    (PROJECT / "assets/source/rotate.png").write_bytes(png(None,32,24,bytes([255,0,255,255])*16+bytes([0,255,255,255])*16))
    with (PROJECT / "assets/catalog.toml").open("a") as out:
        out.write('\n[[assets]]\nid = "fixture.rotate"\nkind = "image"\nsource = "source/rotate.png"\nrights = "CC0-1.0"\nexpected_size = [32,24]\n')
    story["cues"]["overlay"]["effects"][0]["effect"]["duration_us"] = "0"
    story["scenes"]["overlaid"].extend([
        {"id":"rotation-group","x":700,"y":100,"width":0,"height":0,"order":300,"clip":[10,0,28,48]},
        {"id":"rotation-marker","parent":"rotation-group","asset":"fixture.rotate","x":0,"y":0,"width":32,"height":24,
            "sprite_transform":{"origin":[48,0],"basis_x":[0,1.5],"basis_y":[-2,0]}}
    ])
    reader["blocks"]["intro"]["ops"] = [o for o in reader["blocks"]["intro"]["ops"] if o["id"] != "initial-pause"]
if any(os.environ.get(flag) == "1" for flag in ["NIR_TEST_SPRITE_TIMELINE", "NIR_TEST_ADVANCE_WAIT", "NIR_TEST_SPRITE_LIFECYCLE"]):
    story["cues"]["overlay"]["effects"][0]["effect"]["duration_us"] = "0"
    root = {"id":"movie-root","x":700,"y":100,"width":0,"height":0,"order":300,
            "timeline_binding":"movie.frames"}
    marker = {"id":"movie-picture","parent":"movie-root","asset":"fixture.icon",
              "x":0,"y":0,"width":32,"height":24}
    story["scenes"]["overlaid"].extend([root,marker])
    story["scenes"]["movie.next"] = json.loads(json.dumps(story["scenes"]["overlaid"]))
    story["sprite_timelines"] = {"movie.frames": {"id":"movie.frames","duration_us":"5000000","tracks":[
        {"node":"movie-picture","frames":[
            {"at_us":"0","rect":[0,0,32,24],"opacity":1},
            {"at_us":"1000000","rect":[60,0,32,24],"opacity":1,"color":[0,1,1,1]},
            {"at_us":"3000000","rect":[120,0,32,24],"opacity":1}
        ]}]}}
    reader["blocks"]["intro"]["ops"] = [o for o in reader["blocks"]["intro"]["ops"] if o["id"] != "initial-pause"]
    reader["blocks"]["intro"]["terminator"]["next"] = "movie.start"
    reader["blocks"]["movie.start"] = block({"type":"activate","cue":"movie.start","next":"movie.delay"})
    reader["blocks"]["movie.delay"] = block({"type":"await","conditions":[{"task":"movie.delay","milestone":{"type":"finished"}}],
        "next":"movie.next","on_cancelled":"cancelled","on_failed":"failed"})
    reader["blocks"]["movie.next"] = block({"type":"activate","cue":"movie.next","next":"movie.wait"})
    reader["blocks"]["movie.wait"] = block({"type":"await","conditions":[{"task":"movie","milestone":{"type":"finished"}}],
        "next":"wait_intro","on_cancelled":"cancelled","on_failed":"failed"})
    story["cues"]["movie.start"] = {"effects":[
        {"id":"movie","scope":"session","effect":{"type":"sprite_timeline","timeline":"movie.frames","root":"movie-root","duration_us":"5000000"}},
        {"id":"movie.delay","scope":"frame","effect":{"type":"delay","duration_us":"2000000"}}]}
    story["cues"]["movie.next"] = {"effects":[{"id":"movie.stage","scope":"session","effect":{
        "type":"stage_present","scene":"movie.next","duration_us":"1000000"}}]}
    if os.environ.get("NIR_TEST_SPRITE_LIFECYCLE") == "1":
        story["cues"]["movie.start"]["effects"][0]["effect"]["delete_on_finish"] = True
        next(node for node in story["scenes"]["movie.next"] if node["id"] == "movie-root")["inherit_existence"] = True
        story["cues"]["movie.after"] = {"effects":[{"id":"movie.stage","scope":"session","effect":{
            "type":"stage_present","scene":"movie.next","duration_us":"0"}}]}
        reader["blocks"]["movie.wait"]["terminator"]["next"] = "movie.after"
        reader["blocks"]["movie.after"] = block({"type":"activate","cue":"movie.after","next":"wait_intro"})
    if os.environ.get("NIR_TEST_ADVANCE_WAIT") == "1":
        story["sprite_timelines"]["movie.frames"]["duration_us"]="20000000"
        story["cues"]["movie.start"]["effects"][0]["effect"]["duration_us"]="20000000"
        reader["blocks"]["movie.start"]["terminator"]["next"]="movie.wait"
        reader["blocks"]["movie.wait"]["terminator"]["on_advance"]="wait_intro"
if os.environ.get("NIR_TEST_PREVIEW_FILTERS") == "1":
    story["cues"]["overlay"]["effects"][0]["effect"]["duration_us"] = "0"
    story["variables"]["filter_flag"] = {"type":"i32","value":1}
    truth = {"type":"binary","op":"eq","left":{"type":"var","name":"filter_flag"},
             "right":{"type":"const","value":{"type":"i32","value":1}}}
    inverse = {"type":"not","value":truth}
    (PROJECT / "assets/source/disabled.png").write_bytes(png([255,255,0,255]))
    with (PROJECT / "assets/catalog.toml").open("a") as out:
        out.write('\n[[assets]]\nid = "fixture.disabled"\nkind = "image"\nsource = "source/disabled.png"\nrights = "CC0-1.0"\nexpected_size = [32,24]\n')
    options = story["choices"]["route"]["options"]
    for option in options:
        option["image"]["disabled_asset"] = "fixture.disabled"
    options[1]["enabled"] = inverse
    hidden = json.loads(json.dumps(options[0]));hidden["id"]="hidden";hidden["visible"]=inverse
    hidden["image"]["rect"]=[350,320,200,100];options.append(hidden)
    reader["blocks"]["choose"]["terminator"]["branches"]["hidden"] = "stay"
    captures=[]
    for index,option in enumerate(options):
        for key in ["visible","enabled"]:
            name=f"filter_capture_{index}_{key}"
            predicate=option.get(key,{"type":"const","value":{"type":"bool","value":True}})
            story["variables"][name]={"type":"bool","value":False}
            captures.append(op(name,type="assign",target=name,value=predicate))
            option[key]={"type":"var","name":name}
    nodes = json.loads(json.dumps(story["scenes"]["overlaid"]))
    targets = []
    for index, option in enumerate(options):
        visible = option.get("visible",{"type":"const","value":{"type":"bool","value":True}})
        enabled = option.get("enabled",{"type":"const","value":{"type":"bool","value":True}})
        for kind, asset, predicate in [
            ("normal",option["image"]["asset"],{"type":"binary","op":"and","left":visible,"right":enabled}),
            ("disabled","fixture.disabled",{"type":"binary","op":"and","left":visible,"right":{"type":"not","value":enabled}}),
            ("hover",option["image"]["hover_asset"],None),
        ]:
            node=f"filter.{index}.{kind}";x,y,w,h=option["image"]["rect"]
            nodes.append({"id":node,"asset":asset,"x":x,"y":y,"width":w,"height":h,"opacity":0,"order":1000+index})
            if predicate is not None:targets.append((node,predicate))
    story["scenes"]["filters"] = nodes
    for name,duration in [("filters.prepare","0"),("filters.present","5000000")]:
        story["cues"][name] = {"effects":[{"id":"stage","scope":"session","effect":{
            "type":"stage_present","scene":"filters","duration_us":duration}}]}
    reader["entry"] = "filters.capture"
    reader["blocks"]["filters.capture"] = block({"type":"goto","target":"filters.prepare"},captures)
    reader["blocks"]["filters.prepare"] = block({"type":"activate","cue":"filters.prepare","next":"filter0"})
    for index,(node,predicate) in enumerate(targets):
        following=f"filter{index+1}" if index+1<len(targets) else "filters.present"
        reader["blocks"][f"filter{index}"] = block({"type":"branch","condition":predicate,"yes":f"show{index}","no":following})
        reader["blocks"][f"show{index}"] = block({"type":"goto","target":following},[
            op(f"show{index}",type="draft_patch",node=node,property="opacity",value=1)])
    reader["blocks"]["filters.present"] = block({"type":"activate","cue":"filters.present","next":"filters.wait"},[
        op("mutate-source-after-capture",type="assign",target="filter_flag",value={"type":"const","value":{"type":"i32","value":0}})])
    reader["blocks"]["filters.wait"] = block({"type":"await","conditions":[{"task":"stage","milestone":{"type":"finished"}}],
        "next":"filters.choice","on_cancelled":"cancelled","on_failed":"failed"})
    reader["blocks"]["filters.choice"] = block({"type":"interact","choice":"route", "branches":{
        "walk":"walk","stay":"stay","hidden":"stay"},"on_empty":"failed"})
if os.environ.get("NIR_TEST_IMAGE_INHERIT") == "1" or os.environ.get("NIR_TEST_IMAGE_GEOMETRY") == "1":
    story["cues"]["overlay"]["effects"][0]["effect"]["duration_us"]="0"
    for name,color in [("first",[255,0,0,255]),("second",[0,255,0,255]),("unused",[0,0,255,255]),("new",[255,255,0,255])]:
        (PROJECT/f"assets/source/inherit-{name}.png").write_bytes(png(color))
        with (PROJECT/"assets/catalog.toml").open("a") as out:
            out.write(f'\n[[assets]]\nid = "inherit.{name}"\nkind = "image"\nsource = "source/inherit-{name}.png"\nrights = "CC0-1.0"\nexpected_size = [32,24]\n')
    base=json.loads(json.dumps(story["scenes"]["overlaid"]))
    for name in ["first","second","common"]:
        nodes=json.loads(json.dumps(base))
        nodes.append({"id":"inherit.photo","asset":"inherit."+("unused" if name=="common" else name),
            "x":200,"y":100,"width":180,"height":150,"order":1000})
        if os.environ.get("NIR_TEST_IMAGE_GEOMETRY") == "1":
            rectangle = {"first":[200,100,180,150],"second":[400,180,120,90],"common":[900,600,100,50]}[name]
            nodes[-1].update(zip(["x","y","width","height"],rectangle))
        if name=="common":nodes.append({"id":"inherit.new","asset":"inherit.new",
            "x":600,"y":100,"width":180,"height":150,"order":1000})
        story["scenes"][f"inherit.{name}"]=nodes
        effects=[{"id":"stage","scope":"session","effect":{"type":"stage_present","scene":f"inherit.{name}",
            "duration_us":"5000000" if name=="common" else "0"}}]
        if name=="common":
            effects[0]["effect"]["inherit_images"]=["inherit.photo"]
            if os.environ.get("NIR_TEST_IMAGE_GEOMETRY") == "1":
                effects[0]["effect"]["inherit_image_geometry"]=["inherit.photo"]
            effects.append({"id":"inherit.park","scope":"session","effect":{"type":"delay","duration_us":"500000000"}})
        story["cues"][f"inherit.{name}"]={"effects":effects}
    reader["entry"]="inherit.choose"
    reader["blocks"]["inherit.choose"]=block({"type":"interact","choice":"route",
        "branches":{"walk":"inherit.first","stay":"inherit.second"},"on_empty":"failed"})
    for name in ["first","second"]:
        reader["blocks"][f"inherit.{name}"]=block({"type":"activate","cue":f"inherit.{name}","next":"inherit.common"})
    reader["blocks"]["inherit.common"]=block({"type":"activate","cue":"inherit.common","next":"inherit.hold"})
    reader["blocks"]["inherit.hold"]=block({"type":"await","conditions":[{"task":"inherit.park","milestone":{"type":"finished"}}],
        "next":"inherit.done","on_advance":"inherit.done","on_cancelled":"inherit.done","on_failed":"failed"})
    reader["blocks"]["inherit.done"]=block({"type":"end","outcome":"inherited"})
if os.environ.get("NIR_TEST_DIALOGUE_STYLE") == "1":
    story["cues"]["overlay"]["effects"][0]["effect"]["duration_us"] = "0"
    reader["blocks"]["intro"]["ops"] = []
    reader["blocks"]["wait_intro"]["terminator"]["next"] = "window_style"
    reader["blocks"]["window_style"] = block({"type":"activate", "cue":"window_style", "next":"guarded"})
    reader["blocks"]["guarded"]["ops"] = []
    story["cues"]["window_style"] = {"effects":[{"id":"window_style", "scope":"session", "effect":{"type":"dialogue_style", "style":"alternate"}}]}
if os.environ.get("NIR_TEST_DECORATION") == "1":
    story["cues"]["overlay"]["effects"][0]["effect"]["duration_us"] = "0"
    reader["blocks"]["intro"]["ops"] = [o for o in reader["blocks"]["intro"]["ops"] if o["operation"]["type"] != "audio_pause"]
    reader["blocks"]["intro"]["terminator"]["next"] = "decorate"
    reader["blocks"]["decorate"] = block({"type":"activate", "cue":"decorate", "next":"wait_intro"})
    story["cues"]["decorate"] = {"effects":[
        {"id":"portrait", "scope":"session", "effect":{"type":"dialogue_decoration", "slot":"portrait", "image":{"asset":"fixture.icon", "size":[32,24], "placement":{"type":"absolute","point":[100,400]}}}},
        {"id":"name", "scope":"session", "effect":{"type":"dialogue_decoration", "slot":"name", "image":{"asset":"fixture.hover", "size":[32,24], "placement":{"type":"text_origin","offset":[0,-34]}}}}
    ]}
    reader["blocks"]["wait_intro"]["terminator"]["next"] = "remove_portrait"
    reader["blocks"]["remove_portrait"] = block({"type":"activate", "cue":"remove_portrait", "next":"guarded"})
    story["cues"]["remove_portrait"] = {"effects":[{"id":"portrait", "scope":"session", "effect":{"type":"dialogue_decoration", "slot":"portrait", "image":None}}]}
    reader["blocks"]["guarded"]["ops"] = []
if os.environ.get("NIR_TEST_PREVIEW_CANCEL") == "1":
    story["cues"]["overlay"]["effects"][0]["effect"]["duration_us"] = "0"
    story["variables"]["preview_value"] = {"type":"string","value":"old"}
    base = json.loads(json.dumps(story["scenes"]["overlaid"]))
    nodes = json.loads(json.dumps(base))
    nodes.append({"id":"preview.root","x":0,"y":0,"width":1280,"height":720,"order":10000,"color":[0,0,0,0]})
    for index, option in enumerate(story["choices"]["route"]["options"]):
        x,y,w,h = option["image"]["rect"]
        for variant,asset in [("normal",option["image"]["asset"]),("hover",option["image"]["hover_asset"])]:
            nodes.append({"id":f"preview.{index}.{variant}","parent":"preview.root","asset":asset,
                          "x":x,"y":y,"width":w,"height":h,"opacity":int(variant=="normal"),"order":10001+index})
        option["value"] = {"type":"string","value":option["id"]}
    story["scenes"]["preview.menu"] = nodes
    story["scenes"]["preview.cancel"] = base
    for index in range(2):
        selected = json.loads(json.dumps(nodes))
        for node in selected:
            if node["id"] == f"preview.{index}.normal": node["opacity"] = 0
            if node["id"] == f"preview.{index}.hover": node["opacity"] = 1
        story["scenes"][f"preview.keep{index}"] = selected
    for name in ["menu","cancel","keep0","keep1"]:
        story["cues"][f"preview.{name}"] = {"effects":[{"id":"stage","scope":"session","effect":{
            "type":"stage_present","scene":f"preview.{name}","duration_us":"0"}}]}
    reader["entry"] = "preview.enter"
    reader["blocks"]["preview.enter"] = block({"type":"activate","cue":"preview.menu","next":"preview.choose"})
    reader["blocks"]["preview.choose"] = block({"type":"interact","choice":"route",
        "branches":{"walk":"preview.keep0","stay":"preview.keep1"},"result":"preview_value",
        "on_cancel":"preview.cancel","on_empty":"failed"})
    for name in ["cancel","keep0","keep1"]:
        ops = [op("clear-cancel-value",type="assign",target="preview_value",value={"type":"const","value":{"type":"string","value":""}})] if name=="cancel" else []
        reader["blocks"][f"preview.{name}"] = block({"type":"activate","cue":f"preview.{name}","next":"preview.arrive"},ops)
    reader["blocks"]["preview.arrive"] = block({"type":"activate","cue":"arrival","next":"preview.wait"})
    reader["blocks"]["preview.wait"] = block(wait("preview.done"))
    reader["blocks"]["preview.done"] = block({"type":"end","outcome":"preview"})
if os.environ.get("NIR_TEST_STORY_MODAL") == "1":
    story["cues"]["overlay"]["effects"][0]["effect"]["duration_us"] = "0"
    reader["entry"] = "modal.enter"
    for name, target, continuation in [
        ("load", {"type":"load_saves"}, "modal.gallery"),
        ("gallery", {"type":"image_menu", "menu":"story-gallery"}, "modal.after"),
    ]:
        story["cues"]["modal." + name] = {"effects":[{"id":"modal." + name,"scope":"session",
            "effect":{"type":"story_modal","target":target}}]}
        reader["blocks"]["modal." + name] = block({"type":"activate","cue":"modal." + name,"next":"modal.wait." + name})
        reader["blocks"]["modal.wait." + name] = block({"type":"await","conditions":[{"task":"modal." + name,"milestone":{"type":"finished"}}],
            "next":continuation,"on_cancelled":"cancelled","on_failed":"failed"})
    reader["blocks"]["modal.enter"] = block({"type":"goto","target":"modal.load"})
    reader["blocks"]["modal.after"] = block({"type":"activate","cue":"arrival","next":"modal.read"})
    reader["blocks"]["modal.read"] = block(wait("modal.done"))
    reader["blocks"]["modal.done"] = block({"type":"end","outcome":"modal"})
if os.environ.get("NIR_TEST_LONG_AUDIO_SOURCE"):
    shutil.copyfile(os.environ["NIR_TEST_LONG_AUDIO_SOURCE"], PROJECT / "assets/source/bgm.wav")
    catalog_path = PROJECT / "assets/catalog.toml"
    entries = catalog_path.read_text().split("[[assets]]")
    for index, entry in enumerate(entries):
        if 'id = "audio.bgm"' in entry:
            entries[index] = entry.replace('rights = "CC0-1.0"',
                'rights = "Imported source game asset; original rights retained."')
    catalog_path.write_text("[[assets]]".join(entries))
    reader["blocks"]["intro"]["ops"] = [o for o in reader["blocks"]["intro"]["ops"] if o["id"] != "initial-pause"]
    story["cues"]["overlay"]["effects"][0]["effect"]["duration_us"] = "0"
if os.environ.get("NIR_TEST_INTERNED") == "1":
    # Neutral repeated data exercises real encoded code/static packages while
    # retaining the ordinary story's gates, sound and presentation.
    story["variables"]["fixture_payload"] = {"type":"string", "value":""}
    reader["blocks"][reader["entry"]]["ops"][:0] = [
        op(f"interned-{index}", type="assign", target="fixture_payload",
           value={"type":"const","value":{"type":"string","value":"x" * 1024}})
        for index in range(128)
    ]
    for index in range(1000):
        nodes = json.loads(json.dumps(story["scenes"]["station"]))
        nodes[0]["x"] = index / 10
        story["scenes"][f"unused-{index}"] = nodes
    reader["blocks"]["intro"]["ops"] = [o for o in reader["blocks"]["intro"]["ops"] if o["id"] != "initial-pause"]
    for body in reader["blocks"].values():
        body["ops"] = [operation for operation in body["ops"] if operation["operation"]["type"] != "menu_access"]
write(story_path, story)
theme = PROJECT / "themes/rain/theme.toml"
theme.write_text(theme.read_text().replace("[dialogue]", "[dialogue]\nrect = [160.0, 450.0, 960.0, 200.0]\ntext_rect = [200.0, 490.0, 880.0, 120.0]"))
with theme.open("a") as out:
    out.write('\n[image_menus.title]\nbackground = "bg.station"\nbuttons = []\n[[image_menus.title.elements]]\nid = "start"\nrect = [540,250,200,120]\ncontent = { type = "button", label = "Start", asset = "fixture.icon", action = { type = "new_game" } }\n')
    for index in range(65):
        out.write(f'\n[image_menus.page{index}]\nbackground = "bg.station"\nbuttons = []\n')
if os.environ.get("NIR_TEST_STORY_MODAL") == "1":
    with theme.open("a") as out:
        out.write('\n[image_menus.story-gallery]\nbackground = "fixture.hover"\nbuttons = []\nbuiltin_navigation = true\n')
if os.environ.get("NIR_TEST_DIALOGUE_STYLE") == "1":
    with theme.open("a") as out:
        out.write('\n[dialogue_styles.alternate]\ntext = [0.05,0.1,0.15,1.0]\n[dialogue_styles.alternate.dialogue]\nbackground = "fixture.hover"\nrect = [750,180,300,180]\ntext_rect = [770,200,260,140]\nheight = 180\npadding = 0\nfont_size = 28\nline_height = 1.2\nopacity = 1.0\n')
run("resolve", "--sdk", str(SDK))
run("build", "--target", "web", "--audio-format", "mp3", "--locked")
web = PROJECT / "dist/full/web"
ThreadingHTTPServer(("127.0.0.1", 4268), partial(SimpleHTTPRequestHandler, directory=str(web))).serve_forever()
