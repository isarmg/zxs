// Host-side observer contract tests, not evidence of real G4/browser rendering.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {runInNewContext} from 'node:vm';
const source=readFileSync(new URL('../runner/select_borders.js',import.meta.url),'utf8');
// Only the expanded menu is outlined; its trigger and native text controls
// remain borderless until the distinct keyboard focus indicator appears.
const css=readFileSync(new URL('../../../../src/web/public/styles/components/select.css',import.meta.url),'utf8');
const authCss=readFileSync(new URL('../../../../src/web/public/styles/components/auth.css',import.meta.url),'utf8');
const history=readFileSync(new URL('../../../../src/web/src/history.rs',import.meta.url),'utf8');
assert.match(history,/<div\s+class="zxs-history-toolbar\b[^"]*">\s*<Select\b/,
    'The history toolbar exercised by the browser gate must use the shared Select');
assert.doesNotMatch(authCss,/\.authentication-card \.zxs-input\s*\{[^}]*border:/);
const baseCss=readFileSync(new URL('../../../../src/web/public/styles/components/base.css',import.meta.url),'utf8');
assert.match(baseCss,/:root :is\(input,textarea\)\.zxs-input\s*\{[^}]*border: 0;/,
    'Borderless text policy must target native controls, not the button-based Select');
