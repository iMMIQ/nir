#!/usr/bin/env python3
"""Reproducible original demo and test fixtures. No third-party game assets."""
from pathlib import Path
import json, math, wave, struct, shutil, random, hashlib
from PIL import Image, ImageDraw, ImageFilter
from fontTools.ttLib import TTFont
from fontTools import subset
ROOT=Path(__file__).resolve().parents[1]
out=ROOT/'examples/rain-letters'
for d in ['content/ch01/texts','assets/source','themes/rain','tests/scenarios','credits']:(out/d).mkdir(parents=True,exist_ok=True)
def dump(path,v):path.write_text(json.dumps(v,ensure_ascii=False,indent=2)+'\n')
def val(v):return {'type':'const','value':{'type':'bool' if isinstance(v,bool) else 'i32' if isinstance(v,int) else 'string','value':v}}
def ref(n):return {'type':'var','name':n}
def end(name):return {'type':'end','outcome':name}
def go(name):return {'type':'goto','target':name}
def activate(c,n):return {'type':'activate','cue':c,'next':n}
def wait(task,n,m='finished'):
 return {'type':'await','conditions':[{'task':task,'milestone':{'type':m}}],'next':n,'on_cancelled':'cancelled','on_failed':'failed'}
def op(id,typ,**kw):return {'id':id,'operation':{'type':typ,**kw}}
def block(t,*ops):return {'ops':list(ops),'terminator':t}
def effect(id,typ,scope='scene',**kw):return {'id':id,'scope':scope,'effect':{'type':typ,**kw}}
def node(id,asset,x,y,w,h,**kw):return dict(id=id,asset=asset,x=x,y=y,width=w,height=h,**kw)
lines={
 'speaker.aki':('秋','Aki'),
 'intro':('末班电车刚刚离开。雨把站前的灯光，揉成一封迟迟没有寄出的信。','The last train has gone. Rain folds the station lights into a letter that was never sent.'),
 'arrival':('你还是来了。','You came after all.'),
 'letter':('这封信，我写了很久。','I took a long time to write this letter.'),
 'after_gate':('远处的钟响了。秋把信封放进你的掌心。','A distant bell rings. Aki places the envelope in your hand.'),
 'question':('雨好像小了一些。接下来……你想去哪里？','The rain is letting up. Where would you like to go?'),
 'walk':('沿着河边，一起走回去','Walk home together along the river'),
 'stay':('留在车站，读完这封信','Stay at the station and read the letter'),
 'walk_line':('那就慢慢走吧。今天的路，我想和你一起走完。','Let’s take our time. Tonight, I want to walk the whole way with you.'),
 'walk_end':('雨停了。没有说出口的话，也终于有了归处。','The rain stops. At last, the words left unspoken have found a home.'),
 'stay_line':('那我就在这里，陪你读到最后。','Then I’ll stay here with you until the last word.'),
 'stay_end':('纸上的字迹有些模糊。可你知道，这一次，你不会再错过。','The ink is a little blurred. But this time, you know you won’t miss what it means.'),
}
contracts={k:{'source_revision':1,'contract_revision':1,'meaning_revision':1,'gates':['bell'] if k=='letter' else [],'params':{}} for k in lines}
locales={}
for i,loc in enumerate(['zh-Hans','en']):
 texts={k:{'source_revision':1,'contract_revision':1,'spans':[{'type':'text','id':'body','text':v[i],'emphasis':False}]+([{'type':'gate','id':'bell'},{'type':'text','id':'tail','text':'你愿意收下吗？' if i==0 else 'Will you take it?','emphasis':False}] if k=='letter' else [])} for k,v in lines.items()}
 locales[loc]=texts;dump(out/f'content/ch01/texts/{loc}.json',texts)
