#!/usr/bin/env python3
"""Build neutral NIR-NEXT fixtures with the distributed SDK and serve locally."""
import json
import struct
import zlib
from pathlib import Path
import shutil
import subprocess
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from threading import Thread

root = Path(__file__).resolve().parent.parent
cli = root / "dist/novelc"


def build(name, mutate, hide_policy=None, setup=None):
    project = root / f"reports/nir-next/{name}"
    shutil.copytree(root / "examples/rain-letters", project,
                    ignore=shutil.ignore_patterns("dist", ".nir", "reports"), dirs_exist_ok=True)
    if hide_policy:
        config = project / "config/player.toml"
        config.write_text(config.read_text().replace("[defaults]", f'[defaults]\nhide_policy = "{hide_policy}"'))
        theme = project / "themes/rain/theme.toml"
        theme.write_text(theme.read_text().replace("[dialogue]", "[dialogue]\nrect = [20.0, 450.0, 1240.0, 220.0]\nshadow = { offset = [3.0, 3.0], color = [1.0, 0.0, 1.0, 1.0] }"))
    if setup:
        setup(project)
    fragment = project / "content/ch01/story.nir.json"
    content = json.loads(fragment.read_text())
    mutate(content)
    fragment.write_text(json.dumps(content, ensure_ascii=False, indent=2) + "\n")
    subprocess.run([str(cli), "-p", str(project), "resolve", "--sdk", str(root / "dist/sdk")], check=True)
    subprocess.run([str(cli), "-p", str(project), "build", "--locked"], check=True)
    return project / "dist/full/web"


def effects(content):
    for cue in content["cues"].values():
        for definition in cue["effects"]:
            if definition["effect"]["type"] == "audio":
                definition["effect"]["gain"] = 1.5
    content["cues"]["intro"]["effects"].extend([
        {"id": "music-stop", "scope": "session",
         "effect": {"type": "audio_stop", "target": "music", "duration_us": "5000000"}},
        {"id": "dialogue-fade", "scope": "session",
         "effect": {"type": "tween", "target": {"type": "dialogue_root", "property": "opacity"},
                    "to": 0.2, "duration_us": "5000000"}}])


def reading(content):
    # Once this short voice ends, only looping BGM keeps the reading clock alive.
    voice = content["cues"]["arrival"]["effects"][1]["effect"].copy()
    content["cues"]["intro"]["effects"].append({"id": "spoken", "scope": "session", "effect": voice})
    content["functions"]["main"]["blocks"]["wait_intro"]["ops"].append({
        "id": "reading-boundary", "operation": {
            "type": "dialogue_voice", "task": "line", "voice": "spoken", "wait": "parallel"}})


def wipe(content):
    for scene,color in [("station",[1.,0.,0.,1.]),("together",[0.,0.,1.,1.])]:
        content["scenes"][scene]=[{"id":"solid","x":0,"y":0,"width":1280,"height":720,"color":color}]
    # 10s like the mask fixtures: software renderers must be able to sample
    # the wipe mid-flight even when a frame stalls for a second or more.
    content["cues"]["intro"]["effects"].insert(0,{"id":"wipe-stage","scope":"scene","effect":{"type":"stage_present","scene":"together","duration_us":"10000000","transition":{"type":"wipe","direction":"left_to_right","softness":0.2}}})

def mask_assets(project):
    # Original 2x2 alpha data; RGB is deliberately unrelated to the threshold.
    def chunk(kind,data):
        return struct.pack(">I",len(data))+kind+data+struct.pack(">I",zlib.crc32(kind+data)&0xffffffff)
    rgba=bytes([0, 255,0,0,0, 0,255,0,255, 0, 0,0,255,128, 255,0,255,0])
    png=b"\x89PNG\r\n\x1a\n"+chunk(b"IHDR",struct.pack(">IIBBBBB",2,2,8,6,0,0,0))+chunk(b"IDAT",zlib.compress(rgba))+chunk(b"IEND",b"")
    (project/"assets/source/mask.png").write_bytes(png)
    with (project/"assets/catalog.toml").open("a") as f:
        f.write('\n[[assets]]\nid = "mask.pattern"\nkind = "image"\nsource = "source/mask.png"\nrights = "CC0-1.0"\nexpected_size = [2, 2]\n')


def mask(content,invert=False):
    wipe(content)
    content["cues"]["intro"]["effects"][0]["effect"]["duration_us"]="10000000"
    content["cues"]["intro"]["effects"][0]["effect"]["transition"]={"type":"mask","asset":"mask.pattern","channel":"alpha","invert":invert,"softness":0.2}
    content["functions"]["main"]["blocks"]["wait_intro"]["ops"].append({"id":"hide-mask-text","operation":{"type":"dialogue_visibility","visible":False}})

