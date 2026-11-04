// Unit models for probe classification only, not real DOM, SVG or browser evidence.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {runInNewContext} from 'node:vm';
const source=readFileSync(new URL('../runner/auth_layout.js',import.meta.url),'utf8');
const box=(y,height=44,width=240,x=40)=>({x,y,width,height,right:x+width,bottom:y+height});
function fixture(open=false) {
    const native={borderTopWidth:'1px',borderTopStyle:'solid',borderTopColor:'rgb(0, 0, 0)',borderImageSource:'none',backgroundImage:'none'};
    for(const side of ['Right','Bottom','Left'])for(const property of ['Width','Style','Color'])native[`border${side}${property}`]=native[`borderTop${property}`];
    const node=(name,y,tagName='BUTTON')=>({name,textContent:name,tagName,parentElement:null,children:[],dataset:{},visible:true,isConnected:true,box:box(y),style:{...native},
        getBoundingClientRect(){return this.box;},
        checkVisibility(options){assert.equal(options.opacityProperty,true);assert.equal(options.visibilityProperty,true);return this.visible;},
        matches(selector){return selector==='.zxs-product-identity'&&this.name==='product';},
        closest(selector){for(let n=this;n;n=n.parentElement)if(n.selector===selector)return n;return null;},
        contains(child){for(let n=child;n;n=n.parentElement)if(n===this)return true;return false;}});
    const card=node('card',105,'DIV');card.box=box(105,open?800:415,292,14);
    const header=node('header',14);header.box=box(14,44,96,210);
    const section=node('management',119,'SECTION');section.box=box(119,75);
    const form=node('form',208,'FORM');form.box=box(208,239);
    const details=node('details',461,'DETAILS');details.box=box(461,open?430:45);details.open=open;
    const summary=node('summary',461,'SUMMARY');summary.parentElement=details;details.children=[summary];
    const manager=node('manager',150), username=node('username',239,'INPUT'), password=node('password',321,'INPUT'), login=node('login',403);
    manager.parentElement=section;for(const e of [username,password,login])e.parentElement=form;
    const setupInput=node('setupInput',652,'INPUT'),setupCheck=node('setupCheck',706),setupClear=node('setupClear',771);
    for(const e of [setupInput,setupCheck,setupClear]){e.parentElement=details;e.visible=open;}
    for(const e of [manager,setupCheck,setupClear]){e.style.borderTopColor='rgba(0, 0, 0, 0)';e.style.backgroundImage='url("blob:paint")';e.dataset.zxsContourState='paint';}
    const controls=[manager,username,password,login,setupInput,setupCheck,setupClear];
    const field=(name,control,y)=>{
        const wrapper=node(name+' field',y,'LABEL');wrapper.selector='label.zxs-form-field';
        const label=node(name+' label',y+10,'SPAN');label.box=box(y+10,24,72);
        for(const side of ['Top','Right','Bottom','Left']){control.style[`border${side}Width`]='0px';wrapper.style[`border${side}Width`]='0px';}
        control.style.borderBottomWidth='1px';control.style.borderBottomStyle='dashed';
        control.style.backgroundColor='transparent';control.style.boxShadow='none';
        control.box=box(y,44,152,128);label.parentElement=wrapper;control.parentElement=wrapper;
        wrapper.children=[label,control];wrapper.querySelector=selector=>selector===':scope > span'?label:control;
        return {wrapper,label,control};
    };
    const usernameField=field('username',username,239),passwordField=field('password',password,321),setupField=field('setup',setupInput,652);
    const group=(name,parent,fields)=>{
        const g=node(name,fields[0].wrapper.box.y,'DIV');g.selector='.zxs-form-grid';g.parentElement=parent;
        g.box=box(g.box.y,fields.at(-1).wrapper.box.bottom-g.box.y);g.children=fields.map(f=>f.wrapper);
        g.querySelectorAll=()=>g.children;for(const e of g.children)e.parentElement=g;return g;
    };
    const loginGroup=group('login group',form,[usernameField,passwordField]);
    const setupGroup=group('setup group',details,[setupField]);
    for(const e of [setupGroup,setupField.wrapper,setupField.label])e.visible=open;
    const rows=[loginGroup,login];form.children=rows;
    const product=node('product',0);card.children=[product,section,form,details];for(const e of card.children)e.parentElement=card;
    const required={'[name=username]':username,'[name=password]':password,'button[type=submit]':login};
    form.querySelector=selector=>required[selector];
    details.querySelectorAll=selector=>selector.startsWith('input')?[setupInput]:[setupCheck,setupClear];
    card.querySelector=selector=>({'details':details,':scope > form':form,':scope > section button':manager})[selector];
    card.querySelectorAll=selector=>selector==='.zxs-form-grid'?[loginGroup,setupGroup]:controls;
    let decoded=0;
    class Image {
        naturalWidth=48;naturalHeight=48;
        async decode(){decoded++;if(this.src==='blob:broken')throw new Error('image decode failed');}
    }
    const document={documentElement:{scrollWidth:320,clientWidth:320},querySelector:selector=>selector==='.authentication-card'?card:header,
        createElement(tag){assert.equal(tag,'canvas');let image;
            return {width:0,height:0,getContext(){return {drawImage(value){image=value;},getImageData(x,y,width,height){
                const pixels=new Uint8ClampedArray(width*height*4);
                for(let row=0;row<height;row++)for(let column=0;column<width;column++){
                    const i=(row*width+column)*4;
                    const edge=width>1&&height>1&&(row===0||column===0||row===height-1||column===width-1)&&image.src!=='blob:blank';
                    pixels[i]=pixels[i+1]=pixels[i+2]=edge?0:255;pixels[i+3]=255;
                }return {data:pixels};
            }}}};}};
    return {card,header,form,controls,manager,username,password,login,setupInput,setupCheck,setupClear,details,required,rows,document,node,field,usernameField,passwordField,setupField,loginGroup,setupGroup,
        async run(){return new Promise(resolve=>runInNewContext(source,{arguments:[resolve],document,Image,innerWidth:320,innerHeight:568,getComputedStyle:node=>node.style}));},
        decoded:()=>decoded};
}
let f=fixture(),result=await f.run();
assert.equal(result.valid,true);assert.equal(result.excludedClosedDisclosure,3);assert.equal(result.controls.length,4);assert.equal(f.decoded(),1);
assert.equal(result.controls[0].paint.property,'backgroundImage');assert.ok(result.controls[0].paint.contrastingPixels>0);
f=fixture();f.setupInput.visible=true;result=await f.run();assert.equal(result.valid,false,'Closed content that actually renders must fail');assert.equal(result.closedContentHidden,false);
f=fixture(true);result=await f.run();assert.equal(result.valid,true);assert.equal(result.controls.length,7);assert.equal(result.excludedClosedDisclosure,0);
f=fixture(true);f.setupInput.visible=false;result=await f.run();assert.equal(result.valid,false);assert.equal(result.controls.length,7,'Unexpectedly hidden expanded controls must not be filtered away');
f=fixture();f.manager.style.backgroundImage='url("blob:blank")';result=await f.run();assert.equal(result.valid,false);assert.equal(result.controls[0].paint.contrastingPixels,0);
f=fixture();f.manager.style.borderImageSource='url("blob:paint")';f.manager.style.backgroundImage='none';result=await f.run();assert.equal(result.valid,true);assert.equal(result.controls[0].paint.property,'borderImageSource');
f=fixture();f.manager.isConnected=false;result=await f.run();assert.equal(result.valid,false,'Detached decoration cannot count as paint');
f=fixture();f.manager.style.backgroundImage='none';result=await f.run();assert.equal(result.valid,false,'A G4 state marker alone is not paint');
f=fixture();f.manager.style.backgroundImage='url("blob:broken")';result=await f.run();assert.equal(result.valid,false);assert.match(result.error,/decode failed/);
f=fixture();f.required['[name=username]']=null;result=await f.run();assert.equal(result.valid,false);assert.equal(result.requiredPresent,false);
f=fixture();f.username.visible=false;result=await f.run();assert.equal(result.valid,false,'Hidden login controls must fail');
f=fixture();f.password.box=box(250);result=await f.run();assert.equal(result.valid,false,'Overlapping rows must still fail');
f=fixture();f.username.box=box(239,43);result=await f.run();assert.equal(result.valid,false,'Small hit targets must still fail');
f=fixture();f.username.box=box(239,44,300);result=await f.run();assert.equal(result.valid,false,'Horizontal overflow must still fail');
// Classic scrollbars reserve layout width even though innerWidth stays 320.
f=fixture(true);f.document.documentElement.clientWidth=305;result=await f.run();assert.equal(result.valid,false,'A 320px root overflowing a 305px content viewport must fail');assert.equal(result.viewport.scrollbarWidth,15);
f=fixture(true);f.document.documentElement.clientWidth=305;f.document.documentElement.scrollWidth=305;f.card.box=box(105,800,277,14);f.header.box=box(14,44,96,195);result=await f.run();assert.equal(result.valid,true,'Content that fits beside a classic scrollbar must pass');
f.setupClear.box=box(771,44,275);result=await f.run();assert.equal(result.valid,false,'A setup control under the scrollbar must fail even if the reported document width fits');
f=fixture(true);f.document.documentElement.clientWidth=305;f.document.documentElement.scrollWidth=305;f.card.box=box(105,800,292,24);result=await f.run();assert.equal(result.valid,false,'The card must also fit the layout viewport');
f=fixture(true);f.document.documentElement.clientWidth=305;f.document.documentElement.scrollWidth=305;f.card.box=box(105,800,277,14);f.header.box=box(14,44,96,219);result=await f.run();assert.equal(result.valid,false,'Header controls cannot extend under the scrollbar');
f=fixture(true);result=await f.run();assert.equal(result.valid,true,'Overlay scrollbars do not reserve layout width');assert.equal(result.viewport.scrollbarWidth,0);
// Optional feedback can add content without a fixed row count or reserved slot.
f=fixture();const feedback={...f.password,name:'conditional error',box:box(380,64)};f.rows.splice(1,0,feedback);f.login.box=box(460);f.form.box=box(208,296);f.details.box=box(518,45);f.card.box=box(105,472,292,14);result=await f.run();assert.equal(result.valid,true,'Variable-height conditional feedback must stay in the normal content flow');
// Natural label lengths do not determine each control's column.
f=fixture();f.usernameField.label.box=box(249,24,30);f.passwordField.label.box=box(331,24,80);result=await f.run();assert.equal(result.valid,true,'Varied label lengths retain a shared control edge');
f=fixture();f.usernameField.label.box=box(225,72,72);f.usernameField.wrapper.box=box(225,72);f.loginGroup.box=box(225,140);result=await f.run();assert.equal(result.valid,true,'Wrapped labels may be taller than controls without stacking them');
f=fixture();f.password.box=box(321,44,140,140);result=await f.run();assert.equal(result.valid,false,'Individually valid same-row fields must share a control column');assert.equal(result.formGroups[0].aligned,false);
f=fixture();f.usernameField.label.box=box(208,24);f.username.box=box(239);f.usernameField.wrapper.box=box(208,75);f.loginGroup.box=box(208,157);result=await f.run();assert.equal(result.valid,false,'Labels stacked above inputs must fail');assert.equal(result.formGroups[0].fields[0].sameRow,false);
f=fixture();f.passwordField.wrapper.box=box(270);result=await f.run();assert.equal(result.valid,false,'Overlapping field wrappers must fail');
f=fixture(true);f.setupField.label.box=box(620,24,72);result=await f.run();assert.equal(result.valid,false,'Expanded setup fields obey the same-row contract too');
f=fixture();f.usernameField.label.visible=false;result=await f.run();assert.equal(result.valid,false,'Unexpectedly hidden labels must fail');
f=fixture();const extra=f.field('optional text',f.node('optional text',375,'TEXTAREA'),375);extra.wrapper.parentElement=f.loginGroup;f.loginGroup.children.push(extra.wrapper);f.controls.push(extra.control);f.loginGroup.box=box(239,180);f.login.box=box(433);f.form.box=box(208,269);f.details.box=box(491,45);f.card.box=box(105,445,292,14);result=await f.run();assert.equal(result.valid,true,'Additional textarea rows need no fixed field count');assert.equal(result.formGroups[0].fields.length,3);
f=fixture();for(const field of [f.usernameField,f.passwordField]){field.label.box=box(field.label.box.y,24,48,24);field.control.box=box(field.control.box.y,44,196,92);field.wrapper.box=box(field.wrapper.box.y,44,264,24);}f.loginGroup.box=box(239,126,264,24);result=await f.run();assert.equal(result.valid,true,'Responsive column widths and edges are not fixed pixel contracts');
// Interior controls have only a dashed underline; the shared group supplies the outer frame.
f=fixture();result=await f.run();assert.equal(result.controls.find(c=>c.name==='username').paint.property,'form-group');assert.equal(result.controls.find(c=>c.name==='username').paint.underline,true);
f=fixture();f.username.style.borderRightWidth='1px';result=await f.run();assert.equal(result.valid,false,'Any independent interior input border must fail');
f=fixture();f.username.style.backgroundColor='rgb(255, 255, 255)';result=await f.run();assert.equal(result.valid,false,'Interior input surfaces must remain transparent');
f=fixture();f.username.style.boxShadow='0 0 0 1px black';result=await f.run();assert.equal(result.valid,false,'An inset shadow cannot replace the removed input box');
f=fixture();f.loginGroup.style.borderLeftWidth='0px';result=await f.run();assert.equal(result.valid,false,'The outer form frame must be complete');
f=fixture();f.loginGroup.visible=false;result=await f.run();assert.equal(result.valid,false,'Inputs cannot borrow paint from a hidden group');
f=fixture();f.passwordField.wrapper.style.borderTopWidth='1px';result=await f.run();assert.equal(result.valid,false,'Full-row separators must not return');
f=fixture();f.username.style.borderBottomColor='transparent';result=await f.run();assert.equal(result.valid,false,'The input underline must actually be painted');
f=fixture();f.loginGroup.style.borderTopColor='transparent';f.loginGroup.style.borderImageSource='url("blob:paint")';f.loginGroup.dataset.zxsContourState='paint';result=await f.run();assert.equal(result.valid,true,'A decoded G4 outer frame supports underlined controls');assert.equal(result.formGroups[0].paint.property,'borderImageSource');assert.equal(f.decoded(),2);
f.loginGroup.style.borderImageSource='url("blob:blank")';result=await f.run();assert.equal(result.valid,false,'A blank G4 outer frame cannot supply control paint');
f.loginGroup.style.borderImageSource='none';result=await f.run();assert.equal(result.valid,false,'The outer frame G4 marker alone is insufficient');
f.loginGroup.style.borderImageSource='url("blob:broken")';result=await f.run();assert.equal(result.valid,false);assert.match(result.error,/decode failed/);
f=fixture();f.username.style.borderBottomWidth='0px';result=await f.run();assert.equal(result.valid,false,'Missing input underline must fail');
f=fixture();f.username.style.borderBottomStyle='solid';result=await f.run();assert.equal(result.valid,false,'A solid input underline is not the requested dash');
f=fixture();f.usernameField.wrapper.style.borderBottomWidth='1px';f.usernameField.wrapper.style.borderBottomStyle='dashed';result=await f.run();assert.equal(result.valid,false,'Even a dashed line must not span the entire label row');
f=fixture();result=await f.run();assert.equal(result.valid,true,'Only each right-hand input has a dashed underline');assert.equal(result.formGroups[0].fields.every(field=>field.noRowBorder),true);
console.log('PASS auth-layout probe classification, required controls, decoration contrast and negative cases (unit models only)');