dump(out/'content/ch01/texts/contracts.json',contracts)
scenes={'station':[node('background','bg.station',0,0,1280,720)],'together':[node('background','bg.station',0,0,1280,720),node('aki','actor.aki',850,100,290,530,opacity=1.)],'river':[node('background','bg.river',0,0,1280,720),node('aki','actor.aki',820,100,290,530,opacity=1.)]}
cues={'opening':{'effects':[effect('stage','stage_present',scene='station',duration_us='0'),effect('music','audio',scope='session',asset='audio.bgm',bus='bgm',looped=True)]},'enter':{'effects':[effect('stage','stage_present',scene='together',duration_us='600000')]},'bell':{'effects':[effect('bell','audio',scope='interaction',asset='audio.bell',bus='sfx',looped=False)]},'river':{'effects':[effect('stage','stage_present',scene='river',duration_us='900000')]},'nod':{'effects':[effect('nod','clip',node='aki',property='y',to=112,duration_us='350000',easing='smooth')]}}
for key in ['intro','arrival','letter','after_gate','question','walk_line','walk_end','stay_line','stay_end']:
 effects=[effect('line','dialogue',scope='interaction',text=key,speaker='' if key in ['intro','after_gate','walk_end','stay_end'] else 'speaker.aki',reveal_us='32000')]
 if key=='arrival':effects.append(effect('voice','audio',scope='interaction',asset='audio.voice',bus='voice',looped=False))
 cues[key]={'effects':effects}
