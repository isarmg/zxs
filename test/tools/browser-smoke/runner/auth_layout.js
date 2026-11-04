// Observe the actual six-row form and decoded G4 fill without changing product DOM.
const done=arguments[arguments.length-1],expectedSetup=arguments[0],phase=arguments[1];
(async () => {
const card=document.querySelector('.authentication-card');
// innerWidth includes a classic scrollbar; content must fit the layout viewport.
const layoutWidth=document.documentElement.clientWidth;
const rect=e=>{const r=e.getBoundingClientRect();return {x:r.x,y:r.y,width:r.width,height:r.height,right:r.right,bottom:r.bottom};};
const rendered=e=>e.checkVisibility({opacityProperty:true,visibilityProperty:true});
const transparent=color=>color==='rgba(0, 0, 0, 0)'||color==='transparent';
const sides=['Top','Right','Bottom','Left'];
const noOutline=css=>parseFloat(css.outlineWidth)===0||['none','hidden'].includes(css.outlineStyle);
const equal=(a,b)=>Object.keys(a).every(key=>a[key]===b[key]);
const raster=async url=>{
    // Decode through img-src blob, not CSP-disallowed blob fetch.
    const image=new Image();image.src=url;await image.decode();
    const canvas=document.createElement('canvas');canvas.width=image.naturalWidth;canvas.height=image.naturalHeight;
    const context=canvas.getContext('2d');context.drawImage(image,0,0);
    return {width:canvas.width,height:canvas.height,pixels:context.getImageData(0,0,canvas.width,canvas.height).data};
};
const colorPixel=color=>{
    const canvas=document.createElement('canvas');canvas.width=canvas.height=1;
    const context=canvas.getContext('2d');context.fillStyle='rgba(0, 0, 0, 0)';context.fillStyle=color;context.fillRect(0,0,1,1);
    return [...context.getImageData(0,0,1,1).data];
};
const outerFill=async node=>{
    const properties=['opacity','backgroundColor','backgroundImage','borderImageSource','borderImageSlice','borderImageWidth',
        'borderImageOutset','borderImageRepeat','outlineWidth','outlineStyle','outlineColor','outlineOffset','boxShadow','transform','filter','clipPath','maskImage','display','visibility',
        ...sides.flatMap(side=>['Width','Style','Color'].map(part=>`border${side}${part}`))];
    const snapshot=()=>{
        const css=getComputedStyle(node);
        return {...Object.fromEntries(properties.map(name=>[name,css[name]])),
            extent:css.getPropertyValue('--zxs-contour-extent'),color:css.getPropertyValue('--zxs-bg-panel'),
            row:css.getPropertyValue('--zxs-compact-row-height'),
            rootRow:getComputedStyle(document.documentElement).getPropertyValue('--zxs-compact-row-height'),
            rootColor:getComputedStyle(document.documentElement).getPropertyValue('--zxs-bg-panel')};
    };
    const css=snapshot(),box=rect(node),parent=node.parentElement,state=node.dataset.zxsContourState;
    const source=css.borderImageSource,match=source.match(/^url\(["']?(blob:[^"')]+)["']?\)$/),extent=parseFloat(css.extent);
    const slice=css.borderImageSlice.match(/^([\d.]+)% fill$/);
    // Current borderless canonical panels use a compact tile with 2px margins
    // and an unstretched 1px center strip, not a border ring or a full-size SVG.
    const size=extent*2+5,inset=extent+2;
    const geometry=extent>0&&slice&&Math.abs(Number(slice[1])-inset/size*100)<.001
        &&css.borderImageWidth===`${inset}px`&&css.borderImageOutset==='2px'&&css.borderImageRepeat==='stretch';
    const unadorned=sides.every(side=>parseFloat(css[`border${side}Width`])===0)
        &&css.opacity==='1'&&noOutline(css)&&css.color.trim()===css.rootColor.trim()&&transparent(css.backgroundColor)&&css.backgroundImage==='none'&&css.boxShadow==='none'
        &&['transform','filter','clipPath','maskImage'].every(name=>css[name]==='none')
        &&css.display!=='none'&&css.visibility==='visible'&&node.querySelectorAll('[data-zxs-contour-decoration]').length===0;
    const stable=()=>node.isConnected&&node.parentElement===parent&&document.querySelector('.authentication-card')===node
        &&rendered(node)&&equal(rect(node),box)&&equal(snapshot(),css)&&node.dataset.zxsContourState===state
        &&node.querySelectorAll('[data-zxs-contour-decoration]').length===0;
    if(!match||state!=='paint'||!geometry||!unadorned||!stable())return {valid:false,property:'outer-fill',geometry,unadorned,source};
    const expected=colorPixel(css.color);
    const {width,height,pixels}=await raster(match[1]);
    const pixel=(x,y)=>[...pixels.slice((y*width+x)*4,(y*width+x)*4+4)];
    const filled=(x,y)=>{const p=pixel(x,y);return p[3]>=200&&p.slice(0,3).every((v,i)=>Math.abs(v-expected[i])<=1);};
    const center=pixel(Math.floor(width/2),Math.floor(height/2));
    const middle=[.3,.5,.7];
    const bodyFilled=middle.every(x=>middle.every(y=>filled(Math.floor(width*x),Math.floor(height*y))));
    const edgeFill=[[Math.floor(width/2),3],[width-4,Math.floor(height/2)],
        [Math.floor(width/2),height-4],[3,Math.floor(height/2)]].map(([x,y])=>filled(x,y));
    const corners=[[3,3],[width-4,3],[width-4,height-4],[3,height-4]].map(([x,y])=>pixel(x,y)[3]);
    let wrongColorPixels=0;
    for(let i=0;i<pixels.length;i+=4)if(pixels[i+3]>=200
        &&[0,1,2].some(channel=>Math.abs(pixels[i+channel]-expected[channel])>1))wrongColorPixels++;
    let transparentMargin=true;
    for(let x=0;x<width;x++)transparentMargin&&=pixel(x,0)[3]===0&&pixel(x,height-1)[3]===0;
    for(let y=0;y<height;y++)transparentMargin&&=pixel(0,y)[3]===0&&pixel(width-1,y)[3]===0;
    const unchanged=stable();
    return {valid:unchanged&&width===Math.round(size)&&height===Math.round(size)&&expected[3]>=200
            &&wrongColorPixels===0&&bodyFilled&&edgeFill.every(Boolean)&&corners.every(alpha=>alpha<=8)&&transparentMargin,
        property:'outer-fill',stable:unchanged,source,width,height,expectedSize:size,extent,slice:Number(slice[1]),
        expected,center,wrongColorPixels,bodyFilled,edgeFill,corners,transparentMargin};
};
// The installation fixture is authoritative; never infer state from absent fields.
const rateLimited=expectedSetup==='rate-limited';
const expectedFormSelector=expectedSetup==='existing'||rateLimited?'form[data-auth-form="login"]'
    :['empty','unconfirmed'].includes(expectedSetup)?'form[data-first-account-form]':null;
const forms=[...card.querySelectorAll('form')];
const form=expectedFormSelector?card.querySelector(expectedFormSelector):null;
const present=e=>!!e&&e.isConnected&&rendered(e);
const inside=(inner,outer)=>inner.x>=outer.x-.5&&inner.right<=outer.right+.5
    &&inner.y>=outer.y-.5&&inner.bottom<=outer.bottom+.5;
const within=e=>{const r=rect(e);return r.width>0&&r.height>0&&r.x>=0&&r.right<=layoutWidth+1;};
const separate=list=>list.every((r,i)=>!i||r.y>=list[i-1].bottom-.5);
const headerNode=document.querySelector('.authentication-language'),header=rect(headerNode),panel=rect(card);
const panelStyle=getComputedStyle(card),rootStyle=getComputedStyle(document.documentElement);
const corner={extent:panelStyle.getPropertyValue('--zxs-contour-extent').trim(),original:48};
const rowHeightToken=panelStyle.getPropertyValue('--zxs-compact-row-height').trim();
const sharedRowHeight=Number.parseFloat(rootStyle.getPropertyValue('--zxs-compact-row-height'));
const rowHeightValid=rowHeightToken===rootStyle.getPropertyValue('--zxs-compact-row-height').trim()&&sharedRowHeight>0;
const restoredCorner=parseFloat(corner.extent)===corner.original;
const compact=innerWidth<=416||innerHeight<=480;
const aspect=panelStyle.aspectRatio.split('/').map(Number);
const preferredRatio=compact?panelStyle.aspectRatio==='auto':aspect.length===2&&aspect[0]/aspect[1]===1.5;
// Content may make the panel taller. It must never be compressed or clipped to 3:2.
const panelGeometry=panel.width<=sharedRowHeight*9+.5&&(compact||panel.height>=panel.width/1.5-.5)
    &&panel.y>=header.bottom&&[header,panel].every(r=>r.x>=0&&r.right<=layoutWidth+1);
const headerControls=[...headerNode.querySelectorAll('button')];
const headerTargets=headerControls.length===2&&headerControls.every(e=>present(e)&&within(e)
    &&rect(e).height>=44&&rect(e).width>=44&&inside(rect(e),header));
const outerPaint=await outerFill(card);
const outerFrame=outerPaint.valid&&restoredCorner;
const semanticChildren=[...card.children].filter(e=>!e.matches('svg[data-zxs-contour-decoration="foreground"]'));
const formPresent=forms.length===1&&forms[0]===form&&present(form)&&form.parentElement===card
    &&semanticChildren.length===1&&semanticChildren[0]===form&&within(form)&&inside(rect(form),panel);
const rows=form?[...form.children]:[],rowBoxes=rows.map(rect);
const sixRows=rows.length===6&&rows.map(e=>e.tagName).join(',')==='LABEL,INPUT,LABEL,INPUT,DIV,DIV'
    &&rows[4].matches('.zxs-auth-information')&&rows[5].matches('.zxs-auth-actions');
const controls=form?[...form.querySelectorAll('input,textarea,button')]:[];
const noSurface=e=>{
    const css=getComputedStyle(e),focus=e.tagName==='BUTTON'&&e.matches(':focus-visible');
    return sides.every(side=>parseFloat(css[`border${side}Width`])===(focus&&side==='Bottom'?3:0))
        &&(!focus||css.borderBottomStyle==='solid'&&!transparent(css.borderBottomColor))
        &&css.backgroundImage==='none'&&css.borderImageSource==='none'&&transparent(css.backgroundColor)
        &&css.boxShadow==='none'&&noOutline(css)&&parseFloat(css.getPropertyValue('--zxs-contour-extent'))===0
        &&e.querySelectorAll('svg[data-zxs-contour-decoration]').length===0;
};
const unlinedInput=e=>{
    const css=getComputedStyle(e);
    return ['Top','Right','Left'].every(side=>parseFloat(css[`border${side}Width`])===0)
        &&parseFloat(css.borderBottomWidth)===(e.matches(':focus-visible')?3:0)
        &&(e.matches(':focus-visible')?css.borderBottomStyle==='solid'&&!transparent(css.borderBottomColor):css.borderBottomStyle==='none')
        &&css.backgroundImage==='none'&&css.borderImageSource==='none'&&transparent(css.backgroundColor)
        &&css.boxShadow==='none'&&noOutline(css)&&parseFloat(css.getPropertyValue('--zxs-contour-extent'))===0
        &&e.querySelectorAll('svg[data-zxs-contour-decoration]').length===0;
};
const name=e=>(e.getAttribute('aria-label')||e.textContent).trim();
const visible=controls.map(e=>{
    const r=rect(e),input=e.tagName==='INPUT',paint={property:input?'unlined-input':'unboxed-action',
        valid:input?unlinedInput(e):e.tagName==='BUTTON'&&noSurface(e)&&name(e).length>0};
    return {name:e.name||name(e),rect:r,rendered:present(e),painted:paint.valid,paint,
        valid:present(e)&&r.height>=sharedRowHeight&&r.width>=sharedRowHeight&&within(e)&&inside(r,panel)&&paint.valid};
});
// Wrappers, hint content and labels are transparent. SVG icon paths are not
// HTML surfaces, but an internal G4 decoration anywhere is always forbidden.
const interiors=form?[form,...form.querySelectorAll('*')].filter(e=>e.namespaceURI==='http://www.w3.org/1999/xhtml'&&e.tagName!=='INPUT'):[];
const noInternalBoxes=interiors.every(noSurface)&&card.querySelectorAll('.zxs-form-grid').length===0
    &&(form?.querySelectorAll('svg[data-zxs-contour-decoration]').length===0);
let fieldsValid=false,stateValid=false,informationValid=false,actionsValid=false;
if(sixRows) {
    const [userLabel,user,passwordLabel,password,information,actions]=rows;
    const inputs=[...form.querySelectorAll('input')],submit=[...actions.querySelectorAll('button[type=submit]')];
    fieldsValid=inputs.length===2&&inputs[0]===user&&inputs[1]===password
        &&[user,password].every(e=>!e.disabled&&!e.readOnly&&e.tabIndex>=0)
        &&user.name==='username'&&user.type==='text'&&user.autocomplete==='username'
        &&password.name==='password'&&password.type==='password'
        &&[[userLabel,user],[passwordLabel,password]].every(([label,input])=>input.id!==''
            &&label.htmlFor===input.id&&label.control===input&&label.textContent.trim().length>0
            &&input.labels.length===1&&input.labels[0]===label);
    const setupMarkers=[...form.querySelectorAll('[data-setup-user],[data-setup-password],[data-setup-confirm]')];
    const checks=[...actions.querySelectorAll('[data-setup-check]')],clears=[...actions.querySelectorAll('[data-setup-clear]')];
    const signIns=[...actions.querySelectorAll('[data-setup-sign-in]')],unconfirmed=expectedSetup==='unconfirmed';
    const buttons=[...actions.querySelectorAll('button')];
    const statuses=[...information.querySelectorAll('[role=status]')],alerts=[...information.querySelectorAll('[role=alert]')];
    const errorPhase=(expectedSetup==='existing'&&phase==='invalid-login')||unconfirmed||rateLimited;
    informationValid=errorPhase?alerts.length===1&&statuses.length===0&&present(alerts[0])&&within(alerts[0])
            &&inside(rect(alerts[0]),rect(information))&&alerts[0].textContent.trim().length>0
        :expectedSetup==='existing'?statuses.length===1&&present(statuses[0])&&alerts.length===0
            &&statuses[0].textContent.trim()===''&&information.textContent.trim()===''
        :statuses.length===1&&alerts.length===0&&present(statuses[0])&&within(statuses[0])
            &&inside(rect(statuses[0]),rect(information))
            &&['请设置账户名和密码','Set a name and password.'].includes(statuses[0].textContent.trim());
    if(unconfirmed)informationValid&&=information.hasAttribute('data-setup-unknown')
        &&/Outcome unconfirmed\. Check setup status\.|结果未确认，请检查设置状态。/.test(alerts[0]?.textContent||'');
    const requestIds=[...information.querySelectorAll('.zxs-request-id')];
    informationValid&&=[...requestIds,...requestIds.flatMap(e=>[...e.querySelectorAll('code')])].every(e=>present(e)&&within(e)
        &&inside(rect(e),rect(information))&&e.textContent.trim().length>0);
    if(rateLimited)informationValid&&=alerts[0]?.textContent.includes('RESOURCE_LIMIT_EXCEEDED')
        &&requestIds.length===1&&requestIds[0].querySelectorAll('code').length===1;
    actionsValid=submit.length===1&&buttons.every(e=>e.parentElement===actions&&present(e)&&e.tabIndex>=0)
        &&actions.children.length===buttons.length&&controls.length===inputs.length+buttons.length
        &&buttons.every((e,i)=>buttons.slice(i+1).every(other=>{
            const a=rect(e),b=rect(other);return a.right<=b.x+.5||b.right<=a.x+.5||a.bottom<=b.y+.5||b.bottom<=a.y+.5;
        }));
    if(expectedSetup==='existing'||rateLimited)stateValid=user.required&&password.required&&password.autocomplete==='current-password'&&setupMarkers.length===0
        &&checks.length===(rateLimited?1:0)&&clears.length===0&&buttons.length===(rateLimited?2:1)&&submit.length===1&&!submit[0].disabled
        &&checks.every(e=>!e.disabled&&e.type==='button'&&e.getAttribute('aria-label')?.trim()&&e.getAttribute('title')?.trim())
        &&['登录','Sign in'].includes(submit[0].textContent.trim());
    if(expectedSetup==='empty'||unconfirmed)stateValid=password.autocomplete==='new-password'&&setupMarkers.length===2
        &&setupMarkers[0]===user&&setupMarkers[1]===password&&user.hasAttribute('data-setup-user')&&password.hasAttribute('data-setup-password')
        &&checks.length===1&&clears.length===1&&buttons.length===(unconfirmed?4:3)&&submit.length===1&&submit[0].disabled
        &&signIns.length===(unconfirmed?1:0)&&signIns.every(e=>!e.disabled&&e.type==='button'
            &&['尝试登录','Try signing in'].includes(e.textContent.trim()))
        &&['设置账户','Set up account'].includes(submit[0].textContent.trim())
        &&[checks[0],clears[0]].every(e=>!e.disabled&&e.type==='button'&&e.getAttribute('aria-label')?.trim()&&e.getAttribute('title')?.trim());
}
const allowedPhases=expectedSetup==='existing'?['initial','refreshed','invalid-login']
    :expectedSetup==='unconfirmed'?['response-lost']:rateLimited?['status-rejected']:['initial','refreshed','keyboard-focus'];
const rowGeometry=sixRows&&rows.every(e=>present(e)&&within(e)&&inside(rect(e),rect(form))
        &&rect(e).height>=sharedRowHeight&&parseFloat(getComputedStyle(e).minHeight)>=sharedRowHeight)&&separate(rowBoxes)
    &&rowBoxes.slice(0,4).every(r=>Math.abs(r.height-sharedRowHeight)<=.5)
    &&Math.abs(rowBoxes[1].x-rowBoxes[3].x)<=1&&Math.abs(rowBoxes[1].width-rowBoxes[3].width)<=1;
// Healthy desktop forms must actually fit six shared rows and 3:2, not merely
// declare a preferred ratio. Necessary errors and narrow layouts may grow.
const normalGeometry=compact||expectedSetup==='unconfirmed'||rateLimited||phase==='invalid-login'
    ||(sixRows&&rowBoxes.every(r=>Math.abs(r.height-sharedRowHeight)<=.5)
        &&Math.abs(panel.height-panel.width/1.5)<=.5);
return {valid:allowedPhases.includes(phase)&&formPresent&&sixRows&&fieldsValid&&stateValid&&informationValid&&actionsValid
    &&rowHeightValid&&noInternalBoxes&&rowGeometry&&normalGeometry&&visible.every(e=>e.valid)&&outerFrame&&preferredRatio&&panelGeometry
    &&headerTargets&&present(card)&&present(headerNode)&&document.documentElement.scrollWidth<=layoutWidth+1,
    viewport:{width:innerWidth,height:innerHeight,clientWidth:layoutWidth,scrollbarWidth:innerWidth-layoutWidth},
    documentWidth:document.documentElement.scrollWidth,header,headerTargets,headerControls:headerControls.map(rect),panel,expectedSetup,phase,formPresent,sixRows,fieldsValid,
    stateValid,informationValid,actionsValid,noInternalBoxes,rowGeometry,normalGeometry,rows:rowBoxes,controls:visible,
    outerFrame,outerPaint,restoredCorner,corner,rowHeightValid,rowHeightToken,sharedRowHeight,preferredRatio,aspectRatio:panelStyle.aspectRatio,panelGeometry};
})().then(done,error=>done({valid:false,error:String(error)}));
