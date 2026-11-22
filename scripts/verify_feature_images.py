"""Pixel checks on app-owned native captures; no Lightroom/catalog access."""
from pathlib import Path
from PIL import Image

def verify(directory):
    directory=Path(directory)
    before=Image.open(directory/'05.png').convert('RGB')
    after=Image.open(directory/'06.png').convert('RGB')
    w,h=before.size
    # The photo canvas is a vertical gradient, so a row's canvas colour is the
    # colour at the far right of that row. A pixel belongs to the canvas when
    # it matches that colour; matching one global colour broke as soon as the
    # canvas stopped being flat.
    def canvas_colour(y):
        return before.getpixel((w-2,y))
    def row_canvas(y):
        # The canvas is a neutral surface: dark in the dark appearance, light
        # in the light one. A row whose far right is coloured belongs to some
        # panel instead, and the panels are not exposed.
        ref=canvas_colour(y)
        if max(ref)-min(ref)>=32:
            return None
        return ref if (max(ref)<90 or min(ref)>160) else None
    def is_canvas(x,y):
        ref=row_canvas(y)
        if ref is None:
            return False
        px=before.getpixel((x,y))
        if max(abs(px[i]-ref[i]) for i in range(3))<=6:
            return True
        # The photo sits on a soft shadow. A shadow is the canvas colour
        # darkened towards black, so every channel shrinks by the same factor:
        # that is not part of the photo either.
        ratios=[(px[i]/ref[i]) if ref[i]>2 else 1.0 for i in range(3)]
        return max(ratios)<=1.03 and (max(ratios)-min(ratios))<=0.07
    assert row_canvas(int(h*.4)) is not None,'Cannot locate the neutral photo canvas margin'
    # A visible folder/filter panel has a different colour from the canvas.
    # Fixed screen percentages can include that unchanged UI at the minimum width.
    probe_y=int(h*.4)
    canvas_left=next((x for x in range(0,int(w*.9)-5)
        if all(is_canvas(x+dx,probe_y+dy) for dx in range(5) for dy in range(3))),0)
    # The photo's own extent, found row by row and column by column inside the
    # canvas area, so the sample window follows the photo instead of a fixed
    # fraction of the window.
    def photo_row(y):
        return any(not is_canvas(x,y) for x in range(canvas_left+8,w-8,3))
    rows=[y for y in range(int(h*.05),int(h*.9)) if photo_row(y)]
    assert rows,'Cannot locate the photo on the canvas'
    canvas_top,canvas_bottom=min(rows),max(rows)
    def photo_col(x):
        return any(not is_canvas(x,y) for y in range(canvas_top,canvas_bottom,3))
    cols=[x for x in range(canvas_left+8,w-8) if photo_col(x)]
    assert cols,'Cannot locate the photo on the canvas'
    # Stay inside the photo even when the toolbar wraps at 640x480.
    inset=10
    crop=(max(int(w*.32),min(cols)+inset),max(int(h*.1),canvas_top+inset),
          min(int(w*.68),max(cols)-inset),min(int(h*.65),canvas_bottom-inset))
    def pixels(image):
        return image.get_flattened_data() if hasattr(image,'get_flattened_data') else image.getdata()
    a=before.crop(crop);b=after.crop(crop)
    assert len(set(pixels(a)))>1000,'Photo canvas is missing or hidden by controls'
    def brighter(v):
        s=v/255.;linear=s/12.92 if s<=.04045 else ((s+.055)/1.055)**2.4
        linear=min(1.,linear*2)
        return 255*(linear*12.92 if linear<=.0031308 else 1.055*linear**(1/2.4)-.055)
    errors=[];lift=[]
    pa=a.load();pb=b.load()
    for y in range(0,a.size[1],2):
        for x in range(0,a.size[0],2):
            old=pa[x,y];new=pb[x,y]
            # Canvas pixels are not part of the photo and are not exposed.
            if is_canvas(x+crop[0],y+crop[1]):
                continue
            if not all(20 < v < 190 for v in old):
                continue
            errors.extend(abs(brighter(v)-n) for v,n in zip(old,new));lift.append(sum(new)-sum(old))
    mean=sum(errors)/len(errors) if errors else float('inf')
    assert len(errors)>100 and mean<2.,f'Exposure GPU +1 EV mismatch: n={len(errors)}, error={mean:.3f}'
    assert sum(lift)/len(lift)>10,'Exposure did not brighten the displayed image'
    for name in ['08','09']:
        picture=Image.open(directory/f'{name}.png').convert('RGB')
        middle=(canvas_left+w)//2
        for left,right in [(canvas_left+8,middle-8),(middle+8,w-8)]:
            assert len(set(pixels(picture.crop((left,int(h*.2),right,min(int(h*.65),canvas_bottom-8))))))>1000,'One comparison canvas is missing'
    return dict(width=w,height=h,photo_canvas_left=canvas_left,photo_box=[min(cols),canvas_top,max(cols),canvas_bottom],exposure_channel_samples=len(errors),exposure_mean_error=round(mean,3),both_comparison_canvases_visible=True)

if __name__=='__main__':
    import argparse,json
    parser=argparse.ArgumentParser();parser.add_argument('directory',type=Path);args=parser.parse_args()
    print(json.dumps(verify(args.directory),indent=2))