blocks={
 'start':block(activate('opening','intro')),
 'intro':block(activate('intro','wait_intro')),'wait_intro':block(wait('line','enter')),
 'enter':block(activate('enter','wait_enter')),'wait_enter':block(wait('stage','arrival')),
 'arrival':block(activate('arrival','wait_arrival')),'wait_arrival':block(wait('line','letter')),
 'letter':block(activate('letter','gate')),
 'gate':block({'type':'await','conditions':[{'task':'line','milestone':{'type':'marker','id':'bell'}}],'next':'bell','on_cancelled':'cancelled','on_failed':'failed'}),
 'bell':block(activate('bell','wait_bell')),'wait_bell':block(wait('bell','continue')),
 'continue':block(wait('line','after_gate'),op('continue.letter','dialogue_continue',task='line')),
 'after_gate':block(activate('after_gate','wait_after')),'wait_after':block(wait('line','question')),
 'question':block(activate('question','wait_question')),'wait_question':block(wait('line','choose')),
 'choose':block({'type':'interact','choice':'route','branches':{'walk':'walk_begin','stay':'stay_begin'},'on_empty':'failed'}),
 'walk_begin':block(activate('river','wait_river'),op('affection.walk','assign',target='affection',value={'type':'binary','op':'add','left':ref('affection'),'right':val(1)})),
 'wait_river':block(wait('stage','walk_line')),
 'walk_line':block(activate('walk_line','wait_walk')),'wait_walk':block(wait('line','walk_end')),
 'walk_end':block(activate('walk_end','wait_walk_end')),'wait_walk_end':block(wait('line','end_walk')),
 'end_walk':block(end('walk_home'),op('unlock.walk','profile_merge',key='ending.walk')),
 'stay_begin':block(activate('nod','wait_nod')),'wait_nod':block(wait('nod','stay_line')),
 'stay_line':block(activate('stay_line','wait_stay')),'wait_stay':block(wait('line','stay_end')),
 'stay_end':block(activate('stay_end','wait_stay_end')),'wait_stay_end':block(wait('line','end_stay')),
 'end_stay':block(end('read_letter'),op('unlock.stay','profile_merge',key='ending.stay')),
 'cancelled':block({'type':'fault','code':'E_CANCELLED','message':'A required performance was cancelled.'}),
 'failed':block({'type':'fault','code':'E_PERFORMANCE','message':'A required performance failed.'})
}
fragment={'fragment_format':1,'variables':{'affection':{'type':'i32','value':0}},'functions':{'main':{'entry':'start','blocks':blocks}},'scenes':scenes,'cues':cues,'choices':{'route':{'options':[{'id':'walk','text':'walk'},{'id':'stay','text':'stay'}]}}}
dump(out/'content/ch01/story.nir.json',fragment)
(out/'game.toml').write_text('''project_format = 1
[game]
id = "org.nir.rain-letters"
slug = "rain-letters"
title = "雨后书简 · Rain Letters"
version = "0.1.0"
source_locale = "zh-Hans"
title_scene = "station"
[engine]
api = "nir-player/0.1"
capability_profile = "web-v1"
[stage]
width = 1280
height = 720
[inputs]
modules = ["content/ch01/module.toml"]
asset_catalogs = ["assets/catalog.toml"]
theme = "themes/rain/tokens.json"
scenarios = ["tests/scenarios/walk.toml", "tests/scenarios/stay.toml"]
notices = ["credits/README.md", "credits/FONT-LICENSE.txt"]
''')
(out/'content/ch01/module.toml').write_text('''module_format = 1
id = "ch01"
sources = ["story.nir.json"]
text_contracts = "texts/contracts.json"
text_revisions = "texts/revisions.json"
[exports]
start = "main"
[text_bundles]
zh-Hans = "texts/zh-Hans.json"
en = "texts/en.json"
''')
dump(out/'themes/rain/tokens.json',{'background':[.04,.075,.09,1],'panel':[.06,.105,.12,.97],'accent':[.83,.73,.48,1],'text':[.93,.94,.88,1],'muted':[.58,.69,.69,1]})
for route,outcome,aff in [('walk','walk_home',1),('stay','read_letter',0)]:
 (out/f'tests/scenarios/{route}.toml').write_text(f'''format = 1
id = "{route}"
entry = "main"
text_locale = "zh-Hans"
[[steps]]
action = "await_choice"
id = "route"
[[steps]]
action = "choose"
option_id = "{route}"
[expect]
outcome = "{outcome}"
affection = {aff}
''')
# Original geometric rain-station illustration, deterministic random seed.
for name in ['station','river']:
 rng=random.Random(17);im=Image.new('RGB',(1280,720));px=im.load()
 for y in range(720):
  t=y/720
  for x in range(1280):
   glow=max(0,1-math.hypot((x-880)/800,(y-220)/500))*.35
   px[x,y]=(int(17+17*t+glow*90),int(35+23*t+glow*60),int(45+27*t+glow*20))
 d=ImageDraw.Draw(im,'RGBA')
 for x in range(0,1280,90):
  h=rng.randrange(60,160);d.rectangle((x,270-h,x+75,350),fill=(20,35,42,255))
  for yy in range(280-h,310,22):
   for xx in range(x+8,x+70,18):
    if rng.random()<.4:d.rectangle((xx,yy,xx+5,yy+9),fill=(188,170,114,80))
 d.polygon([(0,430),(1280,365),(1280,720),(0,720)],fill=(18,34,39,255))
 for i in range(60):
  y=rng.randint(445,710);x=rng.randint(0,1280);d.line((x,y,x+rng.randint(15,200),y-3),fill=(124,173,165,rng.randint(12,50)),width=2)
 if name=='station':
  d.polygon([(0,140),(645,170),(740,222),(0,203)],fill=(14,30,35,255));d.line((0,203,737,222),fill=(172,162,126,180),width=3)
  for x in [38,395,697]:d.polygon([(x,210),(x+12,210),(x+18,535),(x+4,537)],fill=(34,57,58,255))
  d.rectangle((76,247,313,438),fill=(44,73,74,255));d.rectangle((93,264,296,414),fill=(130,154,136,130));d.line((194,265,194,414),fill=(25,49,51,255),width=5)
  d.rectangle((320,348,563,361),fill=(111,120,105,255));d.rectangle((340,361,349,415),fill=(40,61,60,255));d.rectangle((545,361,554,413),fill=(40,61,60,255))
  d.rectangle((425,239,591,276),fill=(172,187,160,255));d.line((435,254,576,254),fill=(39,70,68,255),width=3)
 else:
  d.polygon([(240,430),(810,350),(1280,450),(1280,720),(200,720)],fill=(28,61,67,255))
  for i in range(70):
   y=rng.randrange(400,710);x=rng.randrange(290,1250);d.line((x,y,x+rng.randrange(10,100),y),fill=(130,186,166,rng.randrange(12,65)),width=2)
  d.line((0,494,920,356),fill=(57,91,88,255),width=9)
  for x in range(0,850,110):y=int(494-x*.15);d.line((x,y,x,y+96),fill=(52,82,79,255),width=7)
 # streetlamp and halo
 halo=Image.new('RGBA',im.size);hd=ImageDraw.Draw(halo);hd.ellipse((846,157,980,291),fill=(248,216,128,80));halo=halo.filter(ImageFilter.GaussianBlur(26));im=Image.alpha_composite(im.convert('RGBA'),halo);d=ImageDraw.Draw(im,'RGBA')
 d.line((913,226,913,438),fill=(37,55,53,255),width=7);d.rectangle((893,204,933,229),fill=(236,222,157,255));d.polygon([(889,204),(913,192),(938,204)],fill=(37,55,53,255))
 for i in range(320):
  x=rng.randint(0,1280);y=rng.randint(0,720);d.line((x,y,x-6,y+17),fill=(186,214,211,rng.randrange(8,35)),width=1)
 im.convert('RGB').save(out/f'assets/source/{name}.png',optimize=True)
