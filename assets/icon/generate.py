from pathlib import Path
from PIL import Image, ImageDraw, ImageFont
import io, struct
ROOT=Path(__file__).resolve().parent
SIZES=[16,20,24,32,40,48,64,128,256]
# Individual optical masters: margin, corner radius, shell thickness, split, cells.
GRID={16:(1,4,1,10,2),20:(1,5,1,12,3),24:(1,6,2,15,3),32:(2,8,2,20,4),40:(2,10,3,25,5),48:(3,12,3,30,6),64:(4,16,4,40,8),128:(8,32,8,80,16),256:(16,64,16,160,32)}
BLUE='#89BAFF'; INK='#14181E'; WHITE='#E8EDF5'; GREEN='#70D6B2'
def icon(n, concept=0):
    output=n; scale=4 if n>=48 else 1
    m,r,t,split,cell=[v*scale for v in GRID[n]]
    n*=scale
    im=Image.new('RGBA',(n,n)); d=ImageDraw.Draw(im)
    d.rounded_rectangle((m,m,n-m-1,n-m-1),radius=r,fill=BLUE)
    d.rounded_rectangle((m+t,m+t,n-m-t-1,n-m-t-1),radius=max(1,r-t),fill=INK)
    if concept==0:
        # Open terminal chevron plus a pair of agent bays. Integer coordinates,
        # no downsampling: 16/20 use a widened aperture and two distinct cells.
        x=m+t+cell; y=n//2; arm=max(2,cell)
        d.line([(x,y-arm),(x+arm,y),(x,y+arm)],fill=WHITE,width=max(1,t),joint="curve")
        x=split; w=max(2,cell); gap=max(1,t)
        d.rectangle((x,y-cell-gap,x+w-1,y-gap-1),fill=BLUE)
        d.rectangle((x,y+gap,x+w-1,y+gap+cell-1),fill=GREEN)
    elif concept==1:
        for x,y,c in [(n//3,n//3,BLUE),(n*2//3,n//3,WHITE),(n//2,n*2//3,GREEN)]:
            d.rounded_rectangle((x-cell//2,y-cell//2,x+cell//2,y+cell//2),radius=max(1,t),fill=c)
    else:
        d.rounded_rectangle((n//3,n//4,n*2//3,n*3//4),radius=cell,fill=BLUE)
        d.rectangle((n//2,n//2,n*2//3,n*3//4),fill=INK)
        d.rectangle((n//2,n*2//3,n*2//3,n*2//3+t),fill=GREEN)
    return im.resize((output,output),Image.Resampling.LANCZOS) if scale>1 else im
for n in SIZES: icon(n).save(ROOT/f'pod-{n}.png')
# Build ICO directly from each optical PNG, avoiding Pillow's automatic resizing.
chunks=[]
for n in SIZES:
    b=io.BytesIO(); icon(n).save(b,format='PNG'); chunks.append(b.getvalue())
offset=6+16*len(SIZES); entries=[]
for n,b in zip(SIZES,chunks):
    entries.append(struct.pack('<BBBBHHII',n%256,n%256,0,0,1,32,len(b),offset)); offset+=len(b)
(ROOT/'pod.ico').write_bytes(struct.pack('<HHH',0,1,len(SIZES))+b''.join(entries)+b''.join(chunks))
font=ImageFont.truetype('C:/Windows/Fonts/segoeui.ttf',18)
big=ImageFont.truetype('C:/Windows/Fonts/segoeuib.ttf',27)
sheet=Image.new('RGB',(1240,1000),'#F6F8FB'); d=ImageDraw.Draw(sheet)
d.text((30,18),'Pod / original identity studies',font=big,fill=INK)
names=['01  Terminal bays / selected','02  Agent cluster','03  Pod monogram']
for i,name in enumerate(names):
    x=30+i*410; sheet.paste(icon(128,i),(x,78),icon(128,i)); d.text((x,220),name,font=font,fill=INK)
    icon(256,i).save(ROOT/'concepts'/f'{i+1:02}.png')
for row,bg in enumerate(['#14181E','#F6F8FB']):
    y=280+row*360; d.rectangle((0,y,1240,y+359),fill=bg)
    fg=WHITE if row==0 else INK; d.text((30,y+12),'Selected: native pixels / '+('dark' if row==0 else 'light'),font=font,fill=fg)
    x=30
    for n in SIZES:
        im=icon(n); sheet.paste(im,(x,y+55),im); d.text((x,y+320),str(n),font=font,fill=fg); x+=max(n,70)+16
sheet.save(ROOT/'qa/contact-sheet.png')
# SVG master matches the 256px optical grid. Original geometry only.
(ROOT/'pod.svg').write_text('''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256"><title>Pod: terminal bays</title><rect x="16" y="16" width="224" height="224" rx="64" fill="#89BAFF"/><rect x="32" y="32" width="192" height="192" rx="48" fill="#14181E"/><path d="M64 96 96 128 64 160" fill="none" stroke="#E8EDF5" stroke-width="16" stroke-linejoin="round"/><path fill="#89BAFF" d="M160 80h32v32h-32z"/><path fill="#70D6B2" d="M160 144h32v32h-32z"/></svg>''',encoding='utf-8')
print('Generated nine independently snapped PNGs, ICO, SVG and concept contact sheet')

# Installer artwork derives only from the Pod mark, never stock product art.
small=Image.new('RGB',(64,64),'white'); mark=icon(48)
small.paste(mark,(8,8),mark); small.save(ROOT/'installer-small.bmp')
welcome=Image.new('RGB',(164,314),INK); draw=ImageDraw.Draw(welcome)
mark=icon(128); welcome.paste(mark,(18,44),mark)
draw.text((49,191),'Pod',font=big,fill=WHITE)
draw.text((28,244),'Agent coding',font=ImageFont.truetype('C:/Windows/Fonts/segoeui.ttf',16),fill=BLUE)
draw.text((70,267),'IDE',font=ImageFont.truetype('C:/Windows/Fonts/segoeui.ttf',16),fill=BLUE)
welcome.save(ROOT/'installer-welcome.bmp')
