const card = document.querySelector('.authentication-card');
const rect = e => { const r=e.getBoundingClientRect(); return {x:r.x,y:r.y,width:r.width,height:r.height,right:r.right,bottom:r.bottom}; };
const controls = [...card.querySelectorAll('input.zxs-input,button.zxs-button')].filter(e=>e.getClientRects().length);
const visible = controls.map(e => {
    const s=getComputedStyle(e), r=rect(e);
    // G4 paints its real border through border-image and makes the CSS color transparent.
    const painted = parseFloat(s.borderTopWidth)>0 && s.borderTopStyle!=='none' && (s.borderTopColor!=='rgba(0, 0, 0, 0)' || s.borderImageSource.startsWith('url('));
    return {name:e.name||e.textContent.trim(),rect:r,painted,valid:r.height>=44&&r.width>0&&r.x>=0&&r.right<=innerWidth+1&&painted};
});
const regions = [...card.children].filter(e=>!e.matches('.zxs-product-identity')&&e.getClientRects().length).map(rect);
const form = card.querySelector(':scope > form');
const rows = [...form.querySelectorAll('label > span,input,.zxs-error,:scope > button')].filter(e=>e.getClientRects().length).map(rect);
const separate = list => list.every((r,i)=>!i||r.y>=list[i-1].bottom-.5);
const header=rect(document.querySelector('.authentication-language')), panel=rect(card);
return {valid:document.documentElement.scrollWidth<=innerWidth+1&&visible.every(e=>e.valid)&&separate(regions)&&separate(rows)&&panel.y>=header.bottom,
    viewport:{width:innerWidth,height:innerHeight},documentWidth:document.documentElement.scrollWidth,header,panel,regions,rows,controls:visible};