def window_assets(project):
    # A 1x1 opaque green backdrop: over the solid red scene the message
    # window reads as pure green wherever the wipe has not erased it.
    def chunk(kind,data):
        return struct.pack(">I",len(data))+kind+data+struct.pack(">I",zlib.crc32(kind+data)&0xffffffff)
    rgba=bytes([0,0,255,0,255])
    png=b"\x89PNG\r\n\x1a\n"+chunk(b"IHDR",struct.pack(">IIBBBBB",1,1,8,6,0,0,0))+chunk(b"IDAT",zlib.compress(rgba))+chunk(b"IEND",b"")
    (project/"assets/source/window.png").write_bytes(png)
    with (project/"assets/catalog.toml").open("a") as f:
        f.write('\n[[assets]]\nid = "window.green"\nkind = "image"\nsource = "source/window.png"\nrights = "CC0-1.0"\nexpected_size = [1, 1]\n')
    theme=project/"themes/rain/theme.toml"
    theme.write_text(theme.read_text().replace(
        "[dialogue]", '[dialogue]\nrect = [20.0, 450.0, 1240.0, 220.0]\nbackground = "window.green"\n'))


def window_reveal(content):
    # A mid-block styled hide over a solid red opening scene (the `opening`
    # cue presents `station` with no stage transition): while the reader parks
    # on the intro line the window wipes away left-to-right over 10s.
    content["scenes"]["station"]=[{"id":"solid","x":0,"y":0,"width":1280,"height":720,"color":[1.,0.,0.,1.]}]
    content["functions"]["main"]["blocks"]["wait_intro"]["ops"].append(
        {"id":"hide-window","operation":{"type":"dialogue_visibility","visible":False,
         "transition":{"type":"wipe","direction":"left_to_right","softness":0.2},
         "duration_us":"10000000"}})

def menu_assets(project):
    def png(name, rgba):
        def chunk(kind,data):
            return struct.pack(">I",len(data))+kind+data+struct.pack(">I",zlib.crc32(kind+data)&0xffffffff)
        data=b"\x89PNG\r\n\x1a\n"+chunk(b"IHDR",struct.pack(">IIBBBBB",1,1,8,6,0,0,0))+chunk(b"IDAT",zlib.compress(bytes([0,*rgba])))+chunk(b"IEND",b"")
        (project/f"assets/source/{name}.png").write_bytes(data)
        with (project/"assets/catalog.toml").open("a") as f:
            f.write(f'\n[[assets]]\nid = "menu.{name}"\nkind = "image"\nsource = "source/{name}.png"\nrights = "CC0-1.0"\nexpected_size = [1, 1]\n')
    png("black",[0,0,0,255]);png("blue",[0,0,255,128])
    theme=project/"themes/rain/theme.toml"
    with theme.open("a") as f:
        f.write('\n[image_menus.title]\nbackground = "menu.black"\nbuttons = []\n')
        def element(id,x,content):
            f.write(f'\n[[image_menus.title.elements]]\nid = "{id}"\nrect = [{x}, 180, 280, 150]\ncontent = {{ {content} }}\n')
        text='type = "text", text = "MMMM", size = 70, color = [1.0, 0.0, 0.0, 1.0]'
        image='type = "image", asset = "menu.blue"'
        element("first",100,text)
        element("second",450,text);element("cover",450,image)
        element("under",800,image);element("third",800,text)
        element("start",100,'type = "hit_region", label = "Start", action = { type = "new_game" }')
        element("locked",100,'type = "hit_region", label = "Locked", action = { type = "new_game" }, requires = "seen"')
        element("settings",800,'type = "hit_region", label = "Settings", action = { type = "settings" }')
        for id,x in [("hover-left",100),("hover-right",450)]:
            f.write(f'\n[[image_menus.title.elements]]\nid = "{id}"\nrect = [{x}, 500, 200, 80]\ncontent = {{ type = "button", label = "Settings", asset = "menu.blue", hover_asset = "menu.black", action = {{ type = "settings" }} }}\n')

menu_project=build("browser-menu-elements",lambda c:None,setup=menu_assets)
menu_server=ThreadingHTTPServer(("127.0.0.1",4204),partial(SimpleHTTPRequestHandler,directory=str(menu_project)))
Thread(target=menu_server.serve_forever,daemon=True).start()