a=Image.new('RGBA',(290,530));d=ImageDraw.Draw(a)
d.ellipse((42,24,249,243),fill=(35,37,43,255));d.polygon([(74,172),(218,172),(260,491),(37,491)],fill=(159,157,137,255));d.polygon([(108,198),(181,198),(202,484),(89,484)],fill=(209,202,174,255));d.ellipse((89,73,206,218),fill=(231,210,183,255));d.polygon([(87,141),(89,83),(172,56),(210,98),(211,154),(167,110),(136,145),(128,116)],fill=(38,38,43,255));d.line((111,153,129,151),fill=(58,53,48,255),width=3);d.line((171,151,188,153),fill=(58,53,48,255),width=3);d.arc((135,170,164,188),0,150,fill=(154,119,102,255),width=2);d.polygon([(81,218),(145,256),(207,215),(183,278),(105,278)],fill=(63,96,91,255));d.polygon([(137,264),(158,264),(173,379),(151,392)],fill=(63,96,91,255));d.rounded_rectangle((105,315,210,382),8,fill=(225,218,188,255));d.line((106,316,158,351,209,316),fill=(159,149,124,255),width=2);d.ellipse((89,339,127,375),fill=(231,210,183,255));d.ellipse((196,339,232,375),fill=(231,210,183,255));d.rectangle((76,486,117,529),fill=(43,48,48,255));d.rectangle((181,486,219,529),fill=(43,48,48,255));a.save(out/'assets/source/aki.png',optimize=True)
for name,secs in [('bgm',8),('bell',.8),('voice',1.2)]:
 rate=24000;buf=[]
 for i in range(int(secs*rate)):
  t=i/rate
  if name=='bgm':v=sum(math.sin(2*math.pi*f*t)*.024 for f in [220,261.625,329.628])*math.sin(math.pi*t/secs)**2
  elif name=='bell':v=(math.sin(2*math.pi*880*t)+.3*math.sin(2*math.pi*1320*t))*.22*math.exp(-t*7)*min(1,t*100)
  else:v=math.sin(2*math.pi*(330+40*math.sin(t*9))*t)*.09*math.sin(math.pi*t/secs)**2
  buf.append(struct.pack('<h',int(v*32767)))
 with wave.open(str(out/f'assets/source/{name}.wav'),'wb') as w:w.setnchannels(1);w.setsampwidth(2);w.setframerate(rate);w.writeframes(b''.join(buf))
