// Observe actual visibility and decoration bytes; do not mutate the product DOM.
const done = arguments[arguments.length - 1];
(async () => {
const card = document.querySelector('.authentication-card');
// innerWidth includes a classic scrollbar; content must fit the layout viewport.
const layoutWidth = document.documentElement.clientWidth;
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
const raster = async url => {
    // Decode through img-src blob, not CSP-disallowed blob fetch.
    const image=new Image();image.src=url;await image.decode();
    const canvas=document.createElement('canvas');canvas.width=image.naturalWidth;canvas.height=image.naturalHeight;
    const context=canvas.getContext('2d');context.drawImage(image,0,0);
    return {width:canvas.width,height:canvas.height,pixels:context.getImageData(0,0,canvas.width,canvas.height).data};
};
const foregroundSelector=':scope > svg[data-zxs-contour-decoration="foreground"]';
const foregroundStyle = node => {
    const css=getComputedStyle(node);
    return Object.fromEntries(['d','fill','fillRule','stroke','opacity','fillOpacity','strokeOpacity','strokeWidth',
        'display','visibility','transform','filter','clipPath','maskImage'].map(name=>[name,css[name]]));
};
const foregroundPaint = async (node, foreground, property, source, url) => {
    const path=foreground.querySelector(':scope > path');
    if (!path||foreground.children.length!==1||foreground.children[0]!==path) {
        return {valid:false,property:'foreground',reason:'expected one foreground path'};
    }
    const box=rect(node), svgBox=rect(foreground), parent=node.parentElement;
    const style=foregroundStyle(path), svgStyle=foregroundStyle(foreground);
    const markup=foreground.outerHTML, viewBox=foreground.getAttribute('viewBox');
    // SVG d is a presentation attribute: a CSS declaration can override it.
    const computedPath=style.d.match(/^path\("([^"]*)"\)$/);
    const hostStyle=()=>{
        const css=getComputedStyle(node);
        return Object.fromEntries(['opacity',...['Top','Right','Bottom','Left'].flatMap(side=>
            ['Width','Style','Color'].map(part=>`border${side}${part}`))].map(name=>[name,css[name]]));
    };
    const host=hostStyle();
    const state=node.dataset.zxsContourState;
    const equal=(a,b)=>Object.keys(a).every(key=>a[key]===b[key]);
    const plain=s=>s.display!=='none'&&s.visibility==='visible'&&s.transform==='none'
        &&s.filter==='none'&&s.clipPath==='none'&&s.maskImage==='none';
    // The controller emits an untransformed even-odd ring, not a stroked icon.
    if (!computedPath||host.opacity!=='1'||!plain(style)||!plain(svgStyle)||style.fillRule!=='evenodd'||style.stroke!=='none'
        ||box.width<=0||box.height<=0||!['x','y','width','height'].every(key=>Math.abs(box[key]-svgBox[key])<=1)) {
        return {valid:false,property:'foreground',reason:'hidden, misplaced or unsupported foreground'};
    }
    const stable=()=>node.isConnected&&node.parentElement===parent&&foreground.isConnected&&path.isConnected
        &&foreground.parentElement===node&&path.parentElement===foreground
        &&node.querySelectorAll(foregroundSelector).length===1&&node.querySelector(foregroundSelector)===foreground
        &&foreground.children.length===1&&foreground.children[0]===path&&foreground.querySelector(':scope > path')===path
        &&rendered(node)&&rendered(foreground)&&foreground.outerHTML===markup
        &&equal(rect(node),box)&&equal(rect(foreground),svgBox)
        &&equal(foregroundStyle(path),style)&&equal(foregroundStyle(foreground),svgStyle)&&equal(hostStyle(),host)
        &&getComputedStyle(node)[property]===source&&node.dataset.zxsContourState===state;
    if (!stable()) return {valid:false,property:'foreground',stable:false};
    // Foreground borders are deliberately absent from the background decoration.
    // Its opaque center remains the contrast reference in either theme.
    const background=await raster(url);
    const centerIndex=(Math.floor(background.height/2)*background.width+Math.floor(background.width/2))*4;
    const center=[...background.pixels.slice(centerIndex,centerIndex+4)], backgroundStable=stable();
    if (!backgroundStable||center[3]<200) return {valid:false,property:'foreground',stable:backgroundStable,backgroundCenterAlpha:center[3]};
    const ns='http://www.w3.org/2000/svg';
    const exported=document.createElementNS(ns,'svg'), exportedPath=document.createElementNS(ns,'path');
    exported.setAttribute('width',String(svgBox.width));exported.setAttribute('height',String(svgBox.height));
    exported.setAttribute('viewBox',viewBox);exported.setAttribute('opacity',svgStyle.opacity);
    const aspectRatio=foreground.getAttribute('preserveAspectRatio');
    if(aspectRatio!==null)exported.setAttribute('preserveAspectRatio',aspectRatio);
    exportedPath.setAttribute('d',computedPath[1]);
    for(const [attribute,value] of [['fill',style.fill],['fill-rule',style.fillRule],['stroke',style.stroke],
        ['opacity',style.opacity],['fill-opacity',style.fillOpacity]]) exportedPath.setAttribute(attribute,value);
    exported.appendChild(exportedPath);
    const ownedUrl=URL.createObjectURL(new Blob([new XMLSerializer().serializeToString(exported)],{type:'image/svg+xml'}));
    try {
        const {width:w,height:h,pixels}=await raster(ownedUrl);
        const edgePixels=[0,0,0,0];let contrastingPixels=0,centerAlpha=0;
        const middleX=x=>x>=Math.floor(w*.4)&&x<Math.ceil(w*.6);
        const middleY=y=>y>=Math.floor(h*.4)&&y<Math.ceil(h*.6);
        for(let y=0;y<h;y++)for(let x=0;x<w;x++) {
            const i=(y*w+x)*4;
            if(middleX(x)&&middleY(y))centerAlpha=Math.max(centerAlpha,pixels[i+3]);
            if(pixels[i+3]<200)continue;
            const alpha=pixels[i+3]/255, visible=[0,1,2].map(channel=>pixels[i+channel]*alpha+center[channel]*(1-alpha));
            if(contrast(visible,center)<3)continue;
            contrastingPixels++;
            if(middleX(x)&&y<Math.ceil(h*.25))edgePixels[0]++;
            if(middleY(y)&&x>=Math.floor(w*.75))edgePixels[1]++;
            if(middleX(x)&&y>=Math.floor(h*.75))edgePixels[2]++;
            if(middleY(y)&&x<Math.ceil(w*.25))edgePixels[3]++;
        }
        const unchanged=stable();
        return {valid:unchanged&&centerAlpha===0&&edgePixels.every(n=>n>0)&&contrastingPixels>=Math.min(w,h),
            property:'foreground',stable:unchanged,width:w,height:h,centerAlpha,backgroundCenterAlpha:center[3],edgePixels,contrastingPixels};
    } finally {
        URL.revokeObjectURL(ownedUrl);
    }
};
const decoration = async (node, css) => {
    const property=css.borderImageSource!=='none'?'borderImageSource':'backgroundImage';
    const source=css[property], match=source.match(/^url\(["']?(blob:[^"')]+)["']?\)$/);
    if (!match||!['paint','clip'].includes(node.dataset.zxsContourState)) return {valid:false,property,source};
    const foreground=node.querySelector(foregroundSelector);
    if (foreground) return foregroundPaint(node,foreground,property,source,match[1]);
    // G4 exact controls use background-image; nine-slice surfaces use border-image.
    const {width,height,pixels}=await raster(match[1]);
    const i=(Math.floor(height/2)*width+Math.floor(width/2))*4, center=[...pixels.slice(i,i+4)];
    let contrastingPixels=0;
    for(let i=0;i<pixels.length;i+=4) {
        if(pixels[i+3]>=200&&contrast([pixels[i],pixels[i+1],pixels[i+2]],center)>=3) contrastingPixels++;
    }
    const stable=node.isConnected&&getComputedStyle(node)[property]===source;
    return {valid:stable&&center[3]>=200&&contrastingPixels>=Math.min(width,height),property,stable,
        width,height,centerAlpha:center[3],contrastingPixels};
};
const transparent = color => color==='rgba(0, 0, 0, 0)'||color==='transparent';
const sides=['Top','Right','Bottom','Left'];
const surfacePaint = async node => {
    const css=getComputedStyle(node);
    const bordered=sides.every(side=>parseFloat(css[`border${side}Width`])>0&&!['none','hidden'].includes(css[`border${side}Style`]));
    const native=bordered&&sides.every(side=>!transparent(css[`border${side}Color`]));
    const paint=native?{valid:true,property:'border'}:await decoration(node,css);
    return {...paint,valid:bordered&&paint.valid&&rendered(node)};
};
const groupNodes=[...card.querySelectorAll('.zxs-form-grid')].filter(disclosed);
const groupPaint=new Map(await Promise.all(groupNodes.map(async node=>[node,await surfacePaint(node)])));
const allControls=[...card.querySelectorAll('input.zxs-input,textarea.zxs-input,button.zxs-button')];
const controls=allControls.filter(disclosed);
const closedContentHidden=allControls.filter(e=>!disclosed(e)).every(e=>!rendered(e));
const visible=await Promise.all(controls.map(async e => {
    const s=getComputedStyle(e), r=rect(e), isRendered=rendered(e);
    const internal=e.tagName==='INPUT'||e.tagName==='TEXTAREA', group=e.closest('.zxs-form-grid');
    const underline=['Top','Right','Left'].every(side=>parseFloat(s[`border${side}Width`])===0)
        &&parseFloat(s.borderBottomWidth)>0&&s.borderBottomStyle==='dashed'&&!transparent(s.borderBottomColor)
        &&s.backgroundImage==='none'&&s.borderImageSource==='none'&&transparent(s.backgroundColor)&&s.boxShadow==='none';
    const paint=internal?{valid:underline&&groupPaint.get(group)?.valid===true,property:'form-group',underline}:await surfacePaint(e);
    const painted=paint.valid;
    return {name:e.name||e.textContent.trim(),rect:r,rendered:isRendered,painted,paint,
        valid:isRendered&&r.height>=44&&r.width>0&&r.x>=0&&r.right<=layoutWidth+1&&painted};
}));
const regionNodes=[...card.children].filter(e=>!e.matches('.zxs-product-identity'));
const regions=regionNodes.map(rect);
const form=card.querySelector(':scope > form');
// Top-level blocks separate vertically; each field keeps its label beside its control.
const rowNodes=[...form.children];
const rows=rowNodes.map(rect);
const required=[form.querySelector('[name=username]'),form.querySelector('[name=password]'),form.querySelector('button[type=submit]'),card.querySelector(':scope > section button')];
const requiredPresent=required.every(e=>e&&controls.includes(e));
const setup=card.querySelector('details');
const setupPresent=!setup.open||(setup.querySelectorAll('input.zxs-input').length>=1&&setup.querySelectorAll('button.zxs-button').length>=2);
const separate = list => list.every((r,i)=>!i||r.y>=list[i-1].bottom-.5);
const formGroups=groupNodes.map(group=>{
    const bounds=rect(group);
    const fields=[...group.querySelectorAll(':scope > label.zxs-form-field')].map(field=>{
        const label=field.querySelector(':scope > span'), control=field.querySelector(':scope > input,:scope > textarea');
        if (!label||!control) return {valid:false,rect:rect(field)};
        const row=rect(field), left=rect(label), right=rect(control);
        const sameRow=left.right<=right.x+.5&&Math.min(left.bottom,right.bottom)>Math.max(left.y,right.y)
            &&Math.abs((left.y+left.bottom-right.y-right.bottom)/2)<=2;
        const contained=row.x>=bounds.x-.5&&row.right<=bounds.right+.5&&row.y>=bounds.y-.5&&row.bottom<=bounds.bottom+.5
            &&[left,right].every(r=>r.x>=row.x-.5&&r.right<=row.right+.5&&r.y>=row.y-.5&&r.bottom<=row.bottom+.5);
        const css=getComputedStyle(field);
        const noRowBorder=sides.every(side=>parseFloat(css[`border${side}Width`])===0);
        return {valid:noRowBorder&&rendered(field)&&rendered(label)&&rendered(control)&&sameRow&&contained,
            rect:row,label:left,control:right,sameRow,contained,noRowBorder};
    });
    const aligned=fields.every(field=>field.control&&Math.abs(field.control.x-fields[0].control.x)<=1);
    return {valid:bounds.x>=0&&bounds.right<=layoutWidth+1&&groupPaint.get(group).valid&&rendered(group)&&fields.length>0&&fields.every(field=>field.valid)&&separate(fields.map(field=>field.rect))&&aligned,
        rect:rect(group),fields,aligned,paint:groupPaint.get(group)};
});
const formGroupsPresent=formGroups.length>0&&controls.filter(e=>e.tagName==='INPUT'||e.tagName==='TEXTAREA')
    .every(e=>e.closest('.zxs-form-grid')&&e.closest('label.zxs-form-field'));
const header=rect(document.querySelector('.authentication-language')), panel=rect(card);
return {valid:formGroupsPresent&&formGroups.every(group=>group.valid)&&requiredPresent&&setupPresent&&closedContentHidden&&document.documentElement.scrollWidth<=layoutWidth+1&&visible.every(e=>e.valid)&&regionNodes.every(rendered)&&rowNodes.every(rendered)&&separate(regions)&&separate(rows)&&panel.y>=header.bottom&&[header,panel].every(r=>r.x>=0&&r.right<=layoutWidth+1),
    viewport:{width:innerWidth,height:innerHeight,clientWidth:layoutWidth,scrollbarWidth:innerWidth-layoutWidth},documentWidth:document.documentElement.scrollWidth,header,panel,regions,rows,formGroups,formGroupsPresent,
    requiredPresent,setupPresent,closedContentHidden,excludedClosedDisclosure:allControls.length-controls.length,controls:visible};
})().then(done,error=>done({valid:false,error:String(error)}));