def menu_state_assets(project):
    menu_assets(project)
    with (project/"themes/rain/theme.toml").open("a") as f:
        f.write('\n[image_menus.title.locals.tab]\ntype = "enum"\ninitial = "Settings"\nvalues = ["Settings", "Saves", "History"]\n[image_menus.title.locals.slot]\ntype = "int"\ninitial = 0\nmin = 0\nmax = 2\n')
        for tab,x in [("Settings",50),("Saves",300),("History",550)]:
            f.write(f'\n[[image_menus.title.elements]]\nid = "tab-{tab}"\nrect = [{x}, 350, 220, 60]\ncontent = {{ type = "hit_region", label = "{tab} tab", action = {{ type = "set_local", local = "tab", value = "{tab}" }} }}\n')
        f.write('\n[[image_menus.title.elements]]\nid = "tab-label"\nrect = [800,350,350,70]\ntext_local = "tab"\ncontent = { type = "text", text = "Settings", size = 40, color = [1,1,1,1] }\n')
        f.write('\n[[image_menus.title.elements]]\nid = "slots"\nrect = [0,0,0,0]\nvisible_when = [{type = "local", name = "tab", equals = "Saves"}]\ncontent = { type = "group" }\n')
        f.write('\n[[image_menus.title.elements]]\nid = "choose-slot"\nparent = "slots"\nrect = [50,430,220,60]\nenabled_when = [{type = "local", name = "slot", equals = 0}]\ncontent = { type = "hit_region", label = "Select slot 2", action = { type = "set_local", local = "slot", value = 2 } }\n')
        f.write('\n[[image_menus.title.elements]]\nid = "slot-label"\nparent = "slots"\nrect = [800,430,350,60]\ntext_local = "slot"\ncontent = { type = "text", text = "0", size = 40, color = [1,1,1,1] }\n')

state_project=build("browser-menu-state",lambda c:None,setup=menu_state_assets)
state_server=ThreadingHTTPServer(("127.0.0.1",4205),partial(SimpleHTTPRequestHandler,directory=str(state_project)))
Thread(target=state_server.serve_forever,daemon=True).start()

def menu_service_assets(project):
    theme=project/"themes/rain/theme.toml"
    original=theme.read_text()
    menu_assets(project)
    theme.write_text(original.replace("[slots]",'menu_overlay = "system"\n\n[slots]',1))
    with theme.open("a") as f:
        f.write('\n[image_menus.title]\nbackground = "menu.black"\nbuttons = []\n[[image_menus.title.elements]]\nid = "start"\nrect = [80,100,400,80]\ncontent = { type = "button", label = "Start", asset = "menu.blue", action = {type = "new_game"} }\n')
        f.write('\n[image_menus.system]\nbackground = "menu.black"\nbuttons = []\n')
        for name,y in [("text_speed",180),("bgm_volume",280),("auto_wait_scale",380)]:
            f.write(f'\n[[image_menus.system.elements]]\nid = "value-{name}"\nrect = [80,{y},220,70]\ntext_preference = "{name}"\ncontent = {{ type = "text", text = "1.00", size = 40, color = [1,1,1,1] }}\n')
            f.write(f'\n[[image_menus.system.elements]]\nid = "increase-{name}"\nrect = [350,{y},300,70]\ncontent = {{ type = "button", label = "Increase {name}", asset = "menu.blue", action = {{type = "adjust_preference", field = "{name}", delta = 0.25 }} }}\n')
        f.write('\n[[image_menus.system.elements]]\nid = "close"\nrect = [350,500,300,70]\ncontent = { type = "button", label = "Return to story", asset = "menu.blue", action = {type = "close"} }\n')

service_project=build("browser-menu-services",lambda c:None,setup=menu_service_assets)
service_server=ThreadingHTTPServer(("127.0.0.1",4206),partial(SimpleHTTPRequestHandler,directory=str(service_project)))
Thread(target=service_server.serve_forever,daemon=True).start()

def menu_storage_assets(project):
    menu_service_assets(project)
    theme=project/"themes/rain/theme.toml"
    source=theme.read_text().split("[image_menus.system]",1)[0]
    source += '\n[image_menus.system]\nbackground = "menu.black"\nbuttons = []\n[image_menus.system.locals.selected]\ntype = "int"\ninitial = 0\nmin = 0\nmax = 2\n'
    source += '\n[[image_menus.system.elements]]\nid = "slot-label"\nrect = [80,180,1000,80]\ntext_slot = { type = "local", name = "selected" }\ncontent = { type = "text", text = "Empty slot", size = 30, color = [1,1,1,1] }\n'
    for name,label,x,action in [
        ("select","Select slot 2",80,'{type = "set_local", local = "selected", value = 1}'),
        ("save","Save selected",440,'{type = "save_slot", slot = {type = "local", name = "selected"}}'),
        ("load","Load selected",800,'{type = "load_slot", slot = {type = "local", name = "selected"}}')]:
        source += f'\n[[image_menus.system.elements]]\nid = "{name}"\nrect = [{x},320,300,80]\ncontent = {{type = "button", label = "{label}", asset = "menu.blue", action = {action}}}\n'
    theme.write_text(source)

storage_project=build("browser-menu-storage",lambda c:None,setup=menu_storage_assets)
storage_server=ThreadingHTTPServer(("127.0.0.1",4207),partial(SimpleHTTPRequestHandler,directory=str(storage_project)))
Thread(target=storage_server.serve_forever,daemon=True).start()