# Subset a licensed SC face; preserve shaping closure. Include all UI strings from the source.
ui='开始阅读继续阅读返回设置存档读档回看历史回退保存读取导出导入语言正文界面字号声音音乐语音音效减少动态自动已读快进暂停恢复重试返回标题加载中下一段生效浏览器已保存没有存档读取失败存档冲突正在保存已完成结局雨后书简返回故事关闭选择确认第章首版阅读体验测试合成配音新的开始走回家读完信画面加载失败资源准备中作品测试简体中文英文阅读进度菜单无障碍大字小字当前静音键盘空格推进方向选择取消打开按继续任务文字恢复成功失败删除确认错误准备完成设备丢失正在恢复本地存档存储容量不足最近保存尚未阅读重玩夜雨车站沿河同行留下读信创建属于自己的故事'
ui += ''.join(p.read_text() for p in (ROOT/'crates/nir-presentation').rglob('*.ftl')) + ''.join(p.read_text() for p in (ROOT/'crates/nir-presentation/src').rglob('*.rs'))
font=TTFont('/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc',fontNumber=2)
options=subset.Options();options.layout_features=['*'];sub=subset.Subsetter(options=options);sub.populate(text=''.join(t for pair in lines.values() for t in pair)+ui+'你愿意收下吗？Will you take it?'+''.join(chr(i) for i in range(32,127))+' →←↗•—…–·％１２３４５６７８９０');sub.subset(font);font.save(out/'assets/source/reader.otf')
shutil.copy('/usr/share/licenses/noto-fonts-cjk/LICENSE',out/'credits/FONT-LICENSE.txt')
(out/'credits/README.md').write_text('# 素材来源\n\n背景、角色、音乐和测试提示音由 scripts/make_fixture.py 原创生成，以 CC0-1.0 提供。voice.wav 是合成测试音，不是真人配音。\n\n字体来自 Noto Sans CJK SC，按 SIL Open Font License 1.1 分发；附完整许可。示例字体为保留 OpenType 塑形闭包的子集。新增文字时必须补充字体覆盖。\n')
assets=[]
for id,kind,file in [('bg.station','image','station.png'),('bg.river','image','river.png'),('actor.aki','image','aki.png'),('audio.bgm','audio','bgm.wav'),('audio.bell','audio','bell.wav'),('audio.voice','audio','voice.wav'),('font.reader','font','reader.otf')]:
 s=f'[[assets]]\nid = "{id}"\nkind = "{kind}"\nsource = "source/{file}"\nrights = "'+('OFL-1.1' if kind=='font' else 'CC0-1.0')+'"\n'
 if kind=='image':s+='expected_size = ['+(', '.join(str(n) for n in Image.open(out/f'assets/source/{file}').size))+']\n'
 assets.append(s)
(out/'assets/catalog.toml').write_text('format = 1\n\n'+'\n'.join(assets))
program={k:v for k,v in fragment.items() if k!='fragment_format'};program.update(format=1,game_id='org.nir.rain-letters',revision='fixture-v1',entry='main',stage={'width':1280,'height':720},requires=['control.v1','text.gate.v1','text.revisions.v1'],default_locale='zh-Hans',texts=contracts,locales=locales,assets={id:{'kind':kind,'object':file,'bytes':1,'width':1280 if kind=='image' else 0,'height':720 if kind=='image' else 0} for id,kind,file in [('bg.station','image','station'),('bg.river','image','river'),('actor.aki','image','aki'),('audio.bgm','audio','bgm'),('audio.bell','audio','bell'),('audio.voice','audio','voice'),('font.reader','font','font')]})
def digest(value):return hashlib.sha256(json.dumps(value,ensure_ascii=False,separators=(',',':')).encode()).hexdigest()
ledger={'format':1,'source_locale':'zh-Hans','texts':{}}
for key,c in contracts.items():
 contract_digest=digest([1,c['contract_revision'],c['meaning_revision'],c['params'],c['gates']])
 ledger['texts'][key]={'source_revision':1,'contract_revision':1,'meaning_revision':1,'source_digest':digest(locales['zh-Hans'][key]['spans']),'contract_digest':contract_digest,'shape_digest':digest([c['params'],c['gates']]),'reviewed':{'en':digest(locales['en'][key])}}
 c['contract_digest']=contract_digest
 for docs in locales.values():docs[key]['contract_digest']=contract_digest
dump(out/'content/ch01/texts/revisions.json',ledger)
dump(ROOT/'fixtures/rain.json',program)
