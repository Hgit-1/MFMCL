#!/usr/bin/env python3
"""Build an editable, schema-backed Sketch document and a matching offline SVG preview.
Only design/tools/node_modules schemas are read; no existing launcher source is used.
"""
from __future__ import annotations
import copy
import html
import json
from pathlib import Path
import uuid
import zipfile

ROOT = Path(__file__).resolve().parents[1]
SCHEMAS = Path(__file__).resolve().parent / 'node_modules/@sketch-hq/sketch-file-format/dist'
PS = json.loads((SCHEMAS / 'page.schema.json').read_text())
DS = json.loads((SCHEMAS / 'document.schema.json').read_text())
OUT = ROOT / 'deliverables'
OUT.mkdir(parents=True, exist_ok=True)
COUNT = 0

def uid(label=None):
    global COUNT
    if label is None:
        COUNT += 1
        label = f'layer-{COUNT}'
    return str(uuid.uuid5(uuid.NAMESPACE_URL, 'field-launcher-design/' + label)).upper()

def default(schema, root=PS):
    if '$ref' in schema:
        return default(root['definitions'][schema['$ref'].split('/')[-1]], root)
    if 'const' in schema:
        return schema['const']
    if 'enum' in schema:
        return schema['enum'][0]
    if 'oneOf' in schema:
        return default(schema['oneOf'][0], root)
    if schema.get('title') == 'UUID':
        return uid()
    t = schema.get('type')
    if t == 'object':
        return {k: default(schema['properties'][k], root) for k in schema.get('required', [])}
    if t == 'array':
        return [default(schema['items'], root) for _ in range(schema.get('minItems', 0))]
    if t == 'boolean':
        return False
    if t in ('number', 'integer'):
        return max(schema.get('minimum', 0), 1 if 'exclusiveMinimum' in schema else 0)
    if t == 'string':
        if schema.get('pattern', '').startswith('^{'):
            return '{0, 0}'
        return ''
    return None

def model(name, root=PS):
    return default(root if name == 'Page' else root['definitions'][name], root)

def color(v, a=1):
    v = v.lstrip('#')
    return {'_class': 'color', 'red': int(v[:2],16)/255, 'green':int(v[2:4],16)/255,
            'blue':int(v[4:6],16)/255, 'alpha': a}

def hexcolor(c):
    return '#' + ''.join(f'{round(c[k]*255):02x}' for k in ('red','green','blue'))

LIGHT = dict(bg='#F5F5EF', surface='#FFFFFF', ink='#202E29', muted='#67746D', line='#DFE5DC',
             soft='#EAF0E7', primary='#286044', on='#FFFFFF', mint='#D5EDAD', dark='#18392E',
             amber='#8D5C16', amberbg='#FAEED7', red='#A83D36', redbg='#FBEAE6')
DARK = dict(bg='#141D19', surface='#1D2923', ink='#EDF3E8', muted='#B0BFB3', line='#3A4B40',
            soft='#29382F', primary='#BFE69D', on='#203326', mint='#D5EDAD', dark='#18392E',
            amber='#F1CE86', amberbg='#3E3421', red='#FFB3A8', redbg='#442B29')
C = LIGHT
PAGES = []
BOARDS = []
SYMBOLS = []
SYMBOL_MAP = {}
TEXT_STYLES = {}


def style(fill=None, border=None, opacity=1):
    s = model('Style')
    s['contextSettings'] = {'_class':'graphicsContextSettings','blendMode':0,'opacity':opacity}
    if fill:
        f = model('Fill')
        f['isEnabled'] = True
        f['color'] = color(fill)
        f['contextSettings']['opacity'] = 1
        f['gradient']['from'] = '{0, 0}'
        f['gradient']['to'] = '{1, 1}'
        s['fills'] = [f]
    if border:
        b = model('Border')
        b.update(isEnabled=True, color=color(border), thickness=1, position=1)
        b['contextSettings']['opacity'] = 1
        b['gradient']['from'] = '{0, 0}'
        b['gradient']['to'] = '{1, 1}'
        s['borders'] = [b]
    return s

def base(kind, name, x,y,w,h):
    o = model(kind)
    o.update(name=name, nameIsFixed=True, isVisible=True, resizingConstraint=63,
             frame={'_class':'rect','constrainProportions':False,'x':x,'y':y,'width':w,'height':h})
    if 'hasClickThrough' in o:
        o['hasClickThrough'] = False
    return o

def add(parent, obj):
    parent['layers'].append(obj)
    return obj

def group(parent, name, x=0,y=0,w=1440,h=900):
    return add(parent,base('Group',name,x,y,w,h))

def rect(parent,x,y,w,h,fill=None,r=0,border=None,name='Surface'):
    o = base('Rectangle',name,x,y,w,h)
    o.update(fixedRadius=r, hasConvertedToNewRoundCorners=True, isClosed=True, style=style(fill,border))
    o['points'] = []
    for px,py in [(0,0),(1,0),(1,1),(0,1)]:
        p = model('CurvePoint')
        p.update(cornerRadius=r, point=f'{{{px}, {py}}}', curveFrom=f'{{{px}, {py}}}',curveTo=f'{{{px}, {py}}}')
        o['points'].append(p)
    return add(parent,o)

def path(parent, coords, fill=None, stroke=None, thickness=1.6, closed=False, name='Vector'):
    xs,ys = zip(*coords)
    x,y,w,h = min(xs),min(ys),max(xs)-min(xs),max(ys)-min(ys)
    w,h = max(w,.01),max(h,.01)
    o = base('ShapePath',name,x,y,w,h)
    o.update(isClosed=closed, style=style(fill,stroke))
    if stroke:
        o['style']['borders'][0]['thickness'] = thickness
        o['style']['borderOptions'].update(lineCapStyle=1,lineJoinStyle=1)
    o['points'] = []
    for px,py in coords:
        p = model('CurvePoint')
        p.update(point=f'{{{(px-x)/w:.6f}, {(py-y)/h:.6f}}}',curveFrom='{0, 0}',curveTo='{0, 0}')
        o['points'].append(p)
    return add(parent,o)