def history_lines(content, count=4):
    blocks=content["functions"]["main"]["blocks"]
    blocks["start"]["terminator"]["next"]="history0"
    for i in range(count):
        content["cues"][f"history{i}"]={"effects":[{"id":"line","scope":"interaction","effect":{"type":"dialogue","text":"intro" if i%2==0 else "arrival","speaker":"","reveal_us":"0"}}]}
        blocks[f"history{i}"]={"ops":[],"terminator":{"type":"activate","cue":f"history{i}","next":f"history-wait{i}"}}
        blocks[f"history-wait{i}"]={"ops":[],"terminator":{"type":"await","conditions":[{"task":"line","milestone":{"type":"finished"}}],"next":f"history{i+1}" if i<count-1 else "enter","on_cancelled":"cancelled","on_failed":"failed"}}

def menu_history_assets(project):
    menu_service_assets(project)
    theme=project/"themes/rain/theme.toml"
    source=theme.read_text().split("[image_menus.system]",1)[0]
    source += '\n[image_menus.system]\nbackground = "menu.black"\nbuttons = []\n[image_menus.system.locals.offset]\ntype = "int"\ninitial = 0\nmin = 0\nmax = 999\n'
    source += '\n[[image_menus.system.elements]]\nid = "records"\nrect = [80,120,1120,360]\ncontent = { type = "history_window", offset_local = "offset", limit = 2, row_height = 180, size = 30, color = [1,1,1,1] }\n'
    for name,x,delta in [("Older",80,1),("Newer",440,-1)]:
        source += f'\n[[image_menus.system.elements]]\nid = "{name}"\nrect = [{x},540,300,80]\ncontent = {{type = "button", label = "{name}", asset = "menu.blue", action = {{type = "history_page", window = "records", delta = {delta}}}}}\n'
    theme.write_text(source)

history_project=build("browser-menu-history",history_lines,setup=menu_history_assets)
history_server=ThreadingHTTPServer(("127.0.0.1",4208),partial(SimpleHTTPRequestHandler,directory=str(history_project)))
Thread(target=history_server.serve_forever,daemon=True).start()

def menu_value_assets(project):
    menu_service_assets(project)
    theme=project/"themes/rain/theme.toml"
    source=theme.read_text().split("[image_menus.system]",1)[0]
    source += '\n[image_menus.system]\nbackground = "menu.black"\nbuttons = []\n'
    source += '\n[[image_menus.system.elements]]\nid = "volume"\nrect = [100,120,600,100]\ncontent = { type = "range", label = "Volume", binding = {type = "preference", field = "bgm_volume"}, min = 0.0, max = 1.0, step = 0.1 }\n'
    source += '\n[[image_menus.system.elements]]\nid = "motion"\nrect = [100,300,600,80]\ncontent = { type = "toggle", label = "Motion", binding = {type = "reduced_motion"} }\n'
    theme.write_text(source)

value_project=build("browser-menu-values",lambda c:None,setup=menu_value_assets)
value_server=ThreadingHTTPServer(("127.0.0.1",4209),partial(SimpleHTTPRequestHandler,directory=str(value_project)))
Thread(target=value_server.serve_forever,daemon=True).start()

def sampled_reading(content):
    reading(content)
    for op in content["functions"]["main"]["blocks"]["wait_intro"]["ops"]:
        if op["operation"]["type"]=="dialogue_voice":
            op["operation"]["wait"]="sampled_remaining"

def sampled_settings(project):
    config=project/"config/player.toml"
    source=config.read_text().replace('[defaults]','[defaults]\nauto_delay_policy = "fixed"')
    source=source.replace('auto_delay_us = "1200000"','auto_delay_us = "500000"')
    config.write_text(source)

sampled_project=build("browser-sampled-reading",sampled_reading,setup=sampled_settings)
sampled_server=ThreadingHTTPServer(("127.0.0.1",4210),partial(SimpleHTTPRequestHandler,directory=str(sampled_project)))
Thread(target=sampled_server.serve_forever,daemon=True).start()

def menu_reading_assets(project):
    menu_service_assets(project)
    theme=project/"themes/rain/theme.toml"
    source=theme.read_text().split("[image_menus.system]",1)[0]
    source += '\n[image_menus.system]\nbackground = "menu.black"\nbuttons = []\n'
    for i,(mode,label) in enumerate([("auto","Resume Auto"),("skip_read","Skip read"),("peek_story","Hide text")]):
        source += f'\n[[image_menus.system.elements]]\nid = "{mode}"\nrect = [120,{100+i*120},560,80]\ncontent = {{type = "button", label = "{label}", asset = "menu.blue", action = {{type = "reading", mode = "{mode}"}}}}\n'
    theme.write_text(source)

menu_reading_project=build("browser-menu-reading",reading,setup=menu_reading_assets)
menu_reading_server=ThreadingHTTPServer(("127.0.0.1",4211),partial(SimpleHTTPRequestHandler,directory=str(menu_reading_project)))
Thread(target=menu_reading_server.serve_forever,daemon=True).start()

