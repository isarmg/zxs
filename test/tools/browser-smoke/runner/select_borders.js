// Read the actual Select border output, including G4 image and foreground paths.
// No product style, geometry or interaction state is changed by this observer.
const done=arguments[arguments.length-1];
(async () => {
    const black='rgb(0, 0, 0)';
    const rendered=e=>e.checkVisibility({opacityProperty:true,visibilityProperty:true});
    const triggers=[...document.querySelectorAll('.zxs-select-trigger')].filter(rendered);
    const panels=[...document.querySelectorAll('.zxs-select-panel:popover-open')];
    const inspect=async node=>{
        const css=getComputedStyle(node), box=node.getBoundingClientRect();
        const sides=['Top','Right','Bottom','Left'];
        const widths=sides.map(side=>parseFloat(css[`border${side}Width`]));
        let paint;
        const foreground=node.querySelector(':scope > svg[data-zxs-contour-decoration="foreground"]');
        let source, property, ownedUrl, livePath, pathData;
        if(foreground) {
            const path=foreground.querySelector('path'), style=getComputedStyle(path);
            livePath=path;pathData=path.getAttribute('d');
            const svgBox=foreground.getBoundingClientRect();
            if(!rendered(foreground)||style.fill!==black||style.fillRule!=='evenodd'||style.stroke!=='none'||
                Math.abs(svgBox.width-box.width)>1||Math.abs(svgBox.height-box.height)>1||
                Math.abs(svgBox.x-box.x)>1||Math.abs(svgBox.y-box.y)>1) {
                return {valid:false,reason:'missing, misplaced or nonblack G4 foreground',fill:style.fill};
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
                const image=new Image();image.src=match[1];await image.decode();
                const canvas=document.createElement('canvas');canvas.width=image.naturalWidth;canvas.height=image.naturalHeight;
                const context=canvas.getContext('2d');context.drawImage(image,0,0);
                const {width:w,height:h}=canvas;
                const pixels=context.getImageData(0,0,w,h).data;
                const isBlack=(x,y)=>{const i=(y*w+x)*4;return pixels[i]===0&&pixels[i+1]===0&&pixels[i+2]===0&&pixels[i+3]>=128;};
                // Search each edge's central strip, clear of curved corners.
                // A black icon, text, or focus ring cannot satisfy all four sides.
                const horizontal=(start,end)=>{
                    let count=0;
                    for(let y=start;y<end;y++)for(let x=Math.floor(w*.4);x<Math.ceil(w*.6);x++)if(isBlack(x,y))count++;
                    return count;
                };
                const vertical=(start,end)=>{
                    let count=0;
                    for(let x=start;x<end;x++)for(let y=Math.floor(h*.4);y<Math.ceil(h*.6);y++)if(isBlack(x,y))count++;
                    return count;
                };
                const edgePixels=[horizontal(0,Math.ceil(h*.25)),vertical(Math.floor(w*.75),w),horizontal(Math.floor(h*.75),h),vertical(0,Math.ceil(w*.25))];
                const centerAlpha=pixels[(Math.floor(h/2)*w+Math.floor(w/2))*4+3];
                const stable=node.isConnected&&(foreground?foreground.isConnected&&livePath.getAttribute('d')===pathData&&getComputedStyle(livePath).fill===black&&getComputedStyle(livePath).fillRule==='evenodd':getComputedStyle(node)[property]===source);
                paint={valid:stable&&edgePixels.every(n=>n>0)&&(foreground?centerAlpha===0:centerAlpha>=200),property,stable,edgePixels,centerAlpha};
            } finally {
                if(ownedUrl)URL.revokeObjectURL(ownedUrl);
            }
        } else {
            paint={valid:sides.every(side=>css[`border${side}Color`]===black&&css[`border${side}Style`]==='solid'),property:'native-border'};
        }
        return {valid:rendered(node)&&Number(css.opacity)===1&&box.width>0&&box.height>=36&&widths.every(w=>w===1)&&paint.valid,
            role:node.getAttribute('role'),opacity:css.opacity,disabled:node.disabled===true,state:node.dataset.zxsContourState,
            width:box.width,height:box.height,widths,paint};
    };
    const controls=await Promise.all([...triggers,...panels].map(inspect));
    return {valid:triggers.length>0&&panels.length===1&&controls.every(c=>c.valid),controls};
})().then(done,error=>done({valid:false,error:String(error)}));
