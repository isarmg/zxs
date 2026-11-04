// Unit tests of the assertion itself. This is not production browser evidence.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {runInNewContext} from 'node:vm';
const source = readFileSync(new URL('../runner/directory_tree_edges.js',import.meta.url),'utf8');
function fixture() {
    const parent = {box:{left:100,right:300,top:200,bottom:260},getBoundingClientRect(){return this.box;}};
    const child = {box:{left:100,right:300,top:50,bottom:110},getBoundingClientRect(){return this.box;}};
    const path = {dataset:{parentTree:'real-parent',childTree:'real-child'},d:'M200,200 V113',length:87,
        style:{display:'block',visibility:'visible',stroke:'rgb(0,0,0)',strokeWidth:'1.5px',opacity:'1',strokeOpacity:'1'},
        start:{x:200,y:200},end:{x:200,y:113},marker:'url(#zxs-directory-arrow)',
        getAttribute(key){return key==='d'?this.d:key==='marker-end'?this.marker:null;},getTotalLength(){return this.length;},
        getPointAtLength(at){const point=at===0?this.start:this.end;return {matrixTransform:()=>point};}};
    const svg = {style:{opacity:'1'},box:{width:400,height:300},getBoundingClientRect(){return this.box;},getScreenCTM(){return {};}};
    const f = {path,svg,parent,child,paths:[path],busy:'false',nodes:[
        {tree_id:'real-child',parent_tree_id:'real-parent',parent_endpoint:'in_window'},
        {tree_id:'real-parent',parent_tree_id:null,parent_endpoint:null},
    ]};
    const panel = {getAttribute:()=>f.busy,
        querySelector(selector){return selector==='svg.zxs-version-tree-links'?svg:selector==='[data-directory-tree-id="real-parent"]'?parent:selector==='[data-directory-tree-id="real-child"]'?child:null;},
        querySelectorAll:()=>f.paths};
    f.run=()=>runInNewContext('(function(){'+source+'})()',{
        window:{__zxsDirectoryHistory:{nodes:{nodes:f.nodes}}},
        document:{querySelector:()=>panel},getComputedStyle:node=>node.style,
    });
    return f;
}
let f=fixture();assert.equal(f.run(),true,'actual declared parent edge and measured endpoints');
f=fixture();f.paths=[];assert.equal(f.run(),false,'missing real SVG edge');
f=fixture();f.paths.push(f.path);assert.equal(f.run(),false,'duplicate or invented edge');
f=fixture();f.path.dataset.parentTree='composition-reference';assert.equal(f.run(),false,'composition references are not parent edges');
f=fixture();f.nodes[0].parent_endpoint='outside_window';assert.throws(()=>f.run(),/no real in-window parent edges/,'empty edge fixture cannot produce a pass');
for (const [key,value] of [['stroke','none'],['strokeWidth','0px'],['strokeWidth','NaN'],['opacity','0'],['strokeOpacity','0'],['visibility','hidden'],['display','none']]) {
    f=fixture();f.path.style[key]=value;assert.equal(f.run(),false,`invisible edge ${key}`);
}
f=fixture();f.svg.style.opacity='0';assert.equal(f.run(),false,'invisible SVG surface');
f=fixture();f.svg.box.width=0;assert.equal(f.run(),false,'unlaid-out SVG');
f=fixture();f.path.d='MNaN,0';assert.equal(f.run(),false,'invalid path');
f=fixture();f.path.length=0;assert.equal(f.run(),false,'empty path');
f=fixture();f.path.marker='';assert.equal(f.run(),false,'no parent-to-child arrow');
f=fixture();f.path.start.x+=20;assert.equal(f.run(),false,'source misses actual parent');
f=fixture();f.path.end.y+=20;assert.equal(f.run(),false,'target misses actual child');
f=fixture();f.parent.box.top=10;assert.equal(f.run(),false,'reversed parent placement');
f=fixture();f.busy='true';assert.equal(f.run(),false,'not settled, not a rendering pass');
console.log('PASS directory SVG assertion positive/negative host cases; not browser rendering evidence');