def menu_choice(content):
    content["functions"]["main"]["entry"]="choose"

menu_choice_project=build("browser-menu-choice",menu_choice,setup=menu_reading_assets)
menu_choice_server=ThreadingHTTPServer(("127.0.0.1",4212),partial(SimpleHTTPRequestHandler,directory=str(menu_choice_project)))
Thread(target=menu_choice_server.serve_forever,daemon=True).start()

def menu_flow_assets(project):
    menu_reading_assets(project)
    theme=project/"themes/rain/theme.toml"
    source=theme.read_text().split("[image_menus.system]",1)[0]
    source += '\n[image_menus.system]\nbackground = "menu.black"\nbuttons = []\n'
    source += '\n[[image_menus.system.elements]]\nid = "rows"\nrect = [120,100,560,400]\ncontent = {type = "stack", gap = 40}\n'
    for mode,label in [("auto","Resume Auto"),("skip_read","Skip read"),("peek_story","Hide text")]:
        source += f'\n[[image_menus.system.elements]]\nid = "{mode}"\nparent = "rows"\nrect = [0,0,560,80]\nvisible_when = [{{type = "reading_available", mode = "{mode}", available = true}}]\ncontent = {{type = "button", label = "{label}", asset = "menu.blue", action = {{type = "reading", mode = "{mode}"}}}}\n'
    theme.write_text(source)

for port,content in [(4213,reading),(4214,menu_choice)]:
    site=build(f"browser-menu-flow-{port}",content,setup=menu_flow_assets)
    server=ThreadingHTTPServer(("127.0.0.1",port),partial(SimpleHTTPRequestHandler,directory=str(site)))
    Thread(target=server.serve_forever,daemon=True).start()

for port,invert in [(4202,False),(4203,True)]:
    site=build(f"browser-mask-{port}",lambda c,invert=invert:mask(c,invert),setup=mask_assets)
    server=ThreadingHTTPServer(("127.0.0.1",port),partial(SimpleHTTPRequestHandler,directory=str(site)))
    Thread(target=server.serve_forever,daemon=True).start()

def menu_text_assets(project):
    menu_service_assets(project)
    theme=project/"themes/rain/theme.toml"
    source=theme.read_text().split("[image_menus.system]",1)[0]
    source += '\n[image_menus.system]\nbackground = "menu.black"\nbuttons = []\n'
    for i,(name,label) in enumerate([("resume","Resume"),("locked","Locked")]):
        guard=', requires = "unavailable"' if i else ''
        source += f'\n[[image_menus.system.elements]]\nid = "{name}"\nrect = [100,{100+i*120},560,80]\ncontent = {{type = "text_button", label = "{label}", size = 48, color = [1,0,0,1], hover_color = [0,1,0,1], disabled_color = [0,0,1,1], action = {{type = "close"}}{guard}}}\n'
    theme.write_text(source)

text_menu_project=build("browser-menu-text",reading,setup=menu_text_assets)
text_menu_server=ThreadingHTTPServer(("127.0.0.1",4215),partial(SimpleHTTPRequestHandler,directory=str(text_menu_project)))
Thread(target=text_menu_server.serve_forever,daemon=True).start()

def menu_story_assets(project):
    menu_service_assets(project)
    theme=project/"themes/rain/theme.toml"
    source=theme.read_text().split("[image_menus.system]",1)[0]
    source += '\n[image_menus.system]\nbackground = "menu.black"\nbuttons = []\nstory_exports = { progress = "affection" }\n'
    source += '\n[[image_menus.system.elements]]\nid = "rows"\nrect = [100,100,560,300]\ncontent = {type = "stack", gap = 20}\n'
    for name,label,guard in [("initial","Initial only",'visible_when = [{type = "story", name = "progress", equals = 0}]\n'),("resume","Resume",'')]:
        source += f'\n[[image_menus.system.elements]]\nid = "{name}"\nparent = "rows"\nrect = [0,0,560,80]\n{guard}content = {{type = "button", label = "{label}", asset = "menu.blue", action = {{type = "close"}}}}\n'
    theme.write_text(source)

story_menu_project=build("browser-menu-story",menu_choice,setup=menu_story_assets)
story_menu_server=ThreadingHTTPServer(("127.0.0.1",4217),partial(SimpleHTTPRequestHandler,directory=str(story_menu_project)))
Thread(target=story_menu_server.serve_forever,daemon=True).start()