def text(parent,x,y,value,size=16,fill=None,w=600,h=None,bold=False,align=0,name=None):
    fill = fill or C['ink']
    lines = str(value).split('\n')
    line = round(size*1.5)
    h = h or line * len(lines) + 4
    o = base('Text',name or str(value).replace('\n',' ')[:64],x,y,w,h)
    font = 'NotoSansCJKsc-Bold' if bold else 'NotoSansCJKsc-Regular'
    attrs = {'MSAttributedStringFontAttribute':{'_class':'fontDescriptor','attributes':{'name':font,'size':size}},
             'MSAttributedStringColorAttribute':color(fill),
             'paragraphStyle':{'_class':'paragraphStyle','alignment':align,'minimumLineHeight':line,'maximumLineHeight':line}}
    o['attributedString'] = {'_class':'attributedString','string':str(value),'attributes':[{
        '_class':'stringAttribute','location':0,'length':len(str(value).encode('utf-16-le'))//2,'attributes':attrs}]}
    o['glyphBounds'] = '{{0, 0}, {' + f'{w}, {h}' + '}}'
    o['textBehaviour'] = 2
    o['style'] = style()
    o['style']['textStyle'] = {'_class':'textStyle','verticalAlignment':0,'encodedAttributes':copy.deepcopy(attrs)}
    key = (size,fill,bold,align)
    if key not in TEXT_STYLES:
        shared = model('SharedStyle',DS)
        shared.update(name=f'Type/{"Bold" if bold else "Regular"}/{size}/{fill}/{align}',value=copy.deepcopy(o['style']))
        TEXT_STYLES[key] = shared
    o['sharedStyleID'] = TEXT_STYLES[key]['do_objectID']
    return add(parent,o)

def link(layer,target):
    layer['flow'] = {'_class':'MSImmutableFlowConnection','destinationArtboardID':uid('board-'+target),'animationType':0}
    return layer

def icon(parent,x,y,name,fill=None,size=20):
    fill = fill or C['muted']
    g = group(parent,'Icon/'+name,x,y,size,size)
    s=size/24
    def line(points,close=False):
        return path(g,[(a*s,b*s) for a,b in points],None,fill,1.6,close)
    if name=='play': path(g,[(7*s,4*s),(20*s,12*s),(7*s,20*s)],fill,None,closed=True)
    elif name=='plus': line([(12,4),(12,20)]);line([(4,12),(20,12)])
    elif name=='arrow': line([(4,12),(20,12)]);line([(14,6),(20,12),(14,18)])
    elif name=='check': line([(4,12),(10,18),(21,5)])
    elif name=='down': line([(12,3),(12,16)]);line([(6,10),(12,16),(18,10)]);line([(4,18),(4,21),(20,21),(20,18)])
    elif name=='search':
        rect(g,2*s,2*s,14*s,14*s,None,7*s,fill,'Lens');line([(15,15),(21,21)])
    elif name=='grid':
        for a,b in [(3,3),(14,3),(3,14),(14,14)]: rect(g,a*s,b*s,7*s,7*s,None,2*s,fill)
    elif name=='folder': line([(2,6),(9,6),(11,8),(22,8),(22,20),(2,20)],True)
    elif name=='chevron': line([(8,5),(15,12),(8,19)])
    elif name=='close': line([(5,5),(19,19)]);line([(19,5),(5,19)])
    elif name=='pause': line([(8,5),(8,19)]);line([(16,5),(16,19)])
    else:
        rect(g,3*s,3*s,18*s,18*s,None,5*s,fill)
        line([(8,12),(16,12)]);line([(12,8),(12,16)])
    return g

def button(parent,x,y,label,w=160,kind='primary',target=None,ico=None,h=48):
    g = group(parent,f'Button/{kind}/{label}',x,y,w,h)
    key=(kind,C['bg'])
    fill = C['primary'] if kind=='primary' else C['surface'] if kind=='secondary' else C['soft'] if kind=='tonal' else C['redbg']
    fg = C['on'] if kind=='primary' else C['red'] if kind=='danger' else C['ink']
    if key not in SYMBOL_MAP:
        master = base('SymbolMaster',f'Button surface/{kind}/{"Dark" if C is DARK else "Light"}',0,0,160,48)
        master.update(symbolID=uid('symbol-'+str(key)), allowsOverrides=True, hasBackgroundColor=False,
                      includeBackgroundColorInInstance=False, backgroundColor=color(C['bg']))
        rect(master,0,0,160,48,fill,12,C['line'] if kind=='secondary' else None,'Container')
        SYMBOLS.append(master)
        SYMBOL_MAP[key]=master
    ins = base('SymbolInstance','Reusable button surface',0,0,w,h)
    ins['symbolID'] = SYMBOL_MAP[key]['symbolID']
    ins['scale']=1
    add(g,ins)
    if ico:
        icon(g,18,(h-20)/2,ico,fg)
        text(g,48,(h-26)/2,label,15,fg,w=w-58,bold=True)
    else:
        text(g,8,(h-26)/2,label,15,fg,w=w-16,bold=True,align=2)
    if target: link(g,target)
    return g

def chip(parent,x,y,label,kind='soft',w=None):
    w=w or max(58,len(label)*12+24)
    bg,fg = (C['amberbg'],C['amber']) if kind=='warn' else (C['redbg'],C['red']) if kind=='error' else (C['soft'],C['primary'])
    g=group(parent,'Badge/'+label,x,y,w,28)
    rect(g,0,0,w,28,bg,8)
    text(g,10,4,label,12,fg,w=w-20,bold=True)
    return g

def panel(parent,x,y,w,h,title=None,sub=None):
    g=group(parent,title or 'Panel',x,y,w,h)
    rect(g,0,0,w,h,C['surface'],18,C['line'])
    if title: text(g,24,21,title,20,w=w-48,bold=True)
    if sub: text(g,24,57,sub,13,C['muted'],w=w-48)
    return g

def field(parent,x,y,label,value,w=460,note=None):
    text(parent,x,y,label,14,bold=True,w=w)
    rect(parent,x,y+30,w,48,C['surface'],10,C['line'],'Field/'+label)
    text(parent,x+16,y+42,value,14,w=w-45)
    if note: text(parent,x,y+86,note,12,C['muted'],w=w)

def progress(parent,x,y,w,value,fill=None):
    rect(parent,x,y,w,7,C['soft'],3)
    rect(parent,x,y,max(7,w*value),7,fill or C['primary'],3)

def brand(parent,x,y,dark=False):
    g=group(parent,'Brand / 原野 FIELD',x,y,160,42)
    rect(g,0,0,36,36,C['primary'],11)
    for bx,by in [(9,10),(18,10),(9,19)]:rect(g,bx,by,8,8,C['on'],1)
    text(g,48,-1,'原野',21,bold=True,w=60)
    text(g,110,8,'FIELD',10,C['muted'],w=50,bold=True)
    return g

def page(name):
    p=base('Page',name,0,0,0,0)
    PAGES.append(p)
    return p

def board(page,key,title,w=1440,h=900,theme=LIGHT):
    global C
    C=theme
    n=len(page['layers'])
    b=base('Artboard',title,(n%3)*1560,(n//3)*1040,w,h)
    b.update(do_objectID=uid('board-'+key), backgroundColor=color(C['bg']),hasBackgroundColor=True,
             includeBackgroundColorInExport=True,isFlowHome=key=='home',resizesContent=False)
    b['userInfo']={'design.field':{'key':key,'theme':'dark' if theme is DARK else 'light','status':'design-prototype'}}
    add(page,b);BOARDS.append(b)
    return b

def shell(b,section,title,subtitle='',action=None):
    w=b['frame']['width']
    rect(b,0,0,w,88,C['surface'])
    rect(b,0,87,w,1,C['line'])
    link(brand(b,40,26),'home')
    nav=[('开始','home'),('我的游戏','library'),('发现内容','discover')]
    for i,(label,target) in enumerate(nav):
        x=254+i*130
        if section==label:rect(b,x-16,21,110,46,C['soft'],12)
        t=text(b,x,33,label,15,C['primary'] if section==label else C['muted'],w=100,bold=section==label)
        link(t,target)
    button(b,w-364,24,'下载任务',124,'tonal','downloads',ico='down',h=40)
    rect(b,w-220,24,36,36,C['mint'],12)
    text(b,w-211,31,'L',15,C['dark'],w=20,bold=True)
    link(text(b,w-174,33,'Lin  /  账户',13,w=130),'account')
    text(b,48,118,title,30,bold=True,w=w-350)
    text(b,48,167,subtitle,14,C['muted'],w=w-240)
    if action:button(b,w-226,128,action[0],178,target=action[1],ico='plus')
    rect(b,48,b['frame']['height']-52,w-96,1,C['line'])
    text(b,48,b['frame']['height']-33,'原野 FIELD  /  Minecraft Java 版',11,C['muted'],w=340)
    text(b,w-350,b['frame']['height']-33,'设计原型 · 数据与状态仅作演示',11,C['muted'],w=302,align=1)

def heading(parent,x,y,kicker,title,sub,w=700):
    text(parent,x,y,kicker,12,C['primary'],w=w,bold=True)
    text(parent,x,y+32,title,32,bold=True,w=w)
    text(parent,x,y+92,sub,15,C['muted'],w=w)

def voxel(parent,x,y,s=1):
    """Original editable isometric landscape, no external image assets."""
    g=group(parent,'Artwork / 原创矢量浮岛',x,y,450*s,300*s)
    def poly(coords,c):path(g,[(a*s,b*s) for a,b in coords],c,closed=True,name='Terrain plane')
    poly([(50,147),(225,50),(400,147),(225,248)],'#819F70')
    poly([(50,147),(225,248),(225,285),(50,184)],'#4A6B50')
    poly([(225,248),(400,147),(400,184),(225,285)],'#2B503F')
    poly([(105,149),(196,98),(284,149),(192,202)],'#A9C78F')
    poly([(207,80),(258,51),(314,81),(262,111)],'#B5D59C')
    poly([(207,80),(262,111),(262,153),(207,122)],'#6E9569')
    poly([(262,111),(314,81),(314,123),(262,153)],'#40684D')
    poly([(290,157),(326,137),(361,157),(325,177)],'#C5E2AE')
    poly([(290,157),(325,177),(325,193),(290,173)],'#688F60')
    poly([(325,177),(361,157),(361,173),(325,193)],'#386349')
    for tx,ty in [(125,115),(313,103),(202,171)]:
        rect(g,tx*s,(ty+10)*s,8*s,37*s,'#655A3F',1)
        poly([(tx-20,ty+10),(tx+4,ty-4),(tx+29,ty+10),(tx+4,ty+24)],'#D5EDAD')
        poly([(tx-20,ty+10),(tx+4,ty+24),(tx+4,ty+43),(tx-20,ty+29)],'#84AD73')
        poly([(tx+4,ty+24),(tx+29,ty+10),(tx+29,ty+29),(tx+4,ty+43)],'#517B50')
    return g

# 00: Direction and editable foundations.
p=page('00 · 设计说明')
b=board(p,'cover','00 / 原野 FIELD · 独立启动器设计',1440,900)
rect(b,0,0,1440,900,'#18392E')
text(b,64,58,'FIELD / 原野',24,'#D5EDAD',bold=True)
text(b,64,160,'把复杂留给启动器。\n把世界交给你。',62,'#F3F6E9',w=880,h=210,bold=True)
text(b,68,414,'Minecraft Java 启动器 · 可编辑体验设计',23,'#CEDBC9',w=780)
text(b,68,474,'从第一次安装，到属于你的模组世界。\n清晰的下一步、安全的默认值、可以撤回的改变。',18,'#CEDBC9',w=750)
voxel(b,877,405,1.05)
button(b,68,620,'进入设计原型',202,target='home',ico='arrow')
text(b,68,782,'ORIGINAL DESIGN  /  2026.10\n独立视觉方案；并未审阅或复刻 PCL 界面。',13,'#CEDBC9',w=800)

b=board(p,'guide','01 / 使用说明与设计边界')
heading(b,56,54,'READ ME FIRST','这是一份设计稿，不是启动器程序。','所有页面由可编辑文字、矢量形状与可复用按钮构成。')
for i,(t,d) in enumerate([
    ('01  直接修改','双击文字修改文案；调整形状颜色、尺寸和圆角。\n图层按页面、区块、控件分组。按钮表面使用共享 Symbol。'),
    ('02  浏览流程','原生 Sketch 已记录主要按钮的画板跳转。\n离线 HTML 预览也可点击；不连接账户或下载游戏。'),
    ('03  继续创作','附带完整 AI 提示词。品牌、颜色与导航均可推翻重做。\n提示词要求 Figma 原生组件、变量和 Auto Layout。'),
    ('04  导入后检查','本文件经过格式结构验证，但不等于每个工具导入实测。\n字体、共享样式、Symbol 和原型连线需在目标工具复核。'),
    ('05  功能边界','画板展示目标体验；复杂状态和服务集成见提示词。\n没有实现下载、授权、Java 管理、诊断或自动恢复功能。'),
    ('06  正式交付','Sketch 为真实 ZIP / JSON 文档，未伪装成 .fig。\nFigma 导入并检查后，可按其支持能力保存本地 .fig。')]):
    x=56+(i%2)*666;y=228+(i//2)*184
    g=panel(b,x,y,640,160,t)
    text(g,24,62,d,14,C['muted'],w=590)

p=page('01 · 设计系统')
b=board(p,'tokens','02 / 色彩与排版')
heading(b,48,40,'FOUNDATIONS','自然，但不把界面做成游戏地图。','留白、清晰排版与少量原创方块地形，构成独立的视觉语言。')
for i,(name,value) in enumerate([('Canvas',C['bg']),('Surface',C['surface']),('Ink',C['ink']),('Primary',C['primary']),('Mint',C['mint']),('Muted',C['muted']),('Warning',C['amber']),('Error',C['red'])]):
    x=48+i*170
    rect(b,x,224,146,94,value,16,C['line'])
    text(b,x,333,name,14,bold=True,w=150);text(b,x,359,value,13,C['muted'],w=150)
for y,size,sample in [(432,40,'清晰的下一步'),(512,30,'进入你的世界'),(576,20,'游戏、内容与环境'),(626,16,'推荐设置已经准备好，你随时可以调整。'),(674,13,'状态与辅助说明，始终保持可读。')]:
    text(b,48,y,sample,size,w=1050,bold=size>=20)
    text(b,1150,y+5,f'{size}px / Noto Sans CJK',13,C['muted'],w=250)
text(b,48,790,'间距  4 / 8 / 12 / 16 / 24 / 32 / 48    ·    圆角  8 / 12 / 18    ·    默认控件高 48',14,C['muted'],w=1300)

b=board(p,'components','03 / 可编辑控件与状态')
heading(b,48,40,'COMPONENTS','让每一次操作都有反馈。','按钮表面为共享 Symbol；标签、表单、进度与反馈均为原生矢量 / 文字。')
for i,(label,kind) in enumerate([('开始游戏','primary'),('查看详情','secondary'),('稍后设置','tonal'),('移除游戏','danger')]):
    button(b,48+i*330,220,label,280,kind,ico='play' if i==0 else None)
    text(b,48+i*330,282,['主要操作','次要操作','弱化操作','危险操作'][i],13,C['muted'],w=280)
field(b,48,354,'游戏名称','我的生存世界',570,'不会影响 Minecraft 版本名称。')
field(b,720,354,'运行内存','自动分配 · 推荐 4 GB',570,'根据模组需求与电脑可用内存建议，不保证帧率。')
for i,(label,k) in enumerate([('准备就绪','soft'),('需要处理','warn'),('下载失败','error')]):chip(b,48+i*195,500,label,k,165)
progress(b,48,576,570,.64);text(b,48,600,'下载中 · 64% · 支持暂停与恢复',14,C['muted'],w=600)
rect(b,720,522,570,116,C['soft'],16)
text(b,744,544,'更改前先创建备份',18,bold=True,w=520)
text(b,744,581,'恢复之前会再次确认，不会静默覆盖你的存档。',14,C['muted'],w=520)
button(b,48,710,'键盘焦点示例',240,'secondary')
rect(b,44,706,248,56,None,16,C['primary'],'Focus ring')
rect(b,360,710,240,48,C['line'],12)
text(b,368,722,'暂不可用',15,C['muted'],w=224,align=2)
text(b,720,716,'状态扩展：悬停 / 按下 / 加载 / 禁用 / 焦点\n完整变体与响应式规则见随附 AI 提示词。',14,C['muted'],w=620)

# 02: Home in two themes, and compact reflow.
p=page('02 · 开始与引导')
for key,theme in [('home',LIGHT),('home-dark',DARK)]:
    b=board(p,key,'04 / 开始 · '+('深色' if theme is DARK else '浅色'),theme=theme)
    shell(b,'开始','今天，想去哪里？','继续上次的冒险，或用一个新世界重新开始。',('添加游戏','create'))
    hero=group(b,'Continue adventure',48,218,1344,352)
    rect(hero,0,0,1344,352,C['dark'],24)
    chip(hero,32,28,'上次游玩',w=94)
    text(hero,32,83,'山间小屋',42,'#F4F7EA',w=700,bold=True)
    text(hero,34,151,'Minecraft 1.21.1  /  Fabric  /  12 个模组',15,'#C8D8C6',w=700)
    text(hero,34,190,'已准备好 · Java 与游戏文件检查通过',14,'#C8D8C6',w=700)
    button(hero,32,265,'开始游戏',198,target='launch',ico='play',kind='tonal')
    link(text(hero,260,280,'管理这个游戏  →',14,'#D5EDAD',w=230),'instance')
    voxel(hero,875,14,.98)
    text(b,48,612,'给下一次冒险一点灵感',21,bold=True,w=600)
    cards=[('一个干净的新世界','原版安装 · 自动准备运行环境','create'),('让游戏更合心意','模组 / 整合包 / 光影 / 资源包','discover'),('把旧世界带过来','导入已有游戏或整合包','import')]
    for i,(t,d,target) in enumerate(cards):
        g=panel(b,48+i*456,658,432,132)
        icon(g,22,23,['plus','grid','folder'][i],C['primary'],25)
        text(g,62,20,t,18,bold=True,w=340)
        text(g,24,77,d,13,C['muted'],w=370)
        link(g,target)

b=board(p,'home-compact','05 / 开始 · 紧凑窗口 1024 × 720',1024,720)
rect(b,0,0,1024,76,C['surface']);link(brand(b,28,23),'home')
for i,(v,target) in enumerate([('开始','home'),('我的游戏','library'),('发现','discover')]):link(text(b,290+i*118,30,v,14,w=100),'home' if i==0 else target)
button(b,824,20,'下载任务',166,'tonal','downloads',h=40)
text(b,32,102,'今天，想去哪里？',28,bold=True,w=650)
text(b,32,150,'紧凑布局保留启动与添加游戏，次要内容顺序下移。',14,C['muted'],w=800)
g=panel(b,32,200,960,302)
text(g,28,31,'继续上次的冒险',13,C['primary'],w=500)
text(g,28,77,'山间小屋',36,bold=True,w=500)
text(g,28,143,'Minecraft 1.21.1 · Fabric · 已就绪',14,C['muted'],w=530)
button(g,28,221,'开始游戏',190,target='launch',ico='play')
voxel(g,610,9,.72)
button(b,32,536,'添加游戏',298,'secondary','create',ico='plus')
button(b,356,536,'发现内容',298,'secondary','discover',ico='grid')
button(b,680,536,'导入游戏',312,'secondary','import',ico='folder')
text(b,32,653,'小窗口示例 · 原生设计稿不等同于自动布局实现',12,C['muted'],w=850)

b=board(p,'welcome','06 / 首次使用 · 欢迎')
brand(b,56,44)
heading(b,88,180,'WELCOME TO FIELD','第一次玩，也能轻松开始。','选一个你想做的事，其余的准备交给启动器。',900)
for i,(t,d,target) in enumerate([('我想开始玩','推荐稳定原版，自动配置 Java 与内存。','create'),('我有整合包','导入 .mrpack 或支持的 ZIP 整合包。','import'),('我已经装过游戏','选择已有游戏文件夹，扫描后再决定导入。','import')]):
    g=panel(b,88+i*426,394,400,224)
    text(g,26,28,f'0{i+1}',13,C['primary'],bold=True,w=200)
    text(g,26,76,t,24,bold=True,w=350)
    text(g,26,124,d,14,C['muted'],w=348,h=65)
    icon(g,344,178,'arrow',C['primary']);link(g,target)
button(b,88,690,'先登录微软账户',240,'secondary','account')
link(text(b,364,704,'先看看，稍后登录  →',14,C['muted'],w=360),'home')
text(b,88,796,'登录用于验证已购买的 Java 版；安装完成不代表拥有游戏授权。',13,C['muted'],w=1150)

# 03: Guided installation, task states.
p=page('03 · 安装与启动')
b=board(p,'create','07 / 安装 · 选择玩法')
shell(b,'我的游戏','你想怎样开始？','不需要先了解技术名词。可以从推荐项开始，以后再调整。')
for i,(tag,title,desc,small,target) in enumerate([
    ('推荐新手','原版 Minecraft','熟悉最纯粹的生存与创造。','自动下载稳定版与所需 Java','configure-vanilla'),
    ('MODDED','加入模组','为游戏添加新的内容或优化体验。','按版本匹配 Fabric / Forge 等加载器','configure'),
    ('现成玩法','安装整合包','直接体验作者搭配好的模组世界。','从内容平台发现，或导入本地整合包','discover')]):
    x=48+i*456
    g=panel(b,x,236,432,402)
    chip(g,26,27,tag,w=110)
    rect(g,26,94,380,100,C['soft'],16)
    icon(g,188,121,['play','grid','folder'][i],C['primary'],44)
    text(g,26,224,title,26,bold=True,w=380)
    text(g,26,273,desc,15,C['muted'],w=380)
    text(g,26,314,small,12,C['muted'],w=380)
    button(g,26,344,'选择这个玩法',380,target=target)
text(b,48,706,'已经有游戏文件？',18,bold=True,w=600)
button(b,48,748,'导入游戏 / 整合包',270,'secondary','import',ico='folder')
link(text(b,358,762,'查看支持的导入格式  →',14,C['muted'],w=700),'import')

b=board(p,'configure','08 / 安装 · 推荐配置')
shell(b,'我的游戏','给你的新游戏起个名字','1  选择玩法     /     2  确认配置     /     3  安装并开始')
g=panel(b,48,228,832,538,'游戏配置','推荐值已经填好，无需修改也可以安装。')
field(g,24,108,'游戏名称','我的新世界',784)
field(g,24,214,'Minecraft 版本','1.21.1 · 示例稳定版',374)
field(g,434,214,'模组加载器','Fabric · 与所选版本匹配',374)
text(g,24,327,'Java 环境',14,bold=True,w=400)
chip(g,24,358,'自动匹配 Java 21',w=186)
text(g,24,406,'安装到独立游戏目录，模组与存档不会混在一起。',14,C['muted'],w=760)
link(text(g,24,458,'高级设置：安装目录 / 加载器版本 / 内存  →',14,C['primary'],w=760),'settings')
g=panel(b,912,228,480,538,'这次会准备什么')
for i,(t,d) in enumerate([('Minecraft 游戏文件','下载原版与资源索引'),('适配的 Java','按游戏版本与系统架构选择'),('Fabric 与依赖','仅安装兼容的组合'),('完整性检查','下载后验证文件，再标记可启动')]):
    icon(g,24,90+i*88,'check',C['primary'])
    text(g,58,85+i*88,t,16,bold=True,w=390)
    text(g,58,117+i*88,d,13,C['muted'],w=390)
button(g,24,458,'检查并安装',432,target='install-review',ico='arrow')

b=board(p,'install-review','09 / 安装 · 确认计划')
shell(b,'我的游戏','已经为你准备好安装计划','这里展示的是示例估算；实际大小与速度以解析结果为准。')
g=panel(b,48,230,1344,477)
text(g,32,28,'我的新世界',27,bold=True,w=800);chip(g,1130,33,'兼容检查通过',w=174)
for i,(a,d) in enumerate([('游戏版本','Minecraft 1.21.1 + Fabric'),('运行环境','Java 21 / 自动管理'),('文件位置','独立目录 · 可在高级设置修改'),('本次下载','约 680 MB（示例）'),('可用空间','42.8 GB（示例） / 安装前再次检查')]):
    y=105+i*61;text(g,32,y,a,14,C['muted'],w=280);text(g,328,y,d,15,w=900)
    if i<4:rect(g,32,y+43,1280,1,C['line'])
text(b,48,746,'安装过程可以暂停；取消时会说明保留哪些缓存、清理哪些临时文件。',14,C['muted'],w=940)
button(b,1120,733,'确认安装',272,target='installing',ico='down')

b=board(p,'installing','10 / 安装 · 正在准备')
shell(b,'我的游戏','正在准备你的新世界','你可以去看别的内容，任务会继续。')
g=panel(b,48,228,890,510,'我的新世界','Minecraft 1.21.1 / Fabric / Java 21')
text(g,28,117,'正在下载游戏资源',28,bold=True,w=800)
text(g,28,169,'423 / 680 MB · 8.4 MB/s · 预计剩余 31 秒（示例）',14,C['muted'],w=800)
progress(g,28,218,834,.62)
for i,(t,d) in enumerate([('下载 Java 与游戏文件','已完成'),('下载游戏资源','进行中 · 62%'),('安装 Fabric','等待前序任务'),('校验并完成配置','等待校验')]):
    y=265+i*45;text(g,28,y,f'0{i+1}   {t}',15,w=550);text(g,650,y,d,13,C['muted'],w=210,align=1)
button(g,28,443,'暂停下载',170,'secondary','paused',ico='pause')
button(g,218,443,'取消安装',170,'tonal','cancel-install')
g=panel(b,970,228,422,510,'接下来就能开始玩了')
voxel(g,22,91,.84)
text(g,24,366,'第一次启动可能需要一点时间。\n启动器会展示当前步骤，不必反复点击。',14,C['muted'],w=374,h=65)
button(g,24,442,'演示：安装完成',374,'secondary','ready')

for key,title,subtitle,tag,action,target in [
    ('paused','下载已暂停','已验证的文件会保留。网络恢复后可继续，不必从头下载。','暂停中','继续下载','installing'),
    ('ready','你的游戏准备好了','安装与文件检查已完成；启动前还会检查账户与运行环境。','准备就绪','开始游戏','launch'),
    ('launch','正在启动山间小屋','已完成环境检查，正在等待游戏窗口出现。','启动中','查看运行状态','running'),
    ('running','游戏正在运行','可以把启动器留在后台；停止游戏前会再次提醒。','运行中','返回我的游戏','library')]:
    b=board(p,key,f'11 / {title}')
    shell(b,'开始',title,subtitle)
    g=panel(b,260,242,920,480)
    chip(g,365,30,tag,w=190)
    voxel(g,257,67,.82)
    text(g,28,323,'山间小屋 · Minecraft 1.21.1 / Fabric',20,w=864,align=2,bold=True)
    button(g,288,392,action,344,target=target,ico='play' if key=='ready' else 'arrow')
    if key=='running':button(b,580,754,'结束游戏…',280,'danger','stop-game')

# 04: Library and management.
p=page('04 · 游戏与内容')
b=board(p,'library','12 / 我的游戏')
shell(b,'我的游戏','属于你的游戏收藏','每个游戏都有独立的模组、配置和存档。',('添加游戏','create'))
rect(b,48,222,820,48,C['surface'],12,C['line']);icon(b,66,236,'search');text(b,98,235,'搜索名称、版本或模组加载器',14,C['muted'],w=700)
chip(b,900,232,'全部  3',w=130);chip(b,1054,232,'最近游玩',w=150)
for i,(title,meta,status) in enumerate([('山间小屋','1.21.1 · Fabric · 12 个模组','准备就绪'),('纯粹创造','1.21.1 · 原版','准备就绪'),('机械工坊','1.20.1 · Forge · 86 个模组','需要更新')]):
    g=panel(b,48+i*456,306,432,450)
    rect(g,14,14,404,190,['#DCE7CF','#E6E1D0','#D4DEDE'][i],12)
    voxel(g,66,8,.67)
    text(g,24,232,title,25,bold=True,w=380)
    text(g,24,284,meta,14,C['muted'],w=380)
    chip(g,24,329,status,'warn' if i==2 else 'soft',w=118)
    button(g,24,382,'开始游戏',240,target='launch',ico='play')
    button(g,280,382,'管理',128,'secondary','instance')

b=board(p,'instance','13 / 游戏详情')
shell(b,'我的游戏','山间小屋','Minecraft 1.21.1 · Fabric · 独立游戏目录')
button(b,1150,128,'开始游戏',242,target='launch',ico='play')
for i,(v,target) in enumerate([('概览','instance'),('模组 12','mods'),('存档与备份','backups'),('运行设置','settings')]):
    button(b,48+i*236,221,v,214,'tonal' if i==0 else 'secondary',target)
g=panel(b,48,310,870,448,'每次回来，都能继续冒险。')
voxel(g,422,74,.89)
text(g,28,108,'最近游玩',13,C['muted'],w=360)
text(g,28,141,'今天 20:14',26,bold=True,w=360)
text(g,28,214,'12 个模组 · 3 个存档\n最近备份：今天 19:52\n运行环境：Java 21 · 自动管理',15,C['muted'],w=390)
button(g,28,353,'打开游戏文件夹',285,'secondary','files-info',ico='folder')
g=panel(b,950,310,442,448,'游戏健康状态')
for i,(a,d) in enumerate([('文件完整','游戏资源已通过检查'),('环境匹配','Java 21 / 建议内存 4 GB'),('2 个模组可更新','先查看兼容性，再决定更新')]):
    icon(g,24,90+i*92,'check',C['primary'])
    text(g,62,85+i*92,a,17,bold=True,w=350)
    text(g,62,119+i*92,d,13,C['muted'],w=350)
button(g,24,353,'查看模组更新',394,'secondary','compatibility')

b=board(p,'discover','14 / 发现内容')
shell(b,'发现内容','为你的世界，发现更多可能','资源与安装权限取决于平台接口、作者许可和游戏兼容性。')
rect(b,48,222,1060,52,C['surface'],12,C['line']);icon(b,66,238,'search');text(b,98,237,'搜索模组、整合包、光影或资源包',15,C['muted'],w=960)
button(b,1132,222,'筛选',260,'secondary','filters',h=52)
for i,v in enumerate(['模组','整合包','光影','资源包','数据包']):
    button(b,48+i*190,300,v,166,'tonal' if i==0 else 'secondary','discover',h=40)
text(b,48,376,'适合「山间小屋」的内容',21,bold=True,w=1000)
text(b,48,415,'当前筛选：Minecraft 1.21.1 / Fabric / Modrinth',13,C['muted'],w=1000)
for i,(title,desc,letter) in enumerate([('森林漫游','更多自然细节，留住探索的惊喜。','F'),('轻快渲染','以兼容为前提，改善渲染体验。','R'),('地图手记','为旅途做标记，记录你的发现。','M')]):
    g=panel(b,48+i*456,468,432,280)
    rect(g,24,24,64,64,C['soft'],16);text(g,24,35,letter,28,C['primary'],w=64,align=2,bold=True)
    chip(g,291,36,'示例内容',w=116)
    text(g,24,113,title,23,bold=True,w=380)
    text(g,24,159,desc,14,C['muted'],w=380)
    text(g,24,207,'Fabric · 1.21.1 · 内容名为演示',12,C['muted'],w=380)
    link(g,'content-detail')
text(b,48,785,'以上名称与数据均为设计样例，不代表已连接平台或可下载的真实项目。',12,C['muted'],w=1300)

b=board(p,'content-detail','15 / 内容详情与安装')
shell(b,'发现内容','森林漫游','示例内容 · 作者、许可、版本与来源由正式接口提供')
g=panel(b,48,228,852,534)
rect(g,24,24,804,244,'#DCE7CF',16);voxel(g,244,7,.78)
text(g,24,303,'给每一次探索，更多自然细节。',25,bold=True,w=804)
text(g,24,358,'示例介绍：为森林与原野添加更丰富的视觉细节。\n正式内容应展示作者介绍、许可、依赖与更新记录。',15,C['muted'],w=804,h=80)
chip(g,24,467,'来源 Modrinth',w=164);chip(g,208,467,'许可：以实际项目为准',w=242)
g=panel(b,932,228,460,534,'安装到哪个游戏？')
field(g,24,94,'目标游戏','山间小屋 · Fabric 1.21.1',412)
text(g,24,206,'匹配的版本',14,C['muted'],w=412)
text(g,24,240,'Fabric / Minecraft 1.21.1',18,bold=True,w=412)
text(g,24,305,'将同时安装必要依赖。\n更改之前会生成恢复点。',14,C['muted'],w=412)
button(g,24,408,'检查依赖并安装',412,target='compatibility',ico='down')
text(g,24,477,'受限资源需按作者要求手动补齐。',12,C['muted'],w=412)

b=board(p,'mods','16 / 模组管理')
shell(b,'我的游戏','山间小屋 / 模组','启用、停用和更新前，先检查依赖与目标游戏版本。',('添加模组','discover'))
chip(b,48,226,'已安装  12',w=144);chip(b,210,226,'可更新  2',w=144);chip(b,372,226,'已停用  1',w=144)
g=panel(b,48,290,1344,462)
for i,(name,desc,tag) in enumerate([('森林漫游','游戏内容 · Fabric 1.21.1','已启用'),('轻快渲染','渲染优化 · Fabric 1.21.1','有更新'),('地图手记','地图工具 · Fabric 1.21.1','已启用'),('前置 API','必要依赖 · 被 3 个模组使用','已启用'),('旧版实验模组','与当前游戏版本不匹配','已停用')]):
    y=18+i*86;rect(g,24,y+10,48,48,C['soft'],12);text(g,88,y+4,name,17,bold=True,w=700)
    text(g,88,y+37,desc,13,C['muted'],w=800);chip(g,1000,y+16,tag,'warn' if tag=='有更新' else 'soft',w=114)
    link(icon(g,1254,y+20,'chevron'),'compatibility')
    if i<4:rect(g,24,y+79,1296,1,C['line'])
button(b,1048,775,'检查更新与兼容性',344,'secondary','compatibility')

b=board(p,'compatibility','17 / 兼容性与变更确认')
shell(b,'我的游戏','先检查，再更改','兼容结果来自版本声明与已知规则；不保证所有模组在运行时完全兼容。')
g=panel(b,48,228,1344,504,'本次计划')
for i,(title,desc,status) in enumerate([('添加「森林漫游」','Fabric 1.21.1 · 与目标游戏匹配','可安装'),('补齐必要依赖','增加前置 API · 1 个项目','自动补齐'),('「旧版实验模组」不匹配','需要 Minecraft 1.20.1；保留停用，不自动删除','需要处理')]):
    y=90+i*100;text(g,28,y,title,20,bold=True,w=1000);text(g,28,y+40,desc,14,C['muted'],w=1000)
    chip(g,1122,y+14,status,'warn' if i==2 else 'soft',w=190)
rect(g,28,405,1288,64,C['soft'],12);text(g,48,426,'将先创建恢复点，再安装兼容项。失败时尝试恢复，并展示不能恢复的部分。',14,w=1220)
button(b,48,767,'返回调整',228,'secondary','mods')
button(b,1050,767,'确认安装兼容项',342,target='installing',ico='check')

# 05: Supporting and failure journeys.
p=page('05 · 管理与故障恢复')
def generic(key,title,subtitle,items,action, target,section='我的游戏',note=None):
    b=board(p,key,title)
    shell(b,section,title,subtitle)
    g=panel(b,48,230,1344,490)
    for i,(a,d,tag) in enumerate(items):
        y=26+i*108
        rect(g,24,y+8,52,52,C['soft'],14)
        icon(g,40,y+24,['check','folder','down','grid'][i%4],C['primary'])
        text(g,100,y+3,a,20,bold=True,w=1050)
        text(g,100,y+43,d,14,C['muted'],w=1060)
        if tag:chip(g,1090,y+10,tag,'warn' if '需' in tag else 'soft',w=220)
        if i<len(items)-1:rect(g,100,y+91,1210,1,C['line'])
    if note:text(b,48,747,note,13,C['muted'],w=900,h=60)
    button(b,1048,764,action,344,target=target,ico='arrow')
    return b

b=generic('import','18 / 导入游戏与整合包','先解析清单，确认来源和变更，再开始安装。',[
    ('选择本地文件','支持 .mrpack 与已实现规范的整合包 ZIP；不把所有 ZIP 都当整合包。','选择后校验'),
    ('或者选择已有游戏目录','扫描版本、模组和存档；原目录默认保留，不直接搬走文件。','只读扫描'),
    ('查看导入计划','展示游戏版本、加载器、依赖、下载来源及空间占用。','确认后安装'),
    ('受限文件需手动补齐','遵守平台 API、作者分发权限与许可，不绕过下载限制。','需要时提示')],
    '演示：查看导入计划','install-review',note='实际工具应提供文件选择器与拖放。本预览按钮仅演示后续画板。')

b=generic('downloads','19 / 下载任务','统一查看安装、资源下载与修复。示例数据不代表真实下载。',[
    ('我的新世界 · 安装中','Minecraft 资源 · 423 / 680 MB · 8.4 MB/s','进行中 62%'),
    ('机械工坊 · 等待网络','已保留校验通过的文件，恢复网络后可以继续。','需要重试'),
    ('地图手记 · 安装完成','依赖已准备好，目标游戏文件已校验。','已完成')],
    '查看正在安装的游戏','installing',section='开始')
button(b,48,764,'处理失败任务',270,'secondary','download-error')

b=generic('download-error','20 / 下载失败 · 可恢复','部分资源未能下载。已完成的文件会保留，不需要重新安装整个游戏。',[
    ('网络连接中断','先检查网络，再继续未完成的下载。','可重试'),
    ('校验失败的文件','仅重新下载校验不通过的文件，不把损坏文件标为就绪。','需重新下载'),
    ('排查仍未解决的问题','查看失败项和已脱敏日志；切换来源时保留许可与完整性校验。','查看详情')],
    '重新尝试未完成任务','installing',note='重试不是自动修复承诺。持续失败时，应明确展示下一步与可导出的诊断信息。')

b=generic('backups','21 / 存档与备份','备份按游戏隔离；恢复前核对世界名称、时间和版本。',[
    ('山间小屋 / 主存档','最近游玩：今天 20:14 · 本地存档 · 示例 146 MB','可创建备份'),
    ('自动备份 · 今天 19:52','更新模组前创建 · 完整性已检查 · 示例 142 MB','可恢复'),
    ('手动备份 · 昨天 22:08','由你手动创建 · 示例 138 MB','可恢复')],
    '查看恢复确认','restore',note='运行中的世界不能直接覆盖恢复；先退出游戏，再备份当前状态。')
button(b,48,764,'演示：创建备份',270,'secondary','backup-created')

b=generic('restore','22 / 恢复备份 · 二次确认','恢复会改变当前存档，请先确认你选对了世界。',[
    ('将恢复：山间小屋','备份时间：今天 19:52 · 对应 Minecraft 1.21.1','核对目标'),
    ('先保留当前存档','恢复前创建一份当前状态备份；可用空间不足时阻止继续。','自动备份'),
    ('确认游戏已退出','游戏运行中禁止替换存档；不会默默终止游戏或覆盖文件。','需检查')],
    '演示：确认恢复后的状态','restore-done',note='正式实现应在这里提供明确的「取消」与「确认恢复」按钮，而不是静默覆盖。')
button(b,48,764,'取消，保留当前存档',320,'secondary','backups')

b=generic('crash','23 / 游戏没有正常启动','先保护你的存档，再一步一步排查；诊断结果需要验证。',[
    ('可能与新加入的模组有关','最近变更：添加森林漫游。根据日志与变更记录给出候选原因。','诊断建议'),
    ('可以先恢复上次配置','恢复前展示影响范围。不能保证自动修复所有崩溃。','推荐下一步'),
    ('需要更多帮助？','导出日志前先预览并隐藏令牌、用户名与本地路径等敏感信息。','脱敏后分享')],
    '查看恢复方案','compatibility',note='未经验证的原因不得写成确定结论。高级用户可以打开完整日志。')
button(b,48,764,'演示：查看脱敏日志',300,'secondary','logs')

b=generic('servers','24 / 服务器收藏','只管理服务器连接信息；不宣称自带免费穿透或绕过服务器认证。',[
    ('朋友的生存服','play.example.test · Java 版 · 版本 1.21.1（示例）','状态未验证'),
    ('检查客户端兼容性','使用同一游戏版本与服务端要求的模组组合。','连接前检查'),
    ('账户与网络要求','服务器是否需要正版认证、白名单或额外配置，由服务器决定。','由服务端决定')],
    '演示：选择游戏再启动','library',section='开始',note='域名为保留示例域名，不连接任何实际服务器。')

# Account and personalization.
p=page('06 · 账户与个性化')
b=board(p,'account','25 / 账户 · 微软授权')
shell(b,'开始','登录你的 Minecraft 账户','使用微软官方授权页面，启动器不收集或保存你的微软密码。')
g=panel(b,48,230,762,510,'安全地连接账户')
text(g,28,110,'1   在浏览器打开微软官方授权页\n\n2   完成登录与授权\n\n3   回到这里，检查 Java 版所有权',20,w=700,h=230)
button(g,28,414,'演示：前往外部授权',440,target='auth-wait',ico='arrow')
g=panel(b,842,230,550,510,'登录前，你应该知道')
text(g,28,94,'• 授权成功不等于拥有 Java 版。\n\n• 账户过期时会请你重新授权。\n\n• 无所有权、未成年限制和网络失败\n  都需要独立说明与下一步。\n\n• 离线仅用于已合法获取的本地内容，\n  不绕过游戏所有权或服务器认证。',16,C['muted'],w=492,h=340)

b=generic('auth-wait','26 / 等待你在浏览器完成授权','这里不会要求你输入微软密码，也不会显示或记录访问令牌。',[
    ('完成官方授权','确认浏览器地址属于官方授权服务，再按提示继续。','等待中'),
    ('回到启动器','验证授权结果，并单独检查 Minecraft Java 版所有权。','完成后检查'),
    ('授权出错或超时？','可以重新发起或取消；不反复弹出多个授权窗口。','可重新尝试')],
    '演示：查看已登录账户','account-ready',section='开始')
button(b,48,764,'取消授权',220,'secondary','account')

b=board(p,'personalize','27 / 个性化 · 即时预览')
shell(b,'开始','让这里，更像你','外观可以自由变化，操作的清晰度始终保留。')
g=panel(b,48,230,496,548,'外观设置')
text(g,24,91,'主题模式',14,bold=True,w=440)
for i,(v,target) in enumerate([('浅色','home'),('深色','home-dark'),('跟随系统','personalize')]):button(g,24+i*150,127,v,138,'tonal' if i==0 else 'secondary',target,h=40)
text(g,24,211,'强调色',14,bold=True,w=440)
for i,c in enumerate(['#286044','#3368A2','#72519B','#A36530']):rect(g,24+i*80,248,56,44,c,12)
text(g,24,329,'背景与可读性',14,bold=True,w=440)
text(g,24,367,'支持自定义背景；自动叠加文字衬底。\n透明度过低时提示，始终保留清晰焦点。',14,C['muted'],w=440)
text(g,24,449,'动效：跟随系统的减少动态效果偏好',13,C['muted'],w=440)
button(g,24,486,'恢复默认外观',448,'secondary','home')
g=panel(b,576,230,816,548,'实时预览 · 示例')
rect(g,24,94,768,254,C['dark'],20)
text(g,48,123,'山间小屋',32,'#F3F6E9',bold=True,w=450)
text(g,48,180,'Minecraft 1.21.1 · Fabric',14,'#CADCC7',w=450)
button(g,48,259,'开始游戏',230,'tonal','launch',ico='play')
voxel(g,480,101,.61)
text(g,24,391,'可读性优先，而不是无限透明。',22,bold=True,w=760)
text(g,24,444,'完整主题变量与其他创意方向交给后续 AI 继续探索。\n当前原野方案只是可修改的起点，不是固定品牌规范。',14,C['muted'],w=760)

b=board(p,'settings','28 / 设置 · 推荐与高级分离')
shell(b,'我的游戏','运行与下载设置','常用设置优先，高风险选项默认收起并解释影响。')
g=panel(b,48,230,832,550,'推荐设置')
field(g,24,94,'Java 环境','自动匹配 · 当前 Java 21',784,'正式安装时核对游戏版本、系统与架构。')
field(g,24,230,'游戏内存','自动分配 · 推荐 4 GB',784,'保留系统可用内存；大型整合包按需求重新建议。')
field(g,24,364,'游戏文件夹','每个游戏独立存放',784,'迁移目录前先检查空间、冲突与现有存档。')
g=panel(b,912,230,480,550,'下载与高级选项')
for i,(t,d) in enumerate([('下载来源','优先合法、可用且能通过校验的来源。'),('代理与连接','跟随系统；错误时提供恢复默认选项。'),('启动参数','高级选项；错误参数可能导致无法启动。'),('诊断日志','导出前预览与脱敏，不包含访问令牌。')]):
    y=90+i*95;text(g,24,y,t,17,bold=True,w=432);text(g,24,y+36,d,13,C['muted'],w=432)
button(g,24,474,'演示：恢复推荐设置',432,'secondary','instance')

# Compact auxiliary states, represented by actual dedicated artboards.
p=page('07 · 补充状态与确认')
aux=[
 ('empty','还没有自己的游戏','从一个推荐原版开始，或把已有的整合包带过来。','添加第一个游戏','create'),
 ('offline','当前处于离线状态','在线资源和授权检查暂不可用。仅可使用符合授权条件的已安装本地内容。','查看本地游戏','library'),
 ('cancel-install','要取消这次安装吗？','仅清理本次临时文件；已验证的下载缓存可以保留。已有游戏与存档不受影响。','演示：确认取消','empty'),
 ('stop-game','要结束正在运行的游戏吗？','请优先在游戏内保存并退出。强制结束可能丢失未保存进度。','返回游戏运行状态','running'),
 ('backup-created','备份已创建','已保存「山间小屋」的示例恢复点。备份耗时和完整性检查应以实际结果为准。','返回备份列表','backups'),
 ('restore-done','存档恢复完成','示例状态：恢复前的存档已另存，恢复后的文件已通过检查。','查看我的存档','backups'),
 ('account-ready','账户已经连接','Lin · 已验证 Java 版所有权（仅为设计演示状态，不是真实授权结果）。','返回开始','home'),
 ('files-info','打开游戏文件夹','正式应用在这里调用系统文件管理器；设计预览不会访问你的本地游戏文件。','返回游戏详情','instance'),
 ('filters','筛选兼容内容','目标：山间小屋 / Minecraft 1.21.1 / Fabric。切换来源时保留许可与版本信息。','应用示例筛选','discover'),
 ('logs','诊断日志 · 脱敏预览','示例：移除访问令牌、用户名与本地路径。用户确认后才分享，完整日志本地查看。','返回故障诊断','crash'),
 ('delete','移除游戏前，请确认','默认只移除启动器中的记录。删除文件、存档和备份必须分别明确选择与确认。','取消，保留游戏','library')
]
for key,title,sub,act,target in aux:
    b=board(p,key,'状态 / '+title)
    shell(b,'开始',title,sub)
    g=panel(b,280,258,880,430)
    icon(g,408,36,'folder' if key=='empty' else 'check',C['primary'],52)
    text(g,40,128,title,28,bold=True,w=800,align=2)
    text(g,70,204,sub,15,C['muted'],w=740,h=86,align=2)
    button(g,230,328,act,420,target=target,kind='danger' if key=='cancel-install' else 'primary')
    if key=='cancel-install':button(b,568,737,'继续安装，不取消',304,'secondary','installing')

# Make distinct original-game and modded paths, with consistent sample instance names.
def all_layers(obj):
    yield obj
    for child in obj.get('layers', []):
        yield from all_layers(child)

def rewrite_board(b, replacements, routes=None):
    for o in all_layers(b):
        for a,z in replacements:
            o['name']=o['name'].replace(a,z)
        if o['_class']=='text':
            s=o['attributedString']['string']
            for a,z in replacements:s=s.replace(a,z)
            o['attributedString']['string']=s
            o['attributedString']['attributes'][0]['length']=len(s.encode('utf-16-le'))//2
        if 'flow' in o and routes:
            for a,z in routes.items():
                if o['flow']['destinationArtboardID']==uid('board-'+a):
                    o['flow']['destinationArtboardID']=uid('board-'+z)
                    break

originals={b['userInfo']['design.field']['key']:b for b in BOARDS}
for k in ['ready','paused']:
    rewrite_board(originals[k],[('山间小屋','我的新世界')],{'launch':'launch-new'})

def clone_board(source,key,replacements,routes,page_target=None):
    src=originals[source]
    host=page_target or next(p for p in PAGES if src in p['layers'])
    b=copy.deepcopy(src)
    for o in all_layers(b):o['do_objectID']=uid()
    n=len(host['layers'])
    b['frame'].update(x=(n%3)*1560,y=(n//3)*1040)
    b['do_objectID']=uid('board-'+key)
    b['name']=src['name']+' / '+('原版' if 'vanilla' in key else '新安装的游戏')
    b['isFlowHome']=False
    b['userInfo']['design.field']['key']=key
    rewrite_board(b,replacements,routes)
    host['layers'].append(b);BOARDS.append(b)
    return b

clone_board('launch','launch-new',[('山间小屋','我的新世界')],{'running':'running-new'})
clone_board('running','running-new',[('山间小屋','我的新世界')],{'stop-game':'library'})
vanilla_routes={k:k+'-vanilla' for k in ['configure','install-review','installing','paused','ready']}
vanilla_routes['launch']='launch-new-vanilla'
vanilla_routes['launch-new']='launch-new-vanilla'
vanilla_replacements=[('Fabric · 与所选版本匹配','无 · 原版不需要加载器'),('Fabric 与依赖','原版配置'),('仅安装兼容的组合','保持原版，不添加模组加载器'),('Minecraft 1.21.1 + Fabric','Minecraft 1.21.1 · 原版'),('安装 Fabric','写入原版配置'),('Fabric','原版')]
for source in ['configure','install-review','installing','paused','ready']:
    clone_board(source,source+'-vanilla',vanilla_replacements,vanilla_routes)
clone_board('launch','launch-new-vanilla',[('山间小屋','我的新世界'),('Fabric','原版')],{'running':'running-new-vanilla'})
clone_board('running','running-new-vanilla',[('山间小屋','我的新世界'),('Fabric','原版')],{'stop-game':'library'})

# A genuine symbol page, keeping masters outside artboards.
p=page('08 · 共享按钮 Symbol')
for i,m in enumerate(SYMBOLS):
    m['frame']['x']=(i%4)*260
    m['frame']['y']=(i//4)*150
    add(p,m)

# Assemble document, named shared styles, colors and metadata.
doc=default(DS,DS)
doc['do_objectID']=uid('document')
doc['colorSpace']=1
for theme_name,theme in [('Light',LIGHT),('Dark',DARK)]:
    for k,v in theme.items():
        a=model('ColorAsset',DS);a.update(name=f'{theme_name}/{k}',color=color(v));doc['assets']['colorAssets'].append(a)
doc['layerTextStyles']['objects']=list(TEXT_STYLES.values())
doc['pages']=[{'_class':'MSJSONFileReference','_ref_class':'MSImmutablePage','_ref':'pages/'+p['do_objectID']} for p in PAGES]
meta={'commit':'generated-independent-design','pagesAndArtboards':{p['do_objectID']:{'name':p['name'],'artboards':{b['do_objectID']:{'name':b['name']} for b in p['layers'] if b['_class']=='artboard'}} for p in PAGES},
      'version':146,'compatibilityVersion':99,'app':'com.bohemiancoding.sketch3','autosaved':0,'variant':'NONAPPSTORE',
      'created':{'commit':'generated-independent-design','appVersion':'schema-generated','build':0,'app':'com.bohemiancoding.sketch3','compatibilityVersion':99,'version':146,'variant':'NONAPPSTORE'},
      'saveHistory':[],'appVersion':'schema-generated','build':0}
user={'document':{'pageListHeight':180,'pageListCollapsed':0}}
for p in PAGES:user[p['do_objectID']]={'scrollOrigin':'{0, 0}','zoomValue':.5}

# Render the SAME native layer tree into SVG; shared symbols are expanded for preview only.
by_symbol={s['symbolID']:s for s in SYMBOLS}
key_by_id={b['do_objectID']:b['userInfo']['design.field']['key'] for b in BOARDS}

def render(o):
    f=o['frame']; x,y,w,h=(f[k] for k in ('x','y','width','height'))
    cls=o['_class']; s=o.get('style',{})
    fills=s.get('fills',[]); borders=s.get('borders',[])
    fc=hexcolor(fills[0]['color']) if fills else 'none'
    bc=hexcolor(borders[0]['color']) if borders else 'none'
    bw=borders[0]['thickness'] if borders else 0
    body=''
    if cls=='rectangle':body=f'<rect width="{w}" height="{h}" rx="{o["fixedRadius"]}" fill="{fc}" stroke="{bc}" stroke-width="{bw}"/>'
    elif cls=='shapePath':
        pts=[]
        for p in o['points']:
            a,b=map(float,p['point'].strip('{}').split(','));pts.append(f'{a*w},{b*h}')
        tag='polygon' if o['isClosed'] else 'polyline'
        body=f'<{tag} points="{" ".join(pts)}" fill="{fc}" stroke="{bc}" stroke-width="{bw}" stroke-linecap="round" stroke-linejoin="round"/>'
    elif cls=='text':
        a=o['attributedString']['attributes'][0]['attributes'];fs=a['MSAttributedStringFontAttribute']['attributes']['size']
        c=hexcolor(a['MSAttributedStringColorAttribute']);align=a['paragraphStyle'].get('alignment',0)
        tx=w/2 if align==2 else w if align==1 else 0
        anchor='middle' if align==2 else 'end' if align==1 else 'start'
        weight=700 if 'Bold' in a['MSAttributedStringFontAttribute']['attributes']['name'] else 400
        lh=a['paragraphStyle']['minimumLineHeight']
        body=f'<text fill="{c}" font-size="{fs}" font-weight="{weight}" text-anchor="{anchor}" font-family="Noto Sans CJK SC, Noto Sans SC, sans-serif">'
        for i,line in enumerate(o['attributedString']['string'].split('\n')):
            body+=f'<tspan x="{tx}" y="{fs+2+i*lh}">{html.escape(line)}</tspan>'
        body+='</text>'
    elif cls=='symbolInstance':
        m=by_symbol[o['symbolID']]
        body=f'<g transform="scale({w/m["frame"]["width"]} {h/m["frame"]["height"]})">'+''.join(render(c) for c in m['layers'])+'</g>'
    else:body=''.join(render(c) for c in o.get('layers',[]))
    if 'flow' in o:
        target=key_by_id[o['flow']['destinationArtboardID']]
        body=f'<a href="#{target}" aria-label="{html.escape(o["name"],quote=True)}">{body}<rect width="{w}" height="{h}" fill="transparent"/></a>'
    return f'<g transform="translate({x} {y})">{body}</g>'

sections=[]
(OUT/'screens').mkdir(exist_ok=True)
for b in BOARDS:
    key=b['userInfo']['design.field']['key'];w=b['frame']['width'];h=b['frame']['height']
    svg=f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}" role="img" aria-label="{html.escape(b["name"],quote=True)}"><rect width="{w}" height="{h}" fill="{hexcolor(b["backgroundColor"])}"/>'+''.join(render(o) for o in b['layers'])+'</svg>'
    (OUT/'screens'/f'{key}.svg').write_text(svg)
    sections.append(f'<section id="{key}" class="screen" data-title="{html.escape(b["name"],quote=True)}">{svg}</section>')
opts=''.join(f'<option value="{b["userInfo"]["design.field"]["key"]}">{html.escape(b["name"])}</option>' for b in BOARDS)
html_doc='''<!doctype html><html lang="zh-CN"><meta charset="UTF-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>原野 FIELD · 设计预览</title><style>
*{box-sizing:border-box}body{margin:0;background:#e5e8e1;color:#25372d;font-family:"Noto Sans CJK SC",sans-serif}header{height:64px;padding:12px 24px;display:flex;align-items:center;gap:18px;background:#fff;border-bottom:1px solid #d5ddd2}header strong{font-size:15px}header span{font-size:12px;color:#627168}select,button{font:inherit;border:1px solid #d6ded2;background:white;border-radius:8px;padding:8px 12px;color:#25372d;cursor:pointer}select{max-width:400px;margin-left:auto;font-size:13px}main{padding:24px}.screen{display:none;max-width:1440px;margin:auto;border-radius:12px;overflow:hidden;box-shadow:0 14px 50px #203b2912}.screen.active{display:block}.screen>svg{display:block;width:100%;height:auto}a{cursor:pointer}a:focus-visible>rect{stroke:#599a39;stroke-width:3}body.clean header{display:none}body.clean main{padding:0}.clean .screen{border-radius:0;box-shadow:none}footer{text-align:center;font-size:12px;padding:0 20px 20px;color:#617064}
</style><header><strong>原野 FIELD</strong><span>可点击的设计演示 · 非启动器程序</span><button id="back">返回上一页</button><select aria-label="选择画板" id="boards">'''+opts+'''</select></header><main>'''+''.join(sections)+'''</main><footer>文字与矢量的离线预览 · 下载、登录、文件操作均为画板演示，不会执行</footer><script>
const select=document.querySelector('#boards');function show(){let key=location.hash.slice(1)||'home';let node=document.getElementById(key);if(!node||!node.classList.contains('screen')){key='home';node=document.getElementById(key)}document.querySelectorAll('.screen').forEach(s=>s.classList.toggle('active',s===node));select.value=key;document.title=node.dataset.title+' · 原野';}select.onchange=()=>location.hash=select.value;document.getElementById('back').onclick=()=>history.back();window.addEventListener('hashchange',show);if(new URLSearchParams(location.search).has('clean'))document.body.classList.add('clean');show();
</script></html>'''
(OUT/'preview.html').write_text(html_doc)

native={'document.json':doc,'meta.json':meta,'user.json':user}
for p in PAGES:native['pages/'+p['do_objectID']+'.json']=p
with zipfile.ZipFile(OUT/'FIELD-Launcher.sketch','w',zipfile.ZIP_DEFLATED) as z:
    for name,value in native.items():z.writestr(name,json.dumps(value,ensure_ascii=False,separators=(',',':')))
    preview=OUT/'previews/home.png'
    if preview.exists():z.write(preview,'previews/preview.png')
# Plain validation input allows a specialized JS validator to check every ZIP JSON entry.
(OUT/'native-document.json').write_text(json.dumps(native,ensure_ascii=False))
manifest={'title':'原野 FIELD / Minecraft Java 启动器设计','format':'Sketch ZIP/JSON, document version 121',
          'board_count':len(BOARDS),'page_count':len(PAGES),'symbol_count':len(SYMBOLS),'text_style_count':len(TEXT_STYLES),
          'artboards':[{'key':b['userInfo']['design.field']['key'],'name':b['name'],'width':b['frame']['width'],'height':b['frame']['height']} for b in BOARDS],
          'limits':['Design only; no launcher backend','No Figma native .fig exported','No target-editor import verified','Fixed layout, not Figma Auto Layout','Prototype transitions only; no real service requests'],
          'source_policy':'Original geometry and text; PCL source and UI not read.'}
(ROOT/'manifest.json').write_text(json.dumps(manifest,ensure_ascii=False,indent=2))
(ROOT/'tokens.json').write_text(json.dumps({'light':LIGHT,'dark':DARK,'spacing':[4,8,12,16,24,32,48],'radii':[8,12,18],'font':'Noto Sans CJK SC','controlHeight':48},indent=2))
print(f'Generated {len(BOARDS)} editable artboards, {len(PAGES)} pages, {len(SYMBOLS)} symbols, {len(TEXT_STYLES)} shared text styles.')
