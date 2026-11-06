// Read the actual Select border output, including G4 image and foreground paths.
// No product style, geometry or interaction state is changed by this observer.
const done=arguments[arguments.length-1];
(async () => {
    const rgba=value=>{
        if(!value) return [];
        const canvas=document.createElement('canvas');canvas.width=canvas.height=1;
        const context=canvas.getContext('2d');context.fillStyle=value;context.fillRect(0,0,1,1);
        return [...context.getImageData(0,0,1,1).data];
    };
    const sameColor=(actual,expected)=>actual.length===4&&expected.length===4&&actual.every((value,index)=>Math.abs(value-expected[index])<=1);
    const imagePixels=async source=>{
        const image=new Image();image.src=source;await image.decode();
        const canvas=document.createElement('canvas');canvas.width=image.naturalWidth;canvas.height=image.naturalHeight;
        const context=canvas.getContext('2d');context.drawImage(image,0,0);
        return {pixels:context.getImageData(0,0,canvas.width,canvas.height).data,w:canvas.width,h:canvas.height};
    };
    const centerOf=({pixels,w,h})=>[...pixels.slice((Math.floor(h/2)*w+Math.floor(w/2))*4,(Math.floor(h/2)*w+Math.floor(w/2))*4+4)];
    const surfaceMatches=async (node,css,expected)=>{
        const property=css.borderImageSource!=='none'?'borderImageSource':'backgroundImage',source=css[property];
        const match=source.match(/^url\(["']?(blob:[^"')]+)["']?\)$/);
        if(!match)return sameColor(rgba(css.backgroundColor),expected);
        const center=centerOf(await imagePixels(match[1]));
        return node.isConnected&&getComputedStyle(node)[property]===source&&sameColor(center,expected);
    };
    const rendered=e=>e.checkVisibility({opacityProperty:true,visibilityProperty:true});
    const triggers=[...document.querySelectorAll('.zxs-select-trigger')].filter(rendered);
    const panels=[...document.querySelectorAll('.zxs-select-panel:popover-open')];
    const inspect=async (node, expanded)=>{
        const css=getComputedStyle(node), box=node.getBoundingClientRect();
        const keyboardFocus=!expanded&&node.matches(':focus-visible');
        const sides=['Top','Right','Bottom','Left'];
        const widths=sides.map(side=>parseFloat(css[`border${side}Width`]));
        const expectedFill=rgba(css.getPropertyValue('--zxs-bg-panel').trim());
        // The fill stays unchanged. Only the 2px ring mixes 15% black into the
        // current panel palette, preserving its hue in both light and dark mode.
        const expectedBorder=expectedFill.map((value,index)=>index===3?value:Math.round(value*.85));
        let paint;
        const foreground=node.querySelector(':scope > svg[data-zxs-contour-decoration="foreground"]');
        let source, property, ownedUrl, livePath, pathData;
        if(!expanded && foreground) return {valid:false,reason:'collapsed trigger retains a foreground border'};
        if(foreground) {
            if(node.getAttribute('data-zxs-contour-foreground-anchor')!=='local'||getComputedStyle(foreground).position!=='absolute') {
                return {valid:false,reason:'menu border must be locally anchored, not a fixed scroll overlay'};
            }
            const path=foreground.querySelector('path'), style=getComputedStyle(path);
            livePath=path;pathData=path.getAttribute('d');
            const svgBox=foreground.getBoundingClientRect();
            const actualBorder=rgba(style.fill);
            const checks={visible:rendered(foreground),palette:sameColor(actualBorder,expectedBorder),
                fillRule:style.fillRule==='evenodd',stroke:style.stroke==='none',
                width:Math.abs(svgBox.width-box.width)<=1,height:Math.abs(svgBox.height-box.height)<=1,
                x:Math.abs(svgBox.x-box.x)<=1,y:Math.abs(svgBox.y-box.y)<=1};
            if(!Object.values(checks).every(Boolean)) {
                const bounds=rect=>({x:rect.x,y:rect.y,width:rect.width,height:rect.height});
                const svgStyle=getComputedStyle(foreground);
                return {valid:false,reason:'missing, misplaced or wrong-palette G4 foreground',checks,
                    fill:style.fill,actualBorder,expectedBorder,fillRule:style.fillRule,stroke:style.stroke,
                    panelBounds:bounds(box),foregroundBounds:bounds(svgBox),
                    foregroundStyle:{display:svgStyle.display,visibility:svgStyle.visibility,opacity:svgStyle.opacity}};
            }
            // Rasterize the real foreground path with its real even-odd fill,
            // rather than accepting its color attribute or state marker alone.
            const ns='http://www.w3.org/2000/svg';
            const exported=document.createElementNS(ns,'svg'), exportedPath=document.createElementNS(ns,'path');
            exported.setAttribute('width',String(svgBox.width));
            exported.setAttribute('height',String(svgBox.height));
            exported.setAttribute('viewBox',foreground.getAttribute('viewBox'));
            exportedPath.setAttribute('d',pathData);
            exportedPath.setAttribute('fill',style.fill);
            exportedPath.setAttribute('fill-rule',style.fillRule);
            exportedPath.setAttribute('stroke',style.stroke);
            exported.appendChild(exportedPath);
            ownedUrl=URL.createObjectURL(new Blob([new XMLSerializer().serializeToString(exported)],{type:'image/svg+xml'}));
            source=`url("${ownedUrl}")`;property='foreground';
        } else {
            property=css.borderImageSource!=='none'?'borderImageSource':'backgroundImage';
            source=css[property];
        }
        const match=source.match(/^url\(["']?(blob:[^"')]+)["']?\)$/);
        if(match) {
            try {
                const raster=await imagePixels(match[1]),{pixels,w,h}=raster;
                // G4 includes the keyboard focus ring outside the border box.
                // Its pixels are validated separately by select_focus.js, not
                // mistaken here for a decorative border inside the control.
                let inset=0;
                if(keyboardFocus){
                    inset=parseFloat(css.borderImageOutset);
                    if(document.activeElement!==node||property!=='borderImageSource'||node.dataset.zxsContourState!=='paint'||
                        !Number.isInteger(inset)||inset<5||Math.abs(w-box.width-2*inset)>1||Math.abs(h-box.height-2*inset)>1){
                        return {valid:false,reason:'keyboard focus raster does not match the actual border box',keyboardFocus,inset,w,h};
                    }
                }
                const left=inset,top=inset,right=w-inset,bottom=h-inset,innerWidth=right-left,innerHeight=bottom-top;
                const isEdge=(x,y)=>{const i=(y*w+x)*4,actual=[...pixels.slice(i,i+3),255];return pixels[i+3]>=128&&(expanded?sameColor(actual,expectedBorder):!sameColor(actual,expectedFill));};
                // Search each edge's central strip, clear of curved corners.
                // An icon, text, or focus ring cannot satisfy all four sides.
                const horizontal=(start,end)=>{
                    let count=0;
                    for(let y=start;y<end;y++)for(let x=Math.floor(left+innerWidth*.4);x<Math.ceil(left+innerWidth*.6);x++)if(isEdge(x,y))count++;
                    return count;
                };
                const vertical=(start,end)=>{
                    let count=0;
                    for(let x=start;x<end;x++)for(let y=Math.floor(top+innerHeight*.4);y<Math.ceil(top+innerHeight*.6);y++)if(isEdge(x,y))count++;
                    return count;
                };
                const edgePixels=[horizontal(top,Math.ceil(top+innerHeight*.25)),vertical(Math.floor(left+innerWidth*.75),right),horizontal(Math.floor(top+innerHeight*.75),bottom),vertical(left,Math.ceil(left+innerWidth*.25))];
                const center=centerOf(raster),centerAlpha=center[3];
                const stable=node.isConnected&&(expanded||node.matches(':focus-visible')===keyboardFocus)&&(!keyboardFocus||document.activeElement===node)&&(foreground?foreground.isConnected&&livePath.getAttribute('d')===pathData&&sameColor(rgba(getComputedStyle(livePath).fill),expectedBorder)&&getComputedStyle(livePath).fillRule==='evenodd':getComputedStyle(node)[property]===source);
                const surface=foreground?await surfaceMatches(node,css,expectedFill):sameColor(center,expectedFill)||(expanded&&centerAlpha===0&&sameColor(rgba(css.backgroundColor),expectedFill));
                const spans=[Math.ceil(left+innerWidth*.6)-Math.floor(left+innerWidth*.4),Math.ceil(top+innerHeight*.6)-Math.floor(top+innerHeight*.4)];
                const thickness=edgePixels.map((count,index)=>count/spans[index%2]);
                paint={valid:stable&&surface&&(expanded?thickness.every(n=>n>=2):edgePixels.every(n=>n===0))&&(!foreground||centerAlpha===0),property,stable,edgePixels,thickness,centerAlpha,surface,inset};
            } finally {
                if(ownedUrl)URL.revokeObjectURL(ownedUrl);
            }
        } else {
            paint={valid:(expanded?sides.every(side=>sameColor(rgba(css[`border${side}Color`]),expectedBorder)&&css[`border${side}Style`]==='solid'):sides.every(side=>css[`border${side}Style`]==='none'))&&sameColor(rgba(css.backgroundColor),expectedFill),property:expanded?'native-border':'borderless'};
        }
        return {valid:rendered(node)&&Number(css.opacity)===1&&box.width>0&&box.height>=36&&widths.every(w=>w===(expanded?2:0))&&paint.valid,
            role:node.getAttribute('role'),expanded,keyboardFocus,opacity:css.opacity,disabled:node.disabled===true,state:node.dataset.zxsContourState,
            width:box.width,height:box.height,widths,paint};
    };
    const controls=await Promise.all([...triggers.map(node=>inspect(node,false)),...panels.map(node=>inspect(node,true))]);
    return {valid:triggers.length>0&&panels.length===1&&controls.every(c=>c.valid),controls};
})().then(done,error=>done({valid:false,error:String(error)}));