assert.match(css,/\.zxs-select-trigger\.zxs-input\s*\{\s*border: 0;/,
    'Collapsed Select must not retain a decorative border');
assert.match(css,/\.zxs-select \.zxs-select-panel\s*\{[^}]*border: var\(--zxs-select-border-width\) solid color-mix\(in srgb, var\(--zxs-bg-panel\) 85%, #000\);/,
    'Expanded Select has a thicker, slightly darker version of the unchanged panel fill');
const selectSource=readFileSync(new URL('../../../../src/interface/src/components/select.rs',import.meta.url),'utf8');
assert.match(selectSource,/<button\s+class="zxs-input zxs-select-trigger zxs-input--compact"[^>]*role="combobox"/,
    'Select must remain a button-based combobox, outside the native text policy');
assert.match(selectSource,/let panel_width = bounds\.width\(\)\.min\(\(width - 16\.0\)\.max\(0\.0\)\);/,
    'Popup must use the trigger border-box width without a separate minimum');
assert.match(selectSource,/let left = bounds\.left\(\)\.min\(width - panel_width - 8\.0\)\.max\(8\.0\);/,
    'Popup must keep the existing viewport inset');
assert.match(selectSource,/style\.set_property\("width", &format!\("\{panel_width\}px"\)\)\?;/,
    'Calculated popup width must reach the real panel');
assert.match(css,/\.zxs-select \.zxs-select-panel\s*\{[^}]*box-sizing:\s*border-box;/,
    'Popup CSS width must include its border');
function fixture() {
    const style={opacity:'1',borderImageSource:'none',backgroundImage:'none',backgroundColor:'#c8c8c8',
        palette:'#c8c8c8',getPropertyValue(){return this.palette;}};
    for(const side of ['Top','Right','Bottom','Left'])Object.assign(style,{[`border${side}Width`]:'2px',[`border${side}Style`]:'solid',[`border${side}Color`]:'rgb(170, 170, 170)'});
    const node=role=>({style:{...style},isConnected:true,visible:true,dataset:{zxsContourState:'clip'},box:{x:0,y:0,width:100,height:36},
        checkVisibility(){return this.visible;},getBoundingClientRect(){return this.box;},hover:false,matches(){return this.hover;},closest(){return null;},querySelector(){return null;},anchor:'local',getAttribute(name){return name==='role'?role:this.anchor;}});
    const trigger=node('combobox'),panel=node('listbox');
    for(const side of ['Top','Right','Bottom','Left'])Object.assign(trigger.style,{[`border${side}Width`]:'0px',[`border${side}Style`]:'none'});
    const f={trigger,panel,pixelColor:170,edgeWidth:2,missingEdge:false,centerColor:200,centerAlpha:255,detached:false,decodeFails:false,foreground:null,revoked:[],created:[]};
    f.withForeground=()=>{
        f.centerAlpha=0;
        const path={style:{fill:'rgb(170, 170, 170)',fillRule:'evenodd',stroke:'none'},getAttribute(name){assert.equal(name,'d');return 'M0 0h100v36H0Z M2 2v32h96V2Z';}};
        f.foreground={isConnected:true,visible:true,box:{...panel.box},path,style:{position:'absolute'},
            checkVisibility(){return this.visible;},getBoundingClientRect(){return this.box;},querySelector(){return path;},
            getAttribute(name){assert.equal(name,'viewBox');return '0 0 100 36';}};
        panel.querySelector=()=>f.foreground;
        return f;
    };
    f.run=()=>new Promise(resolve=>{
        const color=value=>{
            if(value==='transparent')return [0,0,0,0];
            if(value.startsWith('rgb('))return [...value.match(/[\d.]+/g).map(Number),255];
            return [...value.slice(1).match(/../g).map(value=>parseInt(value,16)),255];
        };
        const raster=source=>{
            const pixels=new Uint8ClampedArray(20*20*4),surface=source==='blob:surface';
            for(let y=0;y<20;y++)for(let x=0;x<20;x++){
                const edge=x<f.edgeWidth||x>=20-f.edgeWidth||y<f.edgeWidth||y>=20-f.edgeWidth;
                const value=surface?f.centerColor:edge?f.pixelColor:f.centerColor;
                const rgb=Array.isArray(value)?value:[value,value,value],i=(y*20+x)*4;
                pixels.set([...rgb,surface?255:edge?255:f.centerAlpha],i);
                if(f.missingEdge&&x<f.edgeWidth)pixels.set([200,200,200,255],i);
            }
            return pixels;
        };
        const document={querySelectorAll(selector){return selector.includes(':popover-open')?[panel]:[trigger];},
            createElementNS(ns,tag){assert.equal(ns,'http://www.w3.org/2000/svg');return {tag,attrs:{},children:[],setAttribute(name,value){this.attrs[name]=value;},appendChild(child){this.children.push(child);}};},
            createElement(){
                const canvas={};
                canvas.getContext=()=>({fillStyle:'#000000',drawImage(image){this.source=image.src;},fillRect(){},
                    getImageData(){return {data:canvas.width===1?new Uint8ClampedArray(color(this.fillStyle)):raster(this.source)};}});
                return canvas;
            }};
        const context={URL:{createObjectURL(blob){f.created.push(blob);return 'blob:foreground';},revokeObjectURL(url){f.revoked.push(url);}},
            Blob:class {constructor(parts,options){this.parts=parts;assert.equal(options.type,'image/svg+xml');}},
            XMLSerializer:class {serializeToString(svg){
                assert.deepEqual(svg.attrs,{width:'100',height:'36',viewBox:'0 0 100 36'});
                assert.equal(svg.children.length,1);
                assert.deepEqual(svg.children[0].attrs,{d:f.foreground.path.getAttribute('d'),fill:f.foreground.path.style.fill,'fill-rule':'evenodd',stroke:'none'});
                return '<svg>actual-path-export</svg>';
            }},arguments:[resolve],document,getComputedStyle:e=>e.style,Image:class {
            naturalWidth=20;naturalHeight=20;
            async decode(){if(f.decodeFails)throw new Error('decode failed');if(f.detached){if(f.foreground)f.foreground.isConnected=false;else panel.isConnected=false;}}
        }};
        context.matchMedia=()=>({matches:true});
        runInNewContext(source,context);
    });
    return f;
}
let f=fixture();assert.equal((await f.run()).valid,true,'borderless trigger and native palette-derived menu');
f=fixture();f.panel.style.borderTopColor='rgb(90, 90, 90)';assert.equal((await f.run()).valid,false,'frame override must fail');
f=fixture();f.panel.visible=false;assert.equal((await f.run()).valid,false,'hidden menu must fail');
f=fixture();f.trigger.box.height=20;assert.equal((await f.run()).valid,false,'small click target must fail');
f=fixture();f.trigger.style.borderLeftWidth='1px';assert.equal((await f.run()).valid,false,'trigger border must fail');
f=fixture();f.trigger.disabled=true;assert.equal((await f.run()).valid,true,'disabled trigger retains borderless surface');
f=fixture();f.trigger.disabled=true;f.trigger.style.opacity='.6';assert.equal((await f.run()).valid,false,'disabled trigger must not fade its whole surface');
for(const property of ['backgroundImage','borderImageSource']){
    f=fixture();f.panel.style[property]='url("blob:paint")';assert.equal((await f.run()).valid,true,property);
    f=fixture();f.panel.style[property]='url("blob:paint")';f.pixelColor=60;assert.equal((await f.run()).valid,false,'wrong-palette image border');
    f=fixture();f.panel.style[property]='url("blob:paint")';f.missingEdge=true;assert.equal((await f.run()).valid,false,'three sides are insufficient');
    f=fixture();f.panel.style[property]='url("blob:paint")';f.detached=true;assert.equal((await f.run()).valid,false,'detached image paint');
    f=fixture();f.panel.style[property]='url("blob:paint")';f.centerAlpha=0;assert.equal((await f.run()).valid,true,'transparent Select surface retains its palette border');
    f=fixture();f.panel.style[property]='url("blob:paint")';f.decodeFails=true;assert.equal((await f.run()).valid,false,'broken image decode');
}
f=fixture().withForeground();assert.equal((await f.run()).valid,true,'foreground path with transparent center');assert.deepEqual(f.revoked,['blob:foreground']);
f=fixture().withForeground();f.foreground.path.style.fill='rgb(60, 60, 60)';assert.equal((await f.run()).valid,false,'wrong foreground fill');assert.equal(f.created.length,0);
f=fixture().withForeground();f.foreground.path.style.fillRule='nonzero';assert.equal((await f.run()).valid,false,'wrong foreground fill rule');
f=fixture().withForeground();f.centerAlpha=255;assert.equal((await f.run()).valid,false,'solid foreground hides menu');assert.deepEqual(f.revoked,['blob:foreground']);
for(const [axis,value] of [['x',10],['y',10],['width',90],['height',40]]){
    f=fixture().withForeground();f.foreground.box[axis]=value;assert.equal((await f.run()).valid,false,`misplaced foreground ${axis}`);assert.equal(f.created.length,0);
}
f=fixture().withForeground();f.detached=true;assert.equal((await f.run()).valid,false,'detached foreground');assert.deepEqual(f.revoked,['blob:foreground']);
f=fixture().withForeground();f.decodeFails=true;assert.equal((await f.run()).valid,false,'foreground decode failure');assert.deepEqual(f.revoked,['blob:foreground']);
f=fixture().withForeground();f.pixelColor=60;assert.equal((await f.run()).valid,false,'valid fill metadata without actual matching pixels');assert.deepEqual(f.revoked,['blob:foreground']);
f=fixture().withForeground();f.foreground.visible=false;assert.equal((await f.run()).valid,false,'hidden foreground');assert.equal(f.created.length,0);
f=fixture();f.trigger.querySelector=()=>({});assert.equal((await f.run()).valid,false,'trigger must not retain foreground border');
f=fixture();f.trigger.style.borderImageSource='url("blob:trigger")';assert.equal((await f.run()).valid,false,'stale decorative trigger image must fail despite zero computed border');
f=fixture();f.trigger.style.borderImageSource='url("blob:trigger")';f.pixelColor=200;assert.equal((await f.run()).valid,true,'borderless rounded trigger fill');
f=fixture();f.trigger.style.backgroundImage='url("blob:trigger")';f.pixelColor=200;assert.equal((await f.run()).valid,true,'borderless exact background image');
f=fixture();f.panel.style.borderLeftWidth='0px';assert.equal((await f.run()).valid,false,'expanded panel cannot lose a border side');
f=fixture().withForeground();f.foreground.style.position='fixed';assert.equal((await f.run()).valid,false,'fixed foreground can detach during compositor scrolling');
f=fixture().withForeground();f.panel.anchor=null;assert.equal((await f.run()).valid,false,'foreground must use explicit local anchoring');
f=fixture().withForeground();f.foreground.style.position='relative';assert.equal((await f.run()).valid,false,'flow-relative foreground is not a border-box overlay');
f=fixture();f.trigger.style.backgroundColor='transparent';assert.equal((await f.run()).valid,false,'collapsed trigger must retain its original fill');
f=fixture();f.trigger.style.borderImageSource='url("blob:trigger")';f.pixelColor=200;f.centerAlpha=0;assert.equal((await f.run()).valid,false,'generated trigger paint cannot lose its fill');
f=fixture();f.trigger.hover=true;f.trigger.style.borderImageSource='url("blob:trigger")';f.pixelColor=200;assert.equal((await f.run()).valid,true,'hovered trigger retains its original fill');
f=fixture();f.trigger.style.backgroundColor='#b4b4b4';assert.equal((await f.run()).valid,false,'native resting fill must match its original palette');
f=fixture();f.trigger.style.borderImageSource='url("blob:trigger")';f.pixelColor=200;f.centerColor=210;assert.equal((await f.run()).valid,false,'generated resting fill must match the actual palette');
f=fixture();f.trigger.hover=true;f.trigger.style.borderImageSource='url("blob:trigger")';f.pixelColor=200;f.centerColor=180;f.trigger.style.getPropertyValue=name=>name==='--zxs-row-hover-bg'?'#b4b4b4':'#c8c8c8';assert.equal((await f.run()).valid,false,'hover must not replace the collapsed trigger fill');
f=fixture();f.panel.style.borderTopWidth='1px';assert.equal((await f.run()).valid,false,'old 1px native border must fail');
f=fixture().withForeground();f.edgeWidth=1;assert.equal((await f.run()).valid,false,'actual 1px foreground cannot masquerade as a 2px border');
f=fixture();f.panel.style.backgroundColor='#b4b4b4';assert.equal((await f.run()).valid,false,'panel background must not darken');
f=fixture().withForeground();f.panel.style.backgroundColor='#b4b4b4';assert.equal((await f.run()).valid,false,'foreground border cannot hide a changed panel fill');
f=fixture().withForeground();f.panel.style.backgroundColor='transparent';f.panel.style.borderImageSource='url("blob:surface")';assert.equal((await f.run()).valid,true,'real foreground and separate unchanged surface paint');
f=fixture().withForeground();f.panel.style.backgroundColor='transparent';f.panel.style.borderImageSource='url("blob:surface")';f.centerColor=180;assert.equal((await f.run()).valid,false,'generated panel fill cannot darken');
f=fixture().withForeground();
for(const node of [f.panel,f.trigger]){node.style.palette='#252c31';node.style.backgroundColor='#252c31';}
for(const side of ['Top','Right','Bottom','Left'])f.panel.style[`border${side}Color`]='rgb(31, 37, 42)';
f.foreground.path.style.fill='rgb(31, 37, 42)';f.pixelColor=[31,37,42];f.centerColor=[37,44,49];
assert.equal((await f.run()).valid,true,'dark palette preserves its hue and original background');
f.foreground.path.style.fill='rgb(170, 170, 170)';assert.equal((await f.run()).valid,false,'dark mode cannot inherit a light-mode border');
console.log('PASS 53 Select border observer model cases; no browser rendering executed');