def history_flow_assets(project):
    menu_service_assets(project)
    theme=project/"themes/rain/theme.toml"
    source=theme.read_text().split("[image_menus.system]",1)[0]
    source += '\n[image_menus.system]\nbackground = "menu.black"\nbuiltin_navigation = false\nbuttons = []\n'
    source += '\n[[image_menus.system.elements]]\nid = "records"\nrect = [80,120,600,360]\ncontent = {type = "history_flow", size = 30, line_height = 45, gap = 18, wheel_step = 90, page_step = 180, max_visible = 32, color = [1,1,1,1]}\n'
    for name,color in [("track",[40,40,40,255]),("normal",[0,80,240,255]),("hover",[0,200,80,255]),("pressed",[240,40,0,255]),("disabled",[100,100,100,255])]:
        def chunk(kind,data):
            return struct.pack(">I",len(data))+kind+data+struct.pack(">I",zlib.crc32(kind+data)&0xffffffff)
        png=b"\x89PNG\r\n\x1a\n"+chunk(b"IHDR",struct.pack(">IIBBBBB",1,1,8,6,0,0,0))+chunk(b"IDAT",zlib.compress(bytes([0,*color])))+chunk(b"IEND",b"")
        (project/f"assets/source/bar-{name}.png").write_bytes(png)
        with (project/"assets/catalog.toml").open("a") as catalog:
            catalog.write(f'\n[[assets]]\nid = "bar.{name}"\nkind = "image"\nsource = "source/bar-{name}.png"\nrights = "CC0-1.0"\nexpected_size = [1, 1]\n')
    states='{asset = "bar.normal", hover_asset = "bar.hover", pressed_asset = "bar.pressed", disabled_asset = "bar.disabled"}'
    source += '\n[[image_menus.system.elements]]\nid = "history-scroll"\nrect = [700,120,32,360]\n'
    source += f'content = {{type = "history_scrollbar", window = "records", label = "History scroll", thumb_height = 24, arrow_height = 16, line_step = 45, track = {{asset = "bar.track"}}, thumb = {states}, decrease = {states}, increase = {states}}}\n'
    source += '\n[[image_menus.system.elements]]\nid = "close"\nrect = [80,560,300,80]\ncontent = {type = "button", label = "Close history", asset = "menu.blue", action = {type = "close"}}\n'
    theme.write_text(source)

history_flow_project=build("browser-history-flow",lambda c:history_lines(c,40),setup=history_flow_assets)
history_flow_server=ThreadingHTTPServer(("127.0.0.1",4218),partial(SimpleHTTPRequestHandler,directory=str(history_flow_project)))
Thread(target=history_flow_server.serve_forever,daemon=True).start()

def navigation_assets(project):
    menu_service_assets(project)
    theme=project/"themes/rain/theme.toml"
    source=theme.read_text().split("[image_menus.title]",1)[0]
    def page(name):
        nonlocal source
        source += f'\n[image_menus.{name}]\nbackground = "menu.black"\nbuttons = []\nlocals = {{ selected = {{type = "int", initial = 0, min = 0, max = 9}} }}\n'
    def button(page,id,label,y,action):
        nonlocal source
        source += f'\n[[image_menus.{page}.elements]]\nid = "{id}"\nrect = [80,{y},500,60]\ncontent = {{type = "text_button", label = "{label}", size = 30, color = [1,1,1,1], hover_color = [0,1,0,1], disabled_color = [0.35,0.35,0.35,1], action = {action}}}\n'
    page("title")
    button("title","start","Start",100,'{type = "new_game"}')
    button("title","system","Open system",200,'{type = "push_menu", menu = "system"}')
    page("system")
    source += '\n[[image_menus.system.elements]]\nid = "selected-value"\nrect = [80,100,500,60]\ntext_local = "selected"\ncontent = {type = "text", text = "0", size = 40, color = [1,1,1,1]}\n'
    button("system","select","Select two",200,'{type = "set_local", local = "selected", value = 2}')
    button("system","history","Open history",300,'{type = "push_menu", menu = "history"}')
    source += 'enabled_when = [{type = "history_available", available = true}]\n'
    button("system","parent","Parent",400,'{type = "back"}')
    page("history")
    source += '\n[[image_menus.history.elements]]\nid = "records"\nrect = [80,120,600,360]\ncontent = {type = "history_flow", size = 30, line_height = 45, gap = 18, wheel_step = 90, page_step = 180, max_visible = 32, color = [1,1,1,1]}\n'
    button("history","parent","Return to parent",560,'{type = "back"}')
    theme.write_text(source)

navigation_project=build("browser-menu-navigation",lambda c:history_lines(c,6),setup=navigation_assets)
navigation_server=ThreadingHTTPServer(("127.0.0.1",4219),partial(SimpleHTTPRequestHandler,directory=str(navigation_project)))
Thread(target=navigation_server.serve_forever,daemon=True).start()

