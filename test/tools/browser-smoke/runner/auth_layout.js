// Observe actual visibility and decoration bytes; do not mutate the product DOM.
const done = arguments[arguments.length - 1];
(async () => {
const card = document.querySelector('.authentication-card');
const rect = e => { const r=e.getBoundingClientRect(); return {x:r.x,y:r.y,width:r.width,height:r.height,right:r.right,bottom:r.bottom}; };
const rendered = e => e.checkVisibility({opacityProperty:true,visibilityProperty:true});
// Chromium may retain nonempty layout rects below a closed native disclosure.
// Exclude only that intentional state, never an unexpectedly hidden control.
const disclosed = e => {
    for (let ancestor=e.parentElement;ancestor;ancestor=ancestor.parentElement) {
        if (ancestor.tagName==='DETAILS'&&!ancestor.open) {
            const summary=[...ancestor.children].find(child=>child.tagName==='SUMMARY');
            if (!summary?.contains(e)) return false;
        }
    }
    return true;
};
const luminance = rgb => rgb.slice(0,3).map(x=>x/255).map(x=>x<=.04045?x/12.92:((x+.055)/1.055)**2.4).reduce((sum,x,i)=>sum+x*[.2126,.7152,.0722][i],0);
const contrast = (a,b) => { const x=luminance(a),y=luminance(b);return (Math.max(x,y)+.05)/(Math.min(x,y)+.05); };
const decoration = async (node, css) => {
    const property=css.borderImageSource!=='none'?'borderImageSource':'backgroundImage';
    const source=css[property], match=source.match(/^url\(["']?(blob:[^"')]+)["']?\)$/);
    if (!match||!['paint','clip'].includes(node.dataset.zxsContourState)) return {valid:false,property,source};
    // G4 exact controls use background-image, while nine-slice surfaces use
    // border-image. Decode through img-src blob, not CSP-disallowed blob fetch.
    const image=new Image();image.src=match[1];await image.decode();
    const canvas=document.createElement('canvas');canvas.width=image.naturalWidth;canvas.height=image.naturalHeight;
    const context=canvas.getContext('2d');context.drawImage(image,0,0);
    const center=[...context.getImageData(Math.floor(canvas.width/2),Math.floor(canvas.height/2),1,1).data];
    const pixels=context.getImageData(0,0,canvas.width,canvas.height).data;
    let contrastingPixels=0;
    for(let i=0;i<pixels.length;i+=4) {
        if(pixels[i+3]>=200&&contrast([pixels[i],pixels[i+1],pixels[i+2]],center)>=3) contrastingPixels++;
    }
    const stable=node.isConnected&&getComputedStyle(node)[property]===source;
    return {valid:stable&&center[3]>=200&&contrastingPixels>=Math.min(canvas.width,canvas.height),property,stable,
        width:canvas.width,height:canvas.height,centerAlpha:center[3],contrastingPixels};
};
const allControls=[...card.querySelectorAll('input.zxs-input,button.zxs-button')];
const controls=allControls.filter(disclosed);
const closedContentHidden=allControls.filter(e=>!disclosed(e)).every(e=>!rendered(e));
const visible=await Promise.all(controls.map(async e => {
    const s=getComputedStyle(e), r=rect(e), isRendered=rendered(e);
    const border=parseFloat(s.borderTopWidth)>0&&s.borderTopStyle!=='none';
    const nativeBorder=border&&s.borderTopColor!=='rgba(0, 0, 0, 0)'&&s.borderTopColor!=='transparent';
    const paint=nativeBorder?{valid:true,property:'border'}:await decoration(e,s);
    const painted=border&&paint.valid;
    return {name:e.name||e.textContent.trim(),rect:r,rendered:isRendered,painted,paint,
        valid:isRendered&&r.height>=44&&r.width>0&&r.x>=0&&r.right<=innerWidth+1&&painted};
}));
const regionNodes=[...card.children].filter(e=>!e.matches('.zxs-product-identity'));
const regions=regionNodes.map(rect);
const form=card.querySelector(':scope > form');
const rowNodes=[...form.querySelectorAll('label > span,input,.zxs-error,:scope > button')];
const rows=rowNodes.map(rect);
const required=[form.querySelector('[name=username]'),form.querySelector('[name=password]'),form.querySelector('button[type=submit]'),card.querySelector(':scope > section button')];
const requiredPresent=required.every(e=>e&&controls.includes(e));
const setup=card.querySelector('details');
const setupPresent=!setup.open||(setup.querySelectorAll('input.zxs-input').length>=1&&setup.querySelectorAll('button.zxs-button').length>=2);
const separate = list => list.every((r,i)=>!i||r.y>=list[i-1].bottom-.5);
const header=rect(document.querySelector('.authentication-language')), panel=rect(card);
return {valid:requiredPresent&&setupPresent&&closedContentHidden&&document.documentElement.scrollWidth<=innerWidth+1&&visible.every(e=>e.valid)&&regionNodes.every(rendered)&&rowNodes.every(rendered)&&separate(regions)&&separate(rows)&&panel.y>=header.bottom,
    viewport:{width:innerWidth,height:innerHeight},documentWidth:document.documentElement.scrollWidth,header,panel,regions,rows,
    requiredPresent,setupPresent,closedContentHidden,excludedClosedDisclosure:allControls.length-controls.length,controls:visible};
})().then(done,error=>done({valid:false,error:String(error)}));
