// Unit models for probe classification only, not real DOM, SVG or browser evidence.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {runInNewContext} from 'node:vm';
const source=readFileSync(new URL('../runner/auth_layout.js',import.meta.url),'utf8');
const box=(y,height=44,width=240,x=40)=>({x,y,width,height,right:x+width,bottom:y+height});
function fixture(open=false) {
    const native={borderTopWidth:'1px',borderTopStyle:'solid',borderTopColor:'rgb(0, 0, 0)',borderImageSource:'none',backgroundImage:'none'};
    const node=(name,y,tagName='BUTTON')=>({name,textContent:name,tagName,parentElement:null,children:[],dataset:{},visible:true,isConnected:true,box:box(y),style:{...native},
        getBoundingClientRect(){return this.box;},
        checkVisibility(options){assert.equal(options.opacityProperty,true);assert.equal(options.visibilityProperty,true);return this.visible;},
        matches(selector){return selector==='.zxs-product-identity'&&this.name==='product';},
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
    const usernameLabel=node('username label',208),passwordLabel=node('password label',290);usernameLabel.box=box(208,24);passwordLabel.box=box(290,24);
    const rows=[usernameLabel,username,passwordLabel,password,login];
    const product=node('product',0);card.children=[product,section,form,details];for(const e of card.children)e.parentElement=card;
    const required={'[name=username]':username,'[name=password]':password,'button[type=submit]':login};
    form.querySelector=selector=>required[selector];form.querySelectorAll=()=>rows;
    details.querySelectorAll=selector=>selector.startsWith('input')?[setupInput]:[setupCheck,setupClear];
    card.querySelector=selector=>({'details':details,':scope > form':form,':scope > section button':manager})[selector];
    card.querySelectorAll=()=>controls;
    let decoded=0;
    class Image {
        naturalWidth=48;naturalHeight=48;
        async decode(){decoded++;if(this.src==='blob:broken')throw new Error('image decode failed');}
    }
    const document={documentElement:{scrollWidth:320},querySelector:selector=>selector==='.authentication-card'?card:header,
        createElement(tag){assert.equal(tag,'canvas');let image;
            return {width:0,height:0,getContext(){return {drawImage(value){image=value;},getImageData(x,y,width,height){
                const pixels=new Uint8ClampedArray(width*height*4);
                for(let row=0;row<height;row++)for(let column=0;column<width;column++){
                    const i=(row*width+column)*4;
                    const edge=width>1&&height>1&&(row===0||column===0||row===height-1||column===width-1)&&image.src!=='blob:blank';
                    pixels[i]=pixels[i+1]=pixels[i+2]=edge?0:255;pixels[i+3]=255;
                }return {data:pixels};
            }}}};}};
    return {controls,manager,username,password,login,setupInput,setupCheck,setupClear,details,required,rows,document,
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
console.log('PASS auth-layout probe classification, required controls, decoration contrast and negative cases (unit models only)');