def menu_effects_assets(project):
    menu_service_assets(project)
    theme=project/"themes/rain/theme.toml"
    source=theme.read_text().split("[image_menus.title]",1)[0]
    # Title: enter sound plus looping page music. Overlay: the full set --
    # enter/close fades, click feedback and different looping music -- so the
    # page-change, close-transaction and accepted-commit boundaries all fire.
    source += '\n[image_menus.title]\nbackground = "menu.black"\nbuttons = []\n'
    source += '\n[image_menus.title.effects.enter]\nsound = "audio.bell"\nfade_us = "400000"\n'
    source += '\n[image_menus.title.effects.music]\nasset = "audio.bgm"\n'
    source += '\n[[image_menus.title.elements]]\nid = "start"\nrect = [80,100,400,80]\ncontent = { type = "button", label = "Start", asset = "menu.blue", action = {type = "new_game"} }\n'
    source += '\n[image_menus.system]\nbackground = "menu.black"\nbuttons = []\n'
    source += '\n[image_menus.system.effects]\nclick = "audio.bell"\n'
    source += '\n[image_menus.system.effects.enter]\nsound = "audio.bell"\nfade_us = "400000"\n'
    source += '\n[image_menus.system.effects.close]\nsound = "audio.bell"\nfade_us = "700000"\n'
    source += '\n[image_menus.system.effects.music]\nasset = "audio.voice"\nbus = "voice"\ngain = 0.5\n'
    source += '\n[[image_menus.system.elements]]\nid = "increase"\nrect = [80,180,300,70]\ncontent = { type = "button", label = "Increase speed", asset = "menu.blue", action = {type = "adjust_preference", field = "text_speed", delta = 0.25 } }\n'
    source += '\n[[image_menus.system.elements]]\nid = "close"\nrect = [80,320,300,70]\ncontent = { type = "button", label = "Return to story", asset = "menu.blue", action = {type = "close"} }\n'
    theme.write_text(source)

effects_menu_project=build("browser-menu-effects",lambda c:None,setup=menu_effects_assets)
effects_menu_server=ThreadingHTTPServer(("127.0.0.1",4220),partial(SimpleHTTPRequestHandler,directory=str(effects_menu_project)))
Thread(target=effects_menu_server.serve_forever,daemon=True).start()

def menu_wipe(content):
    # The story parks on a solid red station scene so the page reveal can be
    # sampled against a known underlying frame.
    content["scenes"]["station"]=[{"id":"solid","x":0,"y":0,"width":1280,"height":720,"color":[1.,0.,0.,1.]}]

def menu_wipe_assets(project):
    menu_service_assets(project)
    theme=project/"themes/rain/theme.toml"
    source=theme.read_text().split("[image_menus.system]",1)[0]
    # Both pages wipe in over the underlying frame; the overlay wipes out
    # again. Styled boundaries cap at 2 s (E_VIEW_EFFECTS), so the spec cannot
    # lean on the stage fixtures' long durations: it freezes both time domains
    # on the crossing frame and samples the frozen composite's pure page and
    # frame columns.
    source += '\n[image_menus.title.effects.enter]\nfade_us = "1200000"\nstyle = {type = "wipe", direction = "left_to_right", softness = 0.2}\n'
    source += '\n[image_menus.system]\nbackground = "menu.black"\nbuttons = []\n'
    source += '\n[image_menus.system.effects.enter]\nfade_us = "2000000"\nstyle = {type = "wipe", direction = "left_to_right", softness = 0.1}\n'
    source += '\n[image_menus.system.effects.close]\nsound = "audio.bell"\nfade_us = "2000000"\nstyle = {type = "wipe", direction = "right_to_left", softness = 0.1}\n'
    source += '\n[[image_menus.system.elements]]\nid = "close"\nrect = [490,320,300,80]\ncontent = { type = "button", label = "Return to story", asset = "menu.blue", action = {type = "close"} }\n'
    theme.write_text(source)

wipe_menu_project=build("browser-menu-wipe",menu_wipe,setup=menu_wipe_assets)
wipe_menu_server=ThreadingHTTPServer(("127.0.0.1",4224),partial(SimpleHTTPRequestHandler,directory=str(wipe_menu_project)))
Thread(target=wipe_menu_server.serve_forever,daemon=True).start()

def replay_assets(project):
    menu_service_assets(project)
    theme=project/"themes/rain/theme.toml"
    source=theme.read_text().split("[image_menus.system]",1)[0]
    # Overlay with a locked replay control (profile key "seen") and a live-only
    # exit; the replay function itself replays the arrival cue and merges a
    # profile key that must never escape the transaction.
    source += '\n[image_menus.system]\nbackground = "menu.black"\nbuttons = []\n'
    source += '\n[[image_menus.system.elements]]\nid = "replay"\nrect = [80,160,420,80]\ncontent = { type = "button", label = "Replay arrival", asset = "menu.blue", action = {type = "replay", function = "replay"}, requires = "seen" }\n'
    source += '\n[[image_menus.system.elements]]\nid = "exit"\nrect = [80,280,420,80]\ncontent = { type = "button", label = "Exit replay", asset = "menu.blue", action = {type = "exit_replay"} }\n'
    source += '\n[[image_menus.system.elements]]\nid = "close"\nrect = [80,400,420,80]\ncontent = { type = "button", label = "Return to story", asset = "menu.blue", action = {type = "close"} }\n'
    theme.write_text(source)

