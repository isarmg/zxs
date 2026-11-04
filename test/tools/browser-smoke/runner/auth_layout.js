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
