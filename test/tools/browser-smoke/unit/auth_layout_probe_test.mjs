// Unit models for probe classification only, not real DOM, SVG or browser evidence.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {runInNewContext} from 'node:vm';
const source=readFileSync(new URL('../runner/auth_layout.js',import.meta.url),'utf8');
const box=(y,height=44,width=244,x=38)=>({x,y,width,height,right:x+width,bottom:y+height});
let observations=0;
function fixture(empty=false) {
    const base={opacity:'1',borderImageSource:'none',backgroundImage:'none',backgroundColor:'transparent',boxShadow:'none',outlineWidth:'0px',outlineStyle:'none',outlineColor:'rgb(0, 0, 0)',outlineOffset:'0px',
        '--zxs-contour-extent':'0px','--zxs-compact-row-height':'36px',minHeight:'36px','--zxs-contour-panel':'calc(36px / 2)','--zxs-bg-panel':'#f2f2f2',aspectRatio:'auto',
        transform:'none',filter:'none',clipPath:'none',maskImage:'none',display:'block',visibility:'visible',
        getPropertyValue(name){return this[name]??'';}};
    for(const side of ['Top','Right','Bottom','Left'])Object.assign(base,{[`border${side}Width`]:'0px',[`border${side}Style`]:'none',[`border${side}Color`]:'rgb(0, 0, 0)'});
    const node=(name,y,tagName='DIV')=>({name:'',textContent:name,tagName,type:tagName==='INPUT'?'text':'button',namespaceURI:'http://www.w3.org/1999/xhtml',
        focused:false,disabled:false,checked:false,readOnly:false,required:false,autocomplete:'',tabIndex:0,id:'',htmlFor:'',parentElement:null,children:[],dataset:{},attributes:{},className:'',visible:true,isConnected:true,box:box(y),style:{...base},
        getBoundingClientRect(){return this.box;},getAttribute(name){return Object.hasOwn(this.attributes,name)?this.attributes[name]:null;},hasAttribute(name){return this.getAttribute(name)!==null;},
        checkVisibility(options){assert.equal(options.opacityProperty,true);assert.equal(options.visibilityProperty,true);return this.visible;},
        matches(selector){return selector.split(',').some(part=>{
            const s=part.trim();if(s===':focus-visible')return this.focused;const tag=s.match(/^[\w-]+/);
            return (!tag||tag[0].toLowerCase()===this.tagName.toLowerCase())
                &&[...s.matchAll(/\.([\w-]+)/g)].every(m=>this.className.split(' ').includes(m[1]))
                &&[...s.matchAll(/\[([\w-]+)(?:=["']?([^\]"']+)["']?)?\]/g)].every(([,key,value])=>{
                    const actual=['name','id','type'].includes(key)?this[key]:this.getAttribute(key);
                    return value===undefined?actual!==null:actual===value;
                });
        });},
        querySelectorAll(selector){
            const direct=selector.startsWith(':scope > '),s=direct?selector.slice(9):selector;
            const children=direct?this.children:this.children.flatMap(child=>[child,...child.querySelectorAll('*')]);
            return children.filter(child=>s==='*'||child.matches(s));
        },querySelector(selector){return this.querySelectorAll(selector)[0]??null;},
        contains(child){for(let n=child;n;n=n.parentElement)if(n===this)return true;return false;}});
    const append=(parent,...children)=>{parent.children.push(...children);for(const child of children)child.parentElement=parent;};
    const root=node('',0,'HTML');root.scrollWidth=320;root.clientWidth=320;
    const card=node('',100);card.className='authentication-card';card.box=box(100,216,292,14);card.style['--zxs-contour-extent']='18px';
    card.style.borderImageSource='url("blob:fill")';card.style.borderImageSlice='48.7805% fill';
    card.style.borderImageWidth='20px';card.style.borderImageOutset='2px';card.style.borderImageRepeat='stretch';card.dataset.zxsContourState='paint';
    const header=node('',14);header.className='authentication-language';header.box=box(14,44,96,210);append(root,header,card);
    const language=node('Language',14,'BUTTON'),theme=node('Theme',14,'BUTTON');language.box=box(14,44,44,210);theme.box=box(14,44,44,262);append(header,language,theme);
    const form=node('',100,'FORM');form.box=box(100,216);form.attributes[empty?'data-first-account-form':'data-auth-form']=empty?'':'login';append(card,form);
    const usernameLabel=node('Account name',100,'LABEL');usernameLabel.box=box(100,36);
    const username=node('',136,'INPUT'),passwordLabel=node('Password',172,'LABEL'),password=node('',208,'INPUT');passwordLabel.box=box(172,36);
    username.name='username';username.autocomplete='username';password.name='password';password.type='password';password.autocomplete=empty?'new-password':'current-password';
    for(const [input,label,key] of [[username,usernameLabel,'username'],[password,passwordLabel,'password']]){
        input.box.height=36;input.box.bottom=input.box.y+36;input.id=`zxs-${empty?'setup':'login'}-${key}`;input.required=true;input.labels=[label];label.htmlFor=input.id;label.control=input;
        input.className='zxs-input';input.style.borderBottomWidth='1px';input.style.borderBottomStyle='dashed';
    }
    if(empty){username.attributes['data-setup-user']='';password.attributes['data-setup-password']='';}
    const information=node('',244);information.className='zxs-auth-information';information.box=box(244,36);
    const status=node(empty?'Please set an account name and password.':'',244,'SPAN');status.attributes.role='status';status.box=box(244,36);append(information,status);
    const actions=node('',280);actions.className='zxs-auth-actions';actions.box=box(280,36);
    const submit=node(empty?'Set up account':'Sign in',280,'BUTTON');submit.box=box(280,36);submit.type='submit';submit.className='zxs-button';submit.disabled=empty;
    const check=node('',280,'BUTTON'),clear=node('',280,'BUTTON');
    for(const [button,key,text] of [[check,'check','Check setup status'],[clear,'clear','Clear setup inputs']]){
        button.className='zxs-button';button.attributes[`data-setup-${key}`]='';button.attributes['aria-label']=text;button.attributes.title=text;
    }
    if(empty){submit.box=box(280,36,140);check.box=box(280,36,44,186);clear.box=box(280,36,44,238);append(actions,submit,check,clear);}else append(actions,submit);
    append(form,usernameLabel,username,passwordLabel,password,information,actions);
    let decoded=0;
    const f={root,card,header,language,theme,form,usernameLabel,username,passwordLabel,password,information,status,actions,submit,check,clear,node,append,expectedSetup:empty?'empty':'existing',phase:'initial',innerWidth:320,innerHeight:568,
        decoded:()=>decoded,onDecode:()=>{},fillShape:'round',fillAlpha:255,fillColor:[242,242,242],tileWidth:41,tileHeight:41};
    class Image {
        naturalWidth=f.tileWidth;naturalHeight=f.tileHeight;
        async decode(){decoded++;f.onDecode(this.src);if(this.src==='blob:broken')throw new Error('image decode failed');}
    }
    const document={documentElement:root,querySelector:selector=>root.querySelector(selector),
        createElement(tag){assert.equal(tag,'canvas');let image,fill=[0,0,0,0];
            const context={drawImage(value){image=value;},fillRect(){},
                set fillStyle(value){if(/^#[0-9a-f]{6}$/i.test(value))fill=[...value.slice(1).match(/../g).map(n=>parseInt(n,16)),255];else fill=[0,0,0,0];},
                getImageData(x,y,width,height){
                    if(!image)return {data:new Uint8ClampedArray(fill)};
                    const pixels=new Uint8ClampedArray(width*height*4),w=image.naturalWidth,h=image.naturalHeight;
                    for(let py=0;py<height;py++)for(let px=0;px<width;px++){
                        const i=(py*width+px)*4,dx=px+.5-w/2,dy=py+.5-h/2;
                        const distance=Math.hypot(dx,dy),radius=Math.min(w,h)/2-2;
                        // Deliberate round-fill pixel model, not an SVG parser or renderer.
                        const shapes={round:distance<radius,blank:false,square:px>=2&&px<w-2&&py>=2&&py<h-2,
                            ring:distance<radius&&distance>radius-2,glyph:Math.abs(dx)<3&&Math.abs(dy)<3,
                            cross:Math.abs(dx)<2||Math.abs(dy)<2,solid:true};
                        assert.ok(Object.hasOwn(shapes,f.fillShape));
                        const ink=shapes[f.fillShape]&&image.src!=='blob:blank';
                        for(let c=0;c<3;c++)pixels[i+c]=f.blackRing&&distance>radius-1?0:f.fillColor[c];pixels[i+3]=ink?f.fillAlpha:0;
                    }
                    return {data:pixels};
                }};
            return {width:0,height:0,getContext(){return context;}};
        }};
    f.document=document;
    f.run=()=>{observations++;return new Promise(resolve=>runInNewContext(source,{arguments:[f.expectedSetup,f.phase,resolve],document,Image,
        innerWidth:f.innerWidth,innerHeight:f.innerHeight,getComputedStyle:node=>node.style}));};
    return f;
}

let f=fixture(),result=await f.run();assert.equal(result.valid,true,JSON.stringify(result));
f=fixture(true);result=await f.run();assert.equal(result.valid,true,JSON.stringify(result));
assert.equal(result.controls.length,5);assert.equal(result.sixRows,true);assert.equal(result.sharedRowHeight,36);
assert.equal(result.controls[0].paint.property,'input-underline');assert.equal(result.outerPaint.edgeFill.every(Boolean),true);
for(const empty of [false,true])for(const phase of empty?['initial','refreshed','keyboard-focus']:['initial','refreshed','invalid-login']) {
    f=fixture(empty);f.phase=phase;
    if(phase==='invalid-login'){f.status.attributes.role='alert';f.status.textContent='Enter your account name.';f.information.children=[f.status];f.status.parentElement=f.information;}
    assert.equal((await f.run()).valid,true,`${empty?'empty':'existing'} ${phase} six-row layout`);
}
// Reproduce the real error-only appearance leak. The panel-colored background
// can look identical in a screenshot, but it is still an internal surface.
f=fixture();f.phase='invalid-login';f.status.tagName='DIV';f.status.className='zxs-error zxs-row';
f.status.attributes.role='alert';f.status.textContent='Enter your account name.';
assert.equal((await f.run()).valid,true,'Unboxed ErrorState keeps the invalid-login gate passing');
for(const change of [
    e=>{e.style.backgroundColor='rgb(242, 242, 242)';},
    e=>{e.style['--zxs-contour-extent']='18px';},
    e=>{e.style.backgroundColor='rgb(242, 242, 242)';e.style['--zxs-contour-extent']='18px';},
]) {
    const previous={...f.status.style};change(f.status);const leaked=await f.run();
    assert.equal(leaked.valid,false);assert.equal(leaked.noInternalBoxes,false,'Appearance leak must not be exempted for errors');
    f.status.style=previous;
}
assert.equal((await f.run()).valid,true,'Restoring the anonymous override fixes only the leaked surface');
f=fixture(true);f.status.textContent='请设置账户名和密码';f.submit.textContent='设置账户';assert.equal((await f.run()).valid,true,'Chinese setup hint and action');
f=fixture(true);for(const input of [f.username,f.password])input.required=false;assert.equal((await f.run()).valid,true,'Setup admission belongs to the disabled submit, not browser required attributes');
f=fixture();f.username.focused=true;f.username.style.borderBottomWidth='3px';assert.equal((await f.run()).valid,true,'Focused dashed underline retains the shared row target');
// Preferred desktop 3:2 geometry never hides taller feedback; small viewports can grow naturally.
for(const [width,height] of [[320,568],[390,844],[980,760],[1440,900]]) {
    f=fixture(true);f.innerWidth=width;f.innerHeight=height;f.root.clientWidth=width;f.root.scrollWidth=width;
    if(width>416)f.card.style.aspectRatio='3 / 2';
    assert.equal((await f.run()).valid,true,`Model viewport ${width}x${height}, not browser evidence`);
}
f=fixture();f.phase='invalid-login';f.status.attributes.role='alert';f.status.textContent='A long error wraps onto more lines.';f.status.box=box(244,108);f.information.box=box(244,108);f.information.children=[f.status];f.status.parentElement=f.information;
for(const node of [f.actions,f.submit])node.box=box(352,36);f.form.box=box(100,288);f.card.box=box(100,288,292,14);
assert.equal((await f.run()).valid,true,'Feedback grows within row five, never inserts a seventh row');
f=fixture();f.root.clientWidth=305;f.root.scrollWidth=305;f.card.box=box(100,216,277,14);f.header.box=box(14,44,96,195);f.language.box=box(14,44,44,195);f.theme.box=box(14,44,44,247);
assert.equal((await f.run()).valid,true,'Content fits beside a classic scrollbar');
f.clear.box=box(280,36,44,290); // Not in Existing form.
assert.equal((await f.run()).valid,true,'Only the current state form is observed');
for(const empty of [false,true])for(const [name,change] of [
    ['unknown expected state',f=>{f.expectedSetup='unknown';}],
    ['wrong expected state',f=>{f.expectedSetup=f.expectedSetup==='empty'?'existing':'empty';}],
    ['unknown phase',f=>{f.phase='unknown';}],
    ['wrong phase',f=>{f.phase=f.expectedSetup==='empty'?'invalid-login':'keyboard-focus';}],
    ['missing form',f=>{f.card.children=[];}],
    ['duplicate form',f=>{f.append(f.card,f.node('',100,'FORM'));}],
    ['hidden form',f=>{f.form.visible=false;}],
    ['detached form',f=>{f.form.isConnected=false;}],
    ['wrong form owner',f=>{f.form.parentElement=f.root;}],
    ['extra card section',f=>{f.append(f.card,f.node('Long explanation',100,'SECTION'));}],
    ['missing row',f=>{f.form.children.splice(0,1);}],
    ['seventh row',f=>{f.append(f.form,f.node('extra',316));}],
    ['wrong row order',f=>{[f.form.children[0],f.form.children[1]]=[f.form.children[1],f.form.children[0]];}],
    ['wrapped field box',f=>{f.form.children[0].tagName='DIV';}],
    ['wrong hint row',f=>{f.information.className='';}],
    ['wrong action row',f=>{f.actions.className='';}],
    ['hidden label',f=>{f.usernameLabel.visible=false;}],
    ['empty label',f=>{f.usernameLabel.textContent='';}],
    ['missing input ID',f=>{f.username.id='';}],
    ['wrong label for',f=>{f.usernameLabel.htmlFor=f.password.id;}],
    ['wrong actual label control',f=>{f.usernameLabel.control=f.password;}],
    ['duplicate accessible label',f=>{f.username.labels.push(f.passwordLabel);}],
    ['missing accessible label',f=>{f.username.labels=[];}],
    ['hidden username',f=>{f.username.visible=false;}],
    ['hidden password',f=>{f.password.visible=false;}],
    ['disabled username',f=>{f.username.disabled=true;}],
    ['readonly password',f=>{f.password.readOnly=true;}],
    ['nonkeyboard username',f=>{f.username.tabIndex=-1;}],
    ['wrong username name',f=>{f.username.name='login';}],
    ['wrong password name',f=>{f.password.name='secret';}],
    ['wrong username autocomplete',f=>{f.username.autocomplete='off';}],
    ['wrong password autocomplete',f=>{f.password.autocomplete=f.expectedSetup==='empty'?'current-password':'new-password';}],
    ['unmasked password',f=>{f.password.type='text';}],
    ['unexpected checkbox',f=>{const checkbox=f.node('',316,'INPUT');checkbox.type='checkbox';f.append(f.actions,checkbox);}],
    ['hidden action',f=>{f.submit.visible=false;}],
    ['unlabeled action',f=>{f.submit.textContent='';}],
    ['wrong submit action',f=>{f.submit.textContent='Continue';}],
    ['nonkeyboard submit',f=>{f.submit.tabIndex=-1;}],
    ['missing submit',f=>{f.submit.type='button';}],
    ['duplicate submit',f=>{f.append(f.actions,{...f.submit});}],
    ['extra action wrapper',f=>{f.append(f.actions,f.node('',280));}],
    ['input too short',f=>{f.username.box=box(136,35);}],
    ['oversized input row',f=>{f.username.box.height=100;f.username.box.bottom=f.username.box.y+100;}],
    ['oversized label row',f=>{f.usernameLabel.box.height=72;f.usernameLabel.box.bottom=f.usernameLabel.box.y+72;}],
    ['label too short',f=>{f.usernameLabel.box=box(100,35);}],
    ['hint too short',f=>{f.information.box=box(244,35);}],
    ['action too short',f=>{f.submit.box=box(280,35);}],
    ['missing shared min height',f=>{f.usernameLabel.style.minHeight='0px';}],
    ['wrong row token',f=>{f.card.style['--zxs-compact-row-height']='44px';}],
    ['missing shared row token',f=>{f.root.style['--zxs-compact-row-height']='';}],
    ['row overlaps label',f=>{f.password.box=box(180,36);}],
    ['inputs misaligned',f=>{f.password.box=box(208,36,240,42);}],
    ['form outside panel',f=>{f.form.box=box(100,216,300);}],
    ['input outside panel',f=>{f.username.box=box(136,36,300);}],
    ['input under scrollbar',f=>{f.root.clientWidth=305;f.root.scrollWidth=305;f.username.box=box(136,36,268);}],
    ['document overflow',f=>{f.root.scrollWidth=322;}],
    ['panel under scrollbar',f=>{f.root.clientWidth=304;f.root.scrollWidth=304;}],
    ['panel too wide',f=>{f.innerWidth=980;f.root.clientWidth=980;f.root.scrollWidth=980;f.card.box=box(100,216,340,14);f.card.style.aspectRatio='3 / 2';}],
    ['panel clips lower rows',f=>{f.card.box=box(100,150,292,14);}],
    ['panel covers header',f=>{f.header.box=box(90,44,96,210);}],
    ['header overflow',f=>{f.header.box=box(14,44,96,230);}],
    ['hidden panel',f=>{f.card.visible=false;}],
    ['hidden header',f=>{f.header.visible=false;}],
    ['wrong desktop ratio',f=>{f.innerWidth=980;f.card.style.aspectRatio='1 / 1';}],
    ['independent corner token',f=>{f.card.style['--zxs-contour-panel']='42px';}],
    ['missing shared corner',f=>{f.root.style['--zxs-contour-panel']='';}],
    ['zero outer corner',f=>{f.card.style['--zxs-contour-extent']='0px';}],
    ['input side box',f=>{f.username.style.borderRightWidth='1px';}],
    ['input background',f=>{f.username.style.backgroundColor='rgb(255, 255, 255)';}],
    ['input background decoration',f=>{f.username.style.backgroundImage='url("blob:paint")';}],
    ['input outline box',f=>{f.username.style.outlineWidth='3px';f.username.style.outlineStyle='solid';}],
    ['action outline box',f=>{f.submit.style.outlineWidth='3px';f.submit.style.outlineStyle='solid';}],
    ['label outline box',f=>{f.usernameLabel.style.outlineWidth='3px';f.usernameLabel.style.outlineStyle='solid';}],
    ['hint outline box',f=>{f.information.style.outlineWidth='3px';f.information.style.outlineStyle='solid';}],
    ['input shadow box',f=>{f.username.style.boxShadow='0 0 0 1px black';}],
    ['input contour',f=>{f.username.style['--zxs-contour-extent']='18px';}],
    ['missing underline',f=>{f.username.style.borderBottomWidth='0px';}],
    ['wrong normal underline width',f=>{f.username.style.borderBottomWidth='2px';}],
    ['wrong focused underline width',f=>{f.username.focused=true;}],
    ['small toolbar target',f=>{f.language.box=box(14,43,44,210);}],
    ['solid underline',f=>{f.username.style.borderBottomStyle='solid';}],
    ['transparent underline',f=>{f.username.style.borderBottomColor='transparent';}],
    ['label border',f=>{f.usernameLabel.style.borderBottomWidth='1px';}],
    ['form border',f=>{f.form.style.borderTopWidth='1px';}],
    ['hint box',f=>{f.information.style.backgroundColor='white';}],
    ['action box',f=>{f.submit.style.borderLeftWidth='1px';}],
    ['action contour',f=>{f.submit.style['--zxs-contour-extent']='18px';}],
    ['legacy inner group',f=>{f.actions.className+=' zxs-form-grid';}],
    ['internal contour SVG',f=>{const svg=f.node('',0,'svg');svg.namespaceURI='http://www.w3.org/2000/svg';svg.attributes['data-zxs-contour-decoration']='foreground';f.append(f.information,svg);}],
    ['unexpected outer border',f=>{f.card.style.borderLeftWidth='1px';}],

    ['outer marker only',f=>{f.card.style.borderImageSource='none';}],
    ['blank outer paint',f=>{f.card.style.borderImageSource='url("blob:blank")';}],
    ['ring-only instead of fill',f=>{f.fillShape='ring';}],
    ['icon instead of outer frame',f=>{f.fillShape='glyph';}],
    ['outer image detached',f=>{f.card.isConnected=false;}],
    ['outer image decode failure',f=>{f.card.style.borderImageSource='url("blob:broken")';}],
]) {
    f=fixture(empty);change(f);result=await f.run();assert.equal(result.valid,false,`${empty?'empty':'existing'}: ${name}`);
}
for(const [name,change] of [
    ['missing setup hint',f=>{f.information.children=[];}],
    ['empty setup hint',f=>{f.status.textContent='';}],
    ['old long setup hint',f=>{f.status.textContent='Ready to create the administrator account. No disks are required.';}],
    ['wrong setup state hint',f=>{f.status.textContent='An installation already exists. Sign in.';}],
    ['hidden setup hint',f=>{f.status.visible=false;}],
    ['duplicate setup hint',f=>{f.append(f.information,{...f.status});}],
    ['setup hint overflow',f=>{f.status.box=box(244,36,400);}],
    ['setup hint clipping',f=>{f.status.box=box(244,100);}],
    ['setup error in healthy fixture',f=>{f.status.attributes.role='alert';}],
    ['missing setup username marker',f=>{delete f.username.attributes['data-setup-user'];}],
    ['missing setup password marker',f=>{delete f.password.attributes['data-setup-password'];}],
    ['unexpected confirm marker',f=>{f.submit.attributes['data-setup-confirm']='';}],
    ['premature enabled creation',f=>{f.submit.disabled=false;}],
    ['missing check',f=>{delete f.check.attributes['data-setup-check'];}],
    ['missing clear',f=>{delete f.clear.attributes['data-setup-clear'];}],
    ['disabled check',f=>{f.check.disabled=true;}],
    ['disabled clear',f=>{f.clear.disabled=true;}],
    ['unnamed icon check',f=>{delete f.check.attributes['aria-label'];}],
    ['untitled icon clear',f=>{delete f.clear.attributes.title;}],
    ['overlapping actions',f=>{f.clear.box={...f.check.box};}],
    ['nonbutton clear',f=>{f.clear.type='submit';}],
]) {f=fixture(true);change(f);assert.equal((await f.run()).valid,false,name);}
for(const [name,change] of [
    ['unexpected existing hint',f=>{f.status.textContent='Unexpected setup explanation';}],
    ['disabled login',f=>{f.submit.disabled=true;}],
    ['missing native login required',f=>{f.username.required=false;}],
    ['unexpected creation marker',f=>{f.username.attributes['data-setup-user']='';}],
    ['unexpected creation action',f=>{f.append(f.actions,f.check);}],
    ['missing expected local error',f=>{f.phase='invalid-login';}],
]) {f=fixture();change(f);assert.equal((await f.run()).valid,false,name);}
for(const empty of [false,true]) {
    f=fixture(empty);for(const input of [f.username,f.password])Object.defineProperty(input,'value',{get(){throw new Error('Probe must not read credentials');}});
    assert.equal((await f.run()).valid,true,'Layout probe never reads credential values or changes submit admission');assert.equal(f.submit.disabled,empty);
}

// The borderless outer panel is a decoded compact fill, not a dark ring.
f=fixture();result=await f.run();assert.equal(result.outerPaint.property,'outer-fill');assert.equal(result.outerPaint.width,41);assert.equal(result.outerPaint.height,41);
assert.deepEqual(Array.from(result.outerPaint.center),[242,242,242,255]);assert.ok(result.outerPaint.corners.every(a=>a===0));assert.equal(result.outerPaint.transparentMargin,true);
f=fixture();f.fillAlpha=200;assert.equal((await f.run()).valid,true,'Fill alpha threshold remains inclusive');
f=fixture();f.card.style['--zxs-bg-panel']='#252c31';f.root.style['--zxs-bg-panel']='#252c31';f.fillColor=[37,44,49];assert.equal((await f.run()).valid,true,'Actual dark-theme fill is compared with its own shared panel token');
f=fixture();f.submit.focused=true;f.submit.style.borderBottomWidth='3px';f.submit.style.borderBottomStyle='dashed';assert.equal((await f.run()).valid,true,'Action keyboard focus is a dashed underline, not a decorative box');
for(const [name,change] of [
    ['blank pixels',f=>{f.fillShape='blank';}],
    ['ring without fill',f=>{f.fillShape='ring';}],
    ['square fill instead of rounded corners',f=>{f.fillShape='square';}],
    ['entirely solid image',f=>{f.fillShape='solid';}],
    ['center glyph',f=>{f.fillShape='glyph';}],
    ['cross reaching four edges',f=>{f.fillShape='cross';}],
    ['weak fill',f=>{f.fillAlpha=199;}],
    ['black silhouette around gray fill',f=>{f.blackRing=true;}],
    ['wrong fill color',f=>{f.fillColor=[200,200,200];}],
    ['missing fill token',f=>{f.card.style['--zxs-bg-panel']='';}],
    ['wrong positive extent',f=>{f.card.style['--zxs-contour-extent']='99px';}],
    ['wrong tile width',f=>{f.tileWidth=43;}],
    ['wrong tile height',f=>{f.tileHeight=43;}],
    ['slice consumes center',f=>{f.card.style.borderImageSlice='50% fill';}],
    ['slice without fill',f=>{f.card.style.borderImageSlice='48.7805%';}],
    ['nonpercentage slice',f=>{f.card.style.borderImageSlice='20 fill';}],
    ['wrong physical corner width',f=>{f.card.style.borderImageWidth='18px';}],
    ['wrong margin placement',f=>{f.card.style.borderImageOutset='0px';}],
    ['repeated rather than stretched tile',f=>{f.card.style.borderImageRepeat='repeat';}],
    ['background decoration hiding the fill',f=>{f.card.style.backgroundImage='url("blob:fill")';}],
    ['solid native background hides rounded corners',f=>{f.card.style.backgroundColor='#f2f2f2';}],
    ['native border replaces fill',f=>{f.card.style.borderTopWidth='1px';}],
    ['native outer outline',f=>{f.card.style.outlineWidth='3px';f.card.style.outlineStyle='solid';}],
    ['private fill token',f=>{f.card.style['--zxs-bg-panel']='#c8c8c8';f.fillColor=[200,200,200];}],
    ['faded host',f=>{f.card.style.opacity='0.5';}],
    ['shadow behind fill',f=>{f.card.style.boxShadow='0 0 0 1px black';}],
    ['filtered fill',f=>{f.card.style.filter='blur(1px)';}],
    ['transformed fill',f=>{f.card.style.transform='matrix(1,0,0,1,1,1)';}],
    ['masked fill',f=>{f.card.style.maskImage='url("blob:mask")';}],
    ['clipped fill',f=>{f.card.style.clipPath='inset(10px)';}],
    ['wrong contour state',f=>{f.card.dataset.zxsContourState='fallback';}],
    ['unexpected foreground SVG',f=>{const svg=f.node('',100,'svg');svg.namespaceURI='http://www.w3.org/2000/svg';svg.attributes['data-zxs-contour-decoration']='foreground';f.append(f.card,svg);}],
]) {f=fixture();change(f);assert.equal((await f.run()).valid,false,name);}
// A live source can change while image.decode awaits. Every captured paint and
// identity property must still belong to this same connected card afterward.
for(const [name,change] of [
    ['host detached',f=>{f.card.isConnected=false;}],
    ['host reparented',f=>{f.card.parentElement=f.form;}],
    ['host replaced',f=>{f.root.children[f.root.children.indexOf(f.card)]={...f.card};}],
    ['host hidden',f=>{f.card.visible=false;}],
    ['source replaced',f=>{f.card.style.borderImageSource='url("blob:other")';}],
    ['state changed',f=>{f.card.dataset.zxsContourState='clip';}],
    ['decoration inserted',f=>{const svg=f.node('',100,'svg');svg.attributes['data-zxs-contour-decoration']='foreground';f.append(f.card,svg);}],
    ...['x','y','width','height'].map(axis=>[`host ${axis} changed`,f=>{f.card.box[axis]+=.25;}]),
    ...['opacity','backgroundColor','backgroundImage','borderImageSlice','borderImageWidth','borderImageOutset','borderImageRepeat',
        'outlineWidth','outlineStyle','outlineColor','outlineOffset','boxShadow','transform','filter','clipPath','maskImage','display','visibility',
        ...['Top','Right','Bottom','Left'].flatMap(side=>['Width','Style','Color'].map(part=>`border${side}${part}`)),
        '--zxs-contour-extent','--zxs-bg-panel','--zxs-compact-row-height','--zxs-contour-panel'].map(property=>
        [`host ${property} changed`,f=>{f.card.style[property]='changed';}]),
    ['root row changed',f=>{f.root.style['--zxs-compact-row-height']='40px';}],
    ['root fill changed',f=>{f.root.style['--zxs-bg-panel']='#ffffff';}],
    ['root corner changed',f=>{f.root.style['--zxs-contour-panel']='20px';}],
]) {
    f=fixture();f.onDecode=()=>change(f);result=await f.run();assert.equal(result.valid,false,`decode: ${name}`);
}
// Product wiring is separate evidence from the DOM/pixel models above. The
// explicit models-only option is for isolated worktrees awaiting production edits.
if(!process.argv.includes('--models-only')) {
    const host=readFileSync(new URL('../runner/auth_layout.rs',import.meta.url),'utf8');
    const management=readFileSync(new URL('../runner/management.rs',import.meta.url),'utf8');
    const setupSource=readFileSync(new URL('../../../../src/web/src/setup.rs',import.meta.url),'utf8');
    const loginSource=readFileSync(new URL('../../../../src/interface/src/components/login.rs',import.meta.url),'utf8');
    const authCss=readFileSync(new URL('../../../../src/web/public/styles/components/auth.css',import.meta.url),'utf8');
    const geometryCss=readFileSync(new URL('../../../../src/web/public/styles/components/geometry.css',import.meta.url),'utf8');
    const paintSource=readFileSync(new URL('../../../../src/interface/src/contours/paint.rs',import.meta.url),'utf8');
    assert.match(host,/run_with_setup\(browser, origin, output, SetupState::Existing\)/);
    assert.match(host,/run_with_setup\(browser, origin, output, SetupState::Empty\)/);
    assert.match(host,/\(320, 568\), \(390, 844\), \(980, 760\), \(1440, 900\)/);
    assert.match(host,/"args": \[setup.name\(\), phase\]/);
    assert.match(host,/for phase in \["initial", "refreshed", setup.final_phase\(\)\]/);
    assert.match(host,/Self::Existing => "invalid-login"/);assert.match(host,/Self::Empty => "keyboard-focus"/);
    assert.match(host,/browser.command\("POST", "\/refresh"/);
    assert.match(host,/:focus-visible/);assert.match(host,/\.zxs-auth-information \.zxs-error/);
    assert.ok(host.indexOf('window.scrollTo(0,0)')<host.indexOf('Duration::from_millis(150)'));
    assert.ok(host.indexOf('Duration::from_millis(150)')<host.indexOf('"/execute/async"'));
    assert.ok(host.indexOf('dir.join(&name)')<host.indexOf('if geometry["valid"] != true'));
    assert.ok(host.indexOf('dir.join("login-layout.json")')<host.indexOf('if geometry["valid"] != true'));
    assert.match(host,/Emulation.clearDeviceMetricsOverride/);
    assert.match(management,/join\("first-account"\)/);
    const fresh=management.slice(management.indexOf('pub(super) fn setup('),management.indexOf('struct SetupFixture'));
    assert.ok(fresh.indexOf('fixture.visit(browser)?')<fresh.indexOf('super::auth_layout::first_account('));
    assert.ok(fresh.indexOf('super::auth_layout::first_account(')<fresh.indexOf('fixture.open(browser)?'));
    assert.ok(fresh.indexOf('super::auth_layout::first_account(')<fresh.indexOf('credentials('));
    assert.match(setupSource,/<form data-first-account-form=""/);assert.match(loginSource,/<form data-auth-form="login"/);
    for(const text of ['请设置账户名和密码','Please set an account name and password.','设置账户','Set up account'])assert.ok(setupSource.includes(text));
    for(const marker of ['data-setup-check','data-setup-clear','data-setup-user','data-setup-password'])assert.ok(setupSource.includes(marker));
    assert.doesNotMatch(setupSource+loginSource,/data-setup-confirm|zxs-form-grid|<details|<summary/);
    // Exact source guard, not a substitute CSS engine: three class selectors
    // (0,3,0) outrank the appearance root + error selector (0,2,1), in either order.
    const appearanceCss=readFileSync(new URL('../../../../src/web/public/styles/components/content-blocks.css',import.meta.url),'utf8');
    assert.match(appearanceCss,/html:not\(\[data-zxs-appearance\]\), html\[data-zxs-appearance="content-blocks"\]/);
    assert.match(appearanceCss,/\.zxs-error,\.zxs-empty,\.zxs-loading \{[^}]*background: var\(--zxs-bg-panel\);[^}]*--zxs-contour-extent: var\(--zxs-contour-notification\)/);
    const strong='.authentication-shell .authentication-card :is(.zxs-auth-information, .zxs-auth-actions, .zxs-error, .zxs-button)';
    const weak=strong.replace('.authentication-shell ','');
    const unboxedRule=css=>{
        const line=css.split('\n').find(line=>line.startsWith(`${strong} {`));
        return !!line&&['border: 0;','background: transparent;','box-shadow: none;',
            '--zxs-contour-extent: 0px;','--zxs-contour-mode: none;','--zxs-contour-foreground: 0;'].every(declaration=>line.includes(declaration));
    };
    assert.equal(unboxedRule(authCss),true,'Anonymous override must beat either appearance-root selector');
    assert.equal(unboxedRule(authCss.replace(strong,weak)),false,'The exact previous two-class rule must fail');
    for(const declaration of ['background: transparent;','--zxs-contour-extent: 0px;']) {
        const line=authCss.split('\n').find(line=>line.startsWith(`${strong} {`));
        assert.equal(unboxedRule(authCss.replace(line,line.replace(declaration,''))),false,`Missing ${declaration} must fail`);
    }
    assert.match(authCss,/width: min\(calc\(var\(--zxs-compact-row-height\) \* 9\), 100%\)/);
    assert.match(authCss,/grid-template-rows: repeat\(6, minmax\(var\(--zxs-compact-row-height\), max-content\)\)/);
    assert.match(authCss,/min-height: var\(--zxs-compact-row-height\)/);
    assert.match(authCss,/aspect-ratio: 3 \/ 2/);assert.match(authCss,/aspect-ratio: auto/);
    assert.match(authCss,/--zxs-contour-extent: var\(--zxs-contour-panel\)/);
    assert.doesNotMatch(authCss,/--zxs-contour-(?:panel|control|notification|dialog):/);
    assert.match(geometryCss,/--zxs-contour-control: calc\(var\(--zxs-compact-row-height\) \/ 2\)/);
    assert.match(geometryCss,/--zxs-contour-panel: var\(--zxs-contour-control\)/);
    assert.match(geometryCss,/\.zxs-app-shell \.zxs-frame \{[^}]*--zxs-contour-extent: var\(--zxs-contour-panel\)/);
    assert.match(paintSource,/let margin = 2\.0;/);assert.match(paintSource,/let size = extent \* 2\.0 \+ 1\.0 \+ margin \* 2\.0;/);
    assert.match(paintSource,/slice: Some\(extent \+ margin\)/);
    console.log('PASS auth-layout source wiring, four-viewport driver and shared geometry contracts (not browser acceptance)');
}
console.log(`PASS ${observations} ${process.argv.includes('--models-only')?'models-only ':''}auth-layout six-row/state/accessibility/shared-geometry and live G4 fill classification (unit models only; no browser rendering)`);
