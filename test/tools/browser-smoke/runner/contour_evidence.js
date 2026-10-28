// Same probe for both production builds. No application state or production hook.
(() => {
    const raf = window.requestAnimationFrame;
    const property = CSSStyleDeclaration.prototype.getPropertyValue;
    const style = window.getComputedStyle;
    let current = null, collecting = false, callbacks = [];
    window.requestAnimationFrame = function(callback) {
        return Reflect.apply(raf, this, [function(timestamp) {
            const previous = current;
            const sample = {timestamp_ms:timestamp, started:performance.now(), extent_reads:0};
            current = sample;
            try { return Reflect.apply(callback, this, [timestamp]); }
            finally {
                current = previous;
                if (collecting && sample.extent_reads > 0) callbacks.push({
                    timestamp_ms:timestamp, elapsed_ms:performance.now()-sample.started,
                    extent_reads:sample.extent_reads,
                });
            }
        }]);
    };
    CSSStyleDeclaration.prototype.getPropertyValue = function(name) {
        if (current && name === '--zxs-contour-extent') current.extent_reads++;
        return Reflect.apply(property, this, [name]);
    };
    const tick = () => new Promise(resolve => Reflect.apply(raf, window, [resolve]));
    const frames = async count => { while (count--) await tick(); };
    const check = (condition, message) => { if (!condition) throw Error(message); };
    const css = node => Reflect.apply(style, window, [node]);
    const get = (node, name) => Reflect.apply(property, css(node), [name]);
    const rect = node => {
        const r = node.getBoundingClientRect();
        return {x:r.x,y:r.y,width:r.width,height:r.height};
    };
    const digest = async bytes => [...new Uint8Array(await crypto.subtle.digest('SHA-256',bytes))]
        .map(byte => byte.toString(16).padStart(2,'0')).join('');
    let pane, scroller, shapes = [], rules = [], sheet, scrollEvents = 0, measured = null;
    const probe = window.__zxsContourEvidence = {};
    const painted = () => shapes.every((node,index)=>node.dataset.zxsContourState===(index>=34?'clip':'paint'));

    probe.setup = async () => {
        const loaded = await Promise.all([document.fonts.load('16px ZxsUI'),document.fonts.load('bold 16px ZxsUI')]);
        check(loaded.every(faces=>faces.length>0),'both original font faces must exist');
        await document.fonts.ready;
        check(document.fonts.check('16px ZxsUI') && document.fonts.check('bold 16px ZxsUI'),
            'original regular and bold fonts must be loaded');
        sheet = [...document.styleSheets].find(item => {
            try { return item.href?.startsWith(location.origin) && item.cssRules; }
            catch { return false; }
        });
        check(sheet, 'missing same-origin stylesheet');
        for (const text of [
            '#g4-evidence-pane[data-phase="1"] .g4-evidence-shape { color:rgb(10,70,120); }',
            '#g4-evidence-shape-33:hover { --zxs-contour-extent:24px!important; }',
        ]) { rules.push(sheet.cssRules.length); sheet.insertRule(text,sheet.cssRules.length); }
        pane = document.createElement('div');
        pane.id = 'g4-evidence-pane';
        pane.style.cssText = 'position:fixed;inset:0 auto auto 0;width:1200px;height:760px;z-index:2147483646;background:white;color:rgb(20,30,40);font:16px ZxsUI;overflow:hidden;';
        scroller = document.createElement('div');
        scroller.style.cssText = 'position:absolute;left:12px;top:644px;width:1150px;height:100px;overflow:scroll;scrollbar-gutter:stable both-edges;transform:translateZ(0);';
        scroller.addEventListener('scroll',()=>{ scrollEvents++; });
        const content = document.createElement('div');
        content.style.cssText = 'position:relative;width:1500px;height:260px;';
        scroller.append(content); pane.append(scroller);
        for (let index=0; index<40; index++) {
            const node = document.createElement('div');
            node.id = `g4-evidence-shape-${index}`; node.className = 'g4-evidence-shape';
            const foreground = index>=34;
            const x = foreground ? (index-34)*180 : (index%8)*146+12;
            const y = foreground ? 40 : Math.floor(index/8)*126+12;
            node.style.cssText = `position:absolute;left:${x}px;top:${y}px;width:132px;height:108px;box-sizing:border-box;background:rgb(225,235,245);border:2px solid rgb(30,50,70);padding:8px;`;
            node.style.setProperty('--zxs-contour-extent','12px');
            if (index===0) node.style.border = '0';
            if (index===1) node.style.backgroundImage = 'linear-gradient(rgb(220,60,90),rgb(40,80,160))';
            if (index===2) node.style.boxShadow = '3px 4px 5px rgb(80,90,100)';
            if (foreground) {
                node.style.overflow = 'hidden';
                node.style.setProperty('--zxs-contour-foreground','1');
            }
            node.textContent = `G4 ${index}`;
            (foreground ? content : pane).append(node); shapes.push(node);
        }
        const decoys = document.createElement('div');
        decoys.style.cssText = 'position:absolute;left:1190px;top:0;width:1px;height:1px;overflow:hidden;';
        for (let index=0; index<1000; index++) decoys.append(document.createElement('span'));
        pane.append(decoys); document.body.append(pane); await frames(12);
        check(painted(),'34 painted and six clipped fixtures required');
        check(shapes.slice(34).every(node => node.querySelector('svg[data-zxs-contour-decoration="foreground"]')),
            'six actual foreground SVGs required');
        return {shapes:40,foregrounds:6,decoys:1000,viewport:{width:innerWidth,height:innerHeight,dpr:devicePixelRatio},
            font_status:document.fonts.status,classic_scrollbar_width:scroller.offsetWidth-scroller.clientWidth,
            user_agent:navigator.userAgent,fixture_kind:'synthetic DOM, production contour controller'};
    };

    async function raster(url, includePng) {
        const image = new Image(); image.src = url; await image.decode();
        check(image.naturalWidth>0 && image.naturalHeight>0,'decoded nonempty SVG/PNG required');
        const canvas = document.createElement('canvas');
        canvas.width = image.naturalWidth; canvas.height = image.naturalHeight;
        const context = canvas.getContext('2d'); context.drawImage(image,0,0);
        const result = {width:canvas.width,height:canvas.height,
            rgba_sha256:await digest(context.getImageData(0,0,canvas.width,canvas.height).data)};
        if (includePng) result.png_base64 = canvas.toDataURL('image/png').split(',')[1];
        return result;
    }
    probe.pngSignature = async encoded => {
        const bytes = Uint8Array.from(atob(encoded),char => char.charCodeAt(0));
        const url = URL.createObjectURL(new Blob([bytes],{type:'image/png'}));
        try { return await raster(url,false); } finally { URL.revokeObjectURL(url); }
    };
    probe.snapshot = async () => {
        await frames(6);
        check(painted(),'snapshot requires 34 painted and six clipped fixtures');
        const geometry = shapes.map((node,index) => {
            const foreground = node.querySelector('svg[data-zxs-contour-decoration="foreground"]');
            return {index,box:rect(node),state:node.dataset.zxsContourState,
                extent:get(node,'--zxs-contour-extent'),slice:get(node,'border-image-slice'),
                border_width:get(node,'border-image-width'),outset:get(node,'border-image-outset'),
                foreground:foreground ? {box:rect(foreground),viewbox:foreground.getAttribute('viewBox')} : null};
        });
        const images = [];
        for (const index of [0,1,2,3,33,34,35]) {
            const node = shapes[index];
            const sources = index>=34 ? ['border-image-source','mask-image'] : ['border-image-source'];
            for (const name of sources) {
                const source = get(node,name);
                const match = source.match(/^url\(["']?(blob:[^"')]+)["']?\)$/);
                check(match,`expected actual SVG blob for ${index}/${name}`);
                images.push({name:`shape-${index}-${name}`,...await raster(match[1],true)});
            }
        }
        check(images.length===9,'seven border-image and two mask rasters required');
        return {geometry,images,scroll:{left:scroller.scrollLeft,top:scroller.scrollTop},
            font_status:document.fonts.status,hover:shapes[33].matches(':hover')};
    };
    probe.checkHover = async enter => {
        await frames(6);
        check(shapes[33].matches(':hover')===enter,'WebDriver pointer must change real :hover');
        check(parseFloat(get(shapes[33],'--zxs-contour-extent'))===(enter?24:12),'real hover extent');
    };
    probe.hideShow = async () => {
        shapes[3].style.display='none'; await frames(5);
        shapes[3].style.display=''; await frames(6);
        check(shapes[3].dataset.zxsContourState==='paint','hide/show must repaint');
    };
    probe.scrollSnapshot = async () => {
        scroller.scrollTo(32,48); await frames(8);
        check(scroller.scrollLeft===32 && scroller.scrollTop===48,'real scroller must move');
    };

    const stimulus = (workload,index) => {
        if (workload==='author-invalidation') pane.dataset.phase=String(index%2);
        else if (workload==='scroll') scroller.scrollTo((index%2)*32,(index%2)*48);
    };
    probe.prepare = async workload => {
        collecting=false; callbacks=[];
        shapes[0].style.transition='none'; shapes[0].style.setProperty('--zxs-contour-extent','12px');
        scroller.scrollTo(0,0); pane.dataset.phase='0'; await frames(8);
        if (workload==='extent-animation') {
            shapes[0].style.transition='--zxs-contour-extent 30s linear'; await frames(3);
            shapes[0].style.setProperty('--zxs-contour-extent','48px');
        }
        for (let index=0; index<24; index++) { stimulus(workload,index); await tick(); }
        return true;
    };
    probe.measure = async workload => {
        callbacks=[]; collecting=true;
        const scrollEventsBefore=scrollEvents;
        const timestamps=[];
        try {
            for (let index=0; index<120; index++) {
                stimulus(workload,index); timestamps.push(await tick());
            }
        } finally { collecting=false; }
        measured={workload,timestamps,callbacks,scroll_events:scrollEvents-scrollEventsBefore};
        return true;
    };
    // Called after the CDP ending metrics snapshot: geometry/animation queries
    // and raw-sample serialization must not inflate the timed browser counters.
    probe.result = workload => {
        check(measured?.workload===workload,'missing completed timing workload');
        const {timestamps,callbacks,scroll_events}=measured;
        // Scroll also repositions foregrounds synchronously. Tagged RAF timing
        // excludes that work; actual event evidence and CDP deltas include it.
        if (workload!=='scroll') check(callbacks.length>=30,'insufficient actual contour RAF callbacks');
        if (workload==='scroll') {
            check(scroll_events>0,'no actual scroll event delivered');
            check(scroller.scrollLeft===32 && scroller.scrollTop===48,'scroll workload did not move its container');
        }
        const intervals=timestamps.slice(1).map((time,index)=>time-timestamps[index]);
        check(intervals.every(value=>Number.isFinite(value)&&value>0),'invalid native RAF intervals');
        const animationRunning=shapes[0].getAnimations().some(animation=>animation.playState==='running');
        if (workload==='extent-animation') check(animationRunning,'extent transition ended during timing window');
        return {warmup_ticks:24,measurement_ticks:120,timestamps_ms:timestamps,intervals_ms:intervals,
            contour_callbacks:callbacks,animation_running:animationRunning,
            scroll_events,
            attribution:'RAF callbacks reading --zxs-contour-extent, including nonfixture contours; elapsed includes observer instrumentation, excludes later compositor/presentation work'};
    };
    probe.finish = async () => {
        collecting=false; shapes[0].style.transition='none'; pane.remove();
        for (const index of rules.reverse()) sheet.deleteRule(index);
        await frames(4);
        window.requestAnimationFrame=raf; CSSStyleDeclaration.prototype.getPropertyValue=property;
        return true;
    };
})();
