#!/usr/bin/env python3
"""Original MIT material assets for the bounded Channel terminal experiment.
No text, screenshots, downloaded art or runtime generation. Requires Pillow.
Reproduce: /usr/bin/python3 scripts/build_channel_materials.py
"""
import base64
import io
import math
from pathlib import Path
from PIL import Image, ImageDraw, ImageFilter

ROOT = Path(__file__).resolve().parents[1]
ASSETS = []

def save(name, im, cols, rows):
    stream = io.BytesIO()
    im.save(stream, format='PNG', optimize=True)
    ASSETS.append((name, cols, rows, base64.b64encode(stream.getvalue()).decode()))

def surface(w, h, kind='metal', radius=9):
    # Directional illumination from upper left; smooth specular reflection.
    im = Image.new('RGBA', (w, h)); pixels = im.load()
    for y in range(h):
        t = y / max(h-1, 1)
        for x in range(w):
            u = x / max(w-1, 1)
            if kind == 'red':
                v = 0.72 + .50*math.exp(-((t-.22)/.18)**2) - .3*t - .13*u
                rgb = [min(255, int(c*v)) for c in (155, 32, 40)]
            elif kind == 'well':
                v = 8 + 10*t + 4*u
                rgb = [int(v*.65), int(v*.87), int(v)]
            elif kind in ('pressed', 'alarm'):
                v = 30 + 24*t + 7*u
                rgb = [int(v*f) for f in ((2.0,.38,.44) if kind=='alarm' else (.75,1.0,1.13))]
            else:
                v = 160 + 32*math.exp(-((t-.16)/.20)**2)-22*t-10*u
                # Fine deterministic brushing, subpixel-scale amplitude only.
                v += .8*math.sin(y*2.4)
                rgb = [int(v*.91),int(v*.96),int(v)]
            pixels[x,y]=tuple(rgb)+(255,)
    bottom=h-1 if h==24 else h-4
    mask=Image.new('L',(w,h));d=ImageDraw.Draw(mask)
    d.rounded_rectangle((1,1,w-3,bottom),radius=radius,fill=255)
    im.putalpha(mask)
    d=ImageDraw.Draw(im)
    recessed=kind in ('well','pressed','alarm')
    # Multiple tight bevels, consistent top-left highlight / bottom-right shadow.
    d.rounded_rectangle((1,1,w-3,bottom),radius=radius,outline=(5,8,12,255),width=2)
    d.line((radius,2,w-radius-3,2),fill=(9,13,18,255) if recessed else (223,231,235,255),width=2)
    d.line((2,radius,2,bottom-radius),fill=(20,25,31,255) if recessed else (180,192,201,255),width=1)
    d.line((radius,bottom,w-radius-3,bottom),fill=(157,172,183,255) if recessed else (37,44,52,255),width=2)
    d.line((w-3,radius,w-3,bottom-radius),fill=(89,101,112,255) if recessed else (27,33,40,255),width=1)
    return im

for name,c,r in [('eq',78,44),('comp',58,44),('output',18,44)]:
    im=surface(c*12,r*24)
    save(name,im,c,r)
save('header',surface(1920,48,'red',8),160,2)
# Existing button widths are a finite semantic layout, not product capacity caps.
for c in (18,):
    for state in ('metal','pressed','alarm'):
        save(f'button_{state}_{c}',surface(c*12,24,state,6),c,1)
# One-row numerical wells, plus chart, GR and output display.
for c,r in [(c,1) for c in range(1,85)]+[(74,19),(14,19)]:
    save(f'well_{c}_{r}',surface(c*12,r*24,'well',min(7,r*4)),c,r)
# Smooth round pots: analytic concentric machined surfaces, antialiased at 2x.
for step in range(17):
    scale=2;size=72*scale;im=Image.new('RGBA',(size,size));p=im.load()
    for y in range(size):
        for x in range(size):
            dx=(x/scale-35.5);dy=(y/scale-34);rad=math.hypot(dx,dy)
            shadow=math.hypot(dx-1.5,dy-3)
            if shadow<34:p[x,y]=(5,8,12,round(240*min(1,(34-shadow)/3)))
            if rad>31.5:continue
            angle=math.atan2(dy,dx);light=(-dx-dy)/max(rad,1)/1.414
            if rad>29: v=91+78*light
            elif rad>27:v=204+42*light
            elif rad>25:v=38+18*light
            elif rad>23:v=175+55*light+14*math.cos(angle*2)
            elif rad>21:v=43+20*light
            else:
                v=57+24*light+20*math.exp(-((dx+8)**2+(dy+13)**2)/220)
                v+=2*math.sin(rad*5)
            p[x,y]=(int(max(0,min(255,v*.92))),int(max(0,min(255,v*.97))),int(max(0,min(255,v))),255)
    d=ImageDraw.Draw(im)
    a=math.radians(135+step*270/16)
    points=[(round((35.5+math.cos(a)*r)*scale),round((34+math.sin(a)*r)*scale)) for r in (9,24)]
    d.line(points,fill=(8,13,16,255),width=5*scale)
    d.line([(x-scale,y-scale) for x,y in points],fill=(225,245,236,255),width=3*scale)
    save(f'encoder_{step}',im.resize((96,96),Image.Resampling.LANCZOS),8,4)
for colour,rgb in [('green',(38,255,108)),('yellow',(255,196,31)),('red',(255,34,41)),('blue',(40,150,255))]:
    for on in (False,True):
        im=Image.new('RGBA',(72,48));p=im.load()
        for y in range(48):
            for x in range(72):
                dx=(x-35)/2;dy=(y-23)/2;r=math.hypot(dx,dy)
                if r>11.5:continue
                light=(-dx-dy)/max(1,r)/1.414
                if r>9.5:
                    v=75+65*light;c=(int(v*.9),int(v*.96),int(v))
                elif r>8.2:c=(5,8,11)
                else:
                    factor=(.40+.55*math.sqrt(max(0,1-(r/8.5)**2))) if on else .07+.06*max(light,0)
                    glint=math.exp(-((dx+2.5)**2+(dy+3)**2)/5)*(.65 if on else .22)
                    c=tuple(int(min(255,v*factor+255*glint)) for v in rgb)
                p[x,y]=c+(255,)
        if on:
            halo=Image.new('RGBA',im.size);d=ImageDraw.Draw(halo);d.ellipse((19,7,51,39),fill=rgb+(105,));halo=halo.filter(ImageFilter.GaussianBlur(5));halo.alpha_composite(im);im=halo
        save(f'lens_{colour}_{"on" if on else "off"}',im.resize((36,24),Image.Resampling.LANCZOS),3,1)
lines=['// Generated by scripts/build_channel_materials.py. Original MIT geometry.', '#[rustfmt::skip]', 'pub const ASSETS: &[(&str, u16, u16, &str)] = &[']
for name,c,r,data in ASSETS:lines.append(f'    ("{name}", {c}, {r}, "{data}"),')
lines.append('];')
(ROOT/'src/channel_material_assets.rs').write_text('\n'.join(lines)+'\n')
print(len(ASSETS),'assets;',sum(len(a[3]) for a in ASSETS),'base64 bytes')