def replay_function(content):
    content["functions"]["replay"]={
        "entry":"start",
        "blocks":{
            "start":{"ops":[],"terminator":{"type":"activate","cue":"arrival","next":"wait"}},
            "wait":{"ops":[],"terminator":{"type":"await",
                "conditions":[{"task":"line","milestone":{"type":"finished"}}],
                "next":"mark","on_cancelled":"mark","on_failed":"mark"}},
            "mark":{"ops":[{"id":"seen.once","operation":{"type":"profile_merge","key":"replay-seen"}}],
                    "terminator":{"type":"end","outcome":"replay-done"}}
        }
    }

replay_project=build("browser-replay",replay_function,setup=replay_assets)
replay_server=ThreadingHTTPServer(("127.0.0.1",4221),partial(SimpleHTTPRequestHandler,directory=str(replay_project)))
Thread(target=replay_server.serve_forever,daemon=True).start()

def compose(content):
    # The sample Activate/Await cannot compile: while the VM waits on the
    # dialogue, a chain first fades the panel and then the text on its own.
    content["cues"]["intro"]["effects"][0]["effect"]["reveal_us"]="200000"
    content["cues"]["intro"]["effects"].append({"id":"chain","scope":"session",
        "effect":{"type":"sequence","children":[
            {"id":"panel-fade","scope":"session",
             "effect":{"type":"tween","target":{"type":"dialogue_root","property":"background_opacity"},
                        "to":0.2,"duration_us":"1500000"}},
            {"id":"text-fade","scope":"session",
             "effect":{"type":"tween","target":{"type":"dialogue_root","property":"text_opacity"},
                        "to":0.35,"duration_us":"1500000"}},
        ]}})

compose_project=build("browser-compose",compose)
compose_server=ThreadingHTTPServer(("127.0.0.1",4222),partial(SimpleHTTPRequestHandler,directory=str(compose_project)))
Thread(target=compose_server.serve_forever,daemon=True).start()

def typed_result(content):
    # The story opens on a typed interaction: the VM writes the chosen
    # option's declared value, a switch turns that value into different
    # dialogue, and a declared cancel path exits without any write.
    content["variables"]["picked"]={"type":"i32","value":0}
    for option,value in zip(content["choices"]["route"]["options"],[1,2]):
        option["value"]={"type":"i32","value":value}
    blocks=content["functions"]["main"]["blocks"]
    blocks["choose"]["terminator"]={
        "type":"interact","choice":"route",
        "branches":{"walk":"typed_commit","stay":"typed_commit"},
        "on_empty":"failed","result":"picked","on_cancel":"typed_cancel"}
    blocks["typed_commit"]={"ops":[],"terminator":{"type":"switch",
        "value":{"type":"var","name":"picked"},
        "cases":{"1":"walk_line","2":"stay_line"},"default":"failed"}}
    blocks["typed_cancel"]={"ops":[],"terminator":{"type":"activate","cue":"arrival","next":"wait_cancel"}}
    blocks["wait_cancel"]={"ops":[],"terminator":{"type":"await",
        "conditions":[{"task":"line","milestone":{"type":"finished"}}],
        "next":"cancel_end","on_cancelled":"cancelled","on_failed":"failed"}}
    blocks["cancel_end"]={"ops":[],"terminator":{"type":"end","outcome":"gave_up"}}
    content["functions"]["main"]["entry"]="choose"

typed_project=build("browser-typed-result",typed_result)
typed_server=ThreadingHTTPServer(("127.0.0.1",4223),partial(SimpleHTTPRequestHandler,directory=str(typed_project)))
Thread(target=typed_server.serve_forever,daemon=True).start()

window_project=build("browser-window-reveal",window_reveal,setup=window_assets)
window_server=ThreadingHTTPServer(("127.0.0.1",4216),partial(SimpleHTTPRequestHandler,directory=str(window_project)))
Thread(target=window_server.serve_forever,daemon=True).start()

wipe_project=build("browser-wipe-project",wipe)
wipe_server=ThreadingHTTPServer(("127.0.0.1",4201),partial(SimpleHTTPRequestHandler,directory=str(wipe_project)))
Thread(target=wipe_server.serve_forever,daemon=True).start()
main = build("browser-project", effects)
reader = build("browser-reading-project", reading)
paused_reader = build("browser-hide-project", reading, "pause_story")
hide_server = ThreadingHTTPServer(("127.0.0.1", 4200), partial(SimpleHTTPRequestHandler, directory=str(paused_reader)))
Thread(target=hide_server.serve_forever, daemon=True).start()
reading_server = ThreadingHTTPServer(("127.0.0.1", 4199), partial(SimpleHTTPRequestHandler, directory=str(reader)))
Thread(target=reading_server.serve_forever, daemon=True).start()
ThreadingHTTPServer(("127.0.0.1", 4198), partial(SimpleHTTPRequestHandler, directory=str(main))).serve_forever()
