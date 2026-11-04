// Observer-model negative controls only. Native input, layout, and pixels are
// verified separately by file_row_visual in the real Chrome/WASM workflow.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
const source = readFileSync(new URL('../runner/file_row_pointer.js', import.meta.url), 'utf8');
function observe({rect = {left:17,right:449,top:200,bottom:236,width:432,height:36},
    point = null, hit = 'child', hovered = true, opacity = '1', hoverEnabled = true,
    scrollLeft = 0, folder = true, maxTouchPoints = 0} = {}) {
    const child = Object.freeze({tagName:'BUTTON',getAttribute:()=> 'zxs-file-name',textContent:'browser-fixtures'});
    const stranger = Object.freeze({tagName:'DIV',getAttribute:()=> 'overlay',textContent:'Overlay'});
    const scroll = Object.freeze({clientLeft:0,clientWidth:273,scrollLeft,getBoundingClientRect:()=>({left:17})});
    const surface = Object.freeze({dataset:{zxsContourState:'paint'}});
    const row = Object.freeze({closest:()=>scroll,getBoundingClientRect:()=>rect,
        contains:element=>element===child,matches:()=>hovered,
        querySelector:selector=>selector==='.zxs-file-row-surface'?surface:folder?{}:null});
    const document = Object.freeze({documentElement:{dataset:{zxsTheme:'light'}},
        querySelectorAll:()=>[row],elementFromPoint:()=>hit==='child'?child:hit==='overlay'?stranger:null});
    const getComputedStyle = element => element===surface?
        {opacity,pointerEvents:'none',backgroundColor:'rgba(0, 0, 0, 0)',borderImageSource:'url(blob:row)'}:
        {getPropertyValue:()=>`${scrollLeft}px`};
    const matchMedia = query => ({matches: query==='(pointer: coarse)'?maxTouchPoints>0:
        query==='(pointer: none)'?!hoverEnabled&&maxTouchPoints===0:hoverEnabled});
    const observer = new Function('document','navigator','innerWidth','innerHeight','scrollX','scrollY','matchMedia','getComputedStyle',
        `return function(){${source}}`)(document,{maxTouchPoints},320,568,0,0,matchMedia,getComputedStyle);
    return observer(0,point);
}
const initial = observe();
assert.equal(initial.valid,true);
assert.deepEqual(initial.point,{x:153,y:218});
assert.equal(initial.hit.className,'zxs-file-name');
assert.equal(initial.kind,'folder');
assert.equal(initial.contour,'paint');
assert.deepEqual(initial.device,{hover:true,anyHover:true,fine:true,anyFine:true,coarse:false,none:false,maxTouchPoints:0});
assert.equal(observe({hoverEnabled:false}).device.none,true,'headless without a pointer is distinct from touch');
assert.equal(observe({hoverEnabled:false,maxTouchPoints:1}).device.coarse,true);
assert.equal(observe({hoverEnabled:false,maxTouchPoints:1}).valid,false,'touch cannot satisfy desktop hover acceptance');
assert.equal(observe({folder:false}).kind,'file');
const right = observe({scrollLeft:159,rect:{left:-142,right:290,top:200,bottom:236,width:432,height:36}});
assert.equal(right.valid,true);
assert.deepEqual(right.point,{x:153,y:218});
assert.equal(right.scroll.cssLeft,'159px');
for (const input of [{hit:'overlay'},{hit:null},{hovered:false},{opacity:'0'},{hoverEnabled:false},
    {rect:{left:17,right:449,top:550,bottom:586,width:432,height:36}},
    {point:{x:153,y:150}},{point:{x:310,y:218}}]) {
    assert.equal(observe(input).valid,false,JSON.stringify(input));
}
const shifted = {left:17,right:449,top:270,bottom:306,width:432,height:36};
assert.equal(observe({rect:shifted,point:initial.point}).valid,false,'stale pointer must not pass');
assert.deepEqual(observe({rect:shifted}).point,{x:153,y:288});
assert.equal(observe({rect:shifted}).valid,true,'fresh live center follows layout');
console.log('PASS file-row pointer observer model and invalid target/media/style controls');
