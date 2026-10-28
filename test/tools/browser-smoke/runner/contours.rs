//! Real production WASM CSS-dependency regressions. Counters are diagnostic,
//! not a performance claim. Fixtures do not rely on application class names.
use super::{Browser, Result, json};

pub(super) fn run(browser: &Browser, origin: &str) -> Result<()> {
    browser.command("POST", "/url", Some(json!({"url":origin})))?;
    browser.css("input[autocomplete='current-password']")?;
    let result = browser.command(
        "POST",
        "/execute/async",
        Some(json!({"script":PROBE,"args":[]})),
    )?;
    if result["passed"] != true {
        return Err(format!("contour invalidation fixture failed: {result}").into());
    }
    println!(
        "PASS real G4 CSS dependencies, selector activation, output retention and detached cleanup: {result}"
    );
    Ok(())
}

const PROBE: &str = r#"
const done=arguments[arguments.length-1];
(async () => {
  await document.fonts.ready;
  const frames = async count => { while(count--) await new Promise(requestAnimationFrame); };
  const originalStyle=window.getComputedStyle, originalQuery=Document.prototype.querySelectorAll;
  const originalElementQuery=Element.prototype.querySelectorAll;
  const originalProperty=CSSStyleDeclaration.prototype.getPropertyValue;
  const originalPosition=Node.prototype.compareDocumentPosition;
  const originalTakeRecords=MutationObserver.prototype.takeRecords;
  const takeRecordsDescriptor=Object.getOwnPropertyDescriptor(MutationObserver.prototype,'takeRecords');
  const originalAppend=Node.prototype.appendChild, originalInsert=Node.prototype.insertBefore, originalRemove=Node.prototype.removeChild;
  const originalConsoleError=console.error;
  const originalScreenCtm=SVGGraphicsElement.prototype.getScreenCTM, originalSetProperty=CSSStyleDeclaration.prototype.setProperty;
  let failAfterInsert=null, insertedBeforeFailure=null;
  const expectedHostFailures=[];
  const hostFailureMarker='synthetic insert-then-error probe';
  const measuredHandles=new WeakSet(), handleOwners=new WeakMap();
  const host=document.createElement('div'), active=document.createElement('div'), stable=document.createElement('div'), dormant=document.createElement('div');
  const text=document.createTextNode('initial observer text');
  host.style.cssText='position:fixed;left:0;top:0;width:1px;height:1px;overflow:hidden;pointer-events:none;';
  const shape=node=>{node.style.cssText='position:absolute;width:120px;height:100px;background:rgb(240,240,240);border:1px solid rgb(30,30,30);box-sizing:border-box;';};
  for(const node of [active,stable,dormant]) {shape(node);host.appendChild(node);}
  active.style.setProperty('--zxs-contour-extent','12px');
  active.style.setProperty('transition','--zxs-contour-extent 5s linear');
  stable.style.setProperty('--zxs-contour-extent','18px');
  const decoys=new Set();
  for(let i=0;i<1000;i++){const node=document.createElement('span');node.textContent='plain';decoys.add(node);host.appendChild(node);}
  host.appendChild(text);
  const sheet=[...document.styleSheets].find(sheet=>{try{return sheet.cssRules && sheet.href?.startsWith(location.origin);}catch{return false;}});
  if(!sheet)throw new Error('missing same-origin CSS fixture sheet');
  const rules=[];
  const rule=source=>{const index=sheet.cssRules.length;sheet.insertRule(source,index);rules.push(index);return sheet.cssRules[index];};
  let positionComparisons=0, takeRecordsCalls=0, subtreeQueries=0, subtreeEntries=0;
  // API invocation counts include failed calls, not committed mutation counts.
  let outputCalls={append:0,insert:0,remove:0}, journalTrace=[];
  const journalCounts=()=>{
    for(let index=0;index<journalTrace.length;index++)if(journalTrace[index]!=='drain')
      check(journalTrace[index-1]==='drain' && journalTrace[index+1]==='drain',
        'each observed output API invocation is bracketed by journal drains '+JSON.stringify(journalTrace));
    return {takeRecordsCalls,outputCalls:{...outputCalls},trace:[...journalTrace]};
  };
  const resetJournal=()=>{takeRecordsCalls=0;outputCalls={append:0,insert:0,remove:0};journalTrace=[];};
  const outputNode=node=>node instanceof Element && node.hasAttribute('data-zxs-contour-decoration');
  let scans=0, handleAcquisitions=0, propertyReads=0, decoyReads=0, stableReads=0, activeReads=0, totalHandleAcquisitions=0, totalPropertyReads=0;
  let orderNodes=null, orderReads=[];
  const reset=()=>{subtreeQueries=0;subtreeEntries=0;scans=0;positionComparisons=0;handleAcquisitions=0;propertyReads=0;decoyReads=0;stableReads=0;activeReads=0;};
  const counts=()=>({scans,subtreeQueries,subtreeEntries,handleAcquisitions,propertyReads,positionComparisons,decoyReads,stableReads,activeReads});
  const check=(ok,label)=>{if(!ok)throw new Error(label+' '+JSON.stringify(counts()));};
  // CSSOM expands `border` into border-image defaults (often `initial`), so
  // restoration must compare the browser's original values, not assumed ''.
  const paintProperty=name=>/^(background(?:-|$)|border(?:-|$)|mask(?:-|$))/.test(name) || ['box-shadow','backdrop-filter'].includes(name);
  const originalPaint=node=>{
    const computed=Reflect.apply(originalStyle,window,[node]);
    const properties=[...new Set([...computed,...node.style,'border-image-source'].filter(paintProperty))].sort();
    return {
      inline:Object.fromEntries(properties.map(name=>[name,[node.style.getPropertyValue(name),node.style.getPropertyPriority(name)]])),
      computed:Object.fromEntries(properties.map(name=>[name,computed.getPropertyValue(name)])),
      markers:Object.fromEntries(['data-zxs-contour-state','data-zxs-contour-reason'].map(name=>[name,node.getAttribute(name)]))
    };
  };
  const restoredPaint=(node,expected,label,connected)=>{
    const computed=connected?Reflect.apply(originalStyle,window,[node]):null;
    for(const [name,value] of Object.entries(expected.inline)) {
      const actual=[node.style.getPropertyValue(name),node.style.getPropertyPriority(name)];
      check(JSON.stringify(actual)===JSON.stringify(value),`${label}: inline ${name} expected=${JSON.stringify(value)} actual=${JSON.stringify(actual)}`);
    }
    for(const [name,value] of Object.entries(expected.markers))
      check(node.getAttribute(name)===value,`${label}: ${name} expected=${JSON.stringify(value)} actual=${JSON.stringify(node.getAttribute(name))}`);
    if(computed)for(const [name,value] of Object.entries(expected.computed))
      check(computed.getPropertyValue(name)===value,`${label}: computed ${name} expected=${JSON.stringify(value)} actual=${JSON.stringify(computed.getPropertyValue(name))}`);
  };
  // Only browser-returned live handles obtained by production reads are tagged.
  // Inline style declarations and the probe's originalStyle calls stay excluded.
  // The registry keeps one live handle per element, so read order and
  // "a frame ran" are observed on property reads, not on acquisitions.
  window.getComputedStyle=function(node,...rest){
    handleAcquisitions++;totalHandleAcquisitions++;
    const live=Reflect.apply(originalStyle,this,[node,...rest]);
    measuredHandles.add(live);handleOwners.set(live,node);return live;
  };
  CSSStyleDeclaration.prototype.getPropertyValue=function(name){
    if(measuredHandles.has(this)){
      propertyReads++;totalPropertyReads++;
      const node=handleOwners.get(this);
      if(orderNodes?.has(node) && name==='--zxs-contour-extent')orderReads.push(node);
      if(decoys.has(node))decoyReads++;if(node===stable)stableReads++;if(node===active)activeReads++;
    }
    return Reflect.apply(originalProperty,this,[name]);
  };
  Node.prototype.compareDocumentPosition=function(other){positionComparisons++;return Reflect.apply(originalPosition,this,[other]);};
  MutationObserver.prototype.takeRecords=function(){takeRecordsCalls++;journalTrace.push('drain');return Reflect.apply(originalTakeRecords,this,[]);};
  Node.prototype.appendChild=function(node){if(outputNode(node)){outputCalls.append++;journalTrace.push('append');}return Reflect.apply(originalAppend,this,[node]);};
  Node.prototype.insertBefore=function(node,before){
    if(outputNode(node)){outputCalls.insert++;journalTrace.push('insert');}
    const result=Reflect.apply(originalInsert,this,[node,before]);
    if(this===failAfterInsert && outputNode(node)){
      failAfterInsert=null;insertedBeforeFailure=node;
      throw new Error(hostFailureMarker);
    }
    return result;
  };
  console.error=function(...args){
    if(args.some(value=>String(value).includes(hostFailureMarker)))expectedHostFailures.push(args.map(String));
    else Reflect.apply(originalConsoleError,this,args);
  };
  Node.prototype.removeChild=function(node){if(outputNode(node)){outputCalls.remove++;journalTrace.push('remove');}return Reflect.apply(originalRemove,this,[node]);};
  Element.prototype.querySelectorAll=function(selector){const nodes=Reflect.apply(originalElementQuery,this,[selector]);if(selector==='*'){subtreeQueries++;subtreeEntries+=nodes.length;}return nodes;};
  Document.prototype.querySelectorAll=function(selector){if(selector==='*')scans++;return Reflect.apply(originalQuery,this,[selector]);};
  try {
    document.body.appendChild(host);
    // Read synchronously before the controller's first scheduled paint.
    const dormantOriginal=originalPaint(dormant), stableOriginal=originalPaint(stable);
    await frames(6);
    check(active.dataset.zxsContourState==='paint' && stable.dataset.zxsContourState==='paint','initial arbitrary CSS candidates');
    const stableImage=Reflect.apply(originalStyle,window,[stable]).borderImageSource;
    check(stableImage.includes('blob:'),'actual G4 SVG paint');
    active.style.setProperty('--zxs-contour-extent','48px');
    await frames(4);reset();await frames(8);
    const animation=counts();
    check(propertyReads>0 && activeReads>0,'extent animation re-reads live computed styles every frame');
    check(handleAcquisitions<decoys.size,'animation frames reuse registry handles instead of acquiring one per registered element');
    check(Reflect.apply(originalStyle,window,[stable]).borderImageSource===stableImage,'static paint output stays unchanged during another contour animation');
    active.style.setProperty('transition','none');await frames(5);
    // Both nodes start at zero. A positive delay must not exclude the parent
    // from later frames, nor its explicit-inherit child (which has no transition).
    const delayed=document.createElement('div'), inherited=document.createElement('div');
    shape(delayed);shape(inherited);delayed.style.border='0';inherited.style.border='0';
    // Borderless canonical shapes use compact SVG tiles: physical slice width
    // is extent+2px, not the fixed margin of a full-size bordered image.
    delayed.style.setProperty('--zxs-contour-extent','0px');
    delayed.style.setProperty('transition','--zxs-contour-extent 2s linear 1s');
    inherited.style.setProperty('--zxs-contour-extent','inherit');inherited.style.setProperty('transition','none');
    delayed.appendChild(inherited);host.appendChild(delayed);await frames(5);
    let duringDelay=false, started=false, ended=false;
    delayed.addEventListener('transitionrun',event=>{if(event.propertyName==='--zxs-contour-extent')duringDelay=parseFloat(Reflect.apply(originalStyle,window,[delayed]).getPropertyValue('--zxs-contour-extent'))===0;});
    delayed.addEventListener('transitionstart',event=>{if(event.propertyName==='--zxs-contour-extent')started=true;});
    delayed.addEventListener('transitionend',event=>{if(event.propertyName==='--zxs-contour-extent')ended=true;});
    delayed.style.setProperty('--zxs-contour-extent','36px');
    const deadline=performance.now()+10000;
    while(!started && performance.now()<deadline)await frames(1);
    await frames(2);
    check(duringDelay && started && !ended,'fixture must sample the real delayed transition before its end');
    check(delayed.dataset.zxsContourState==='paint' && inherited.dataset.zxsContourState==='paint','delayed zero-to-positive parent and explicit-inherit child must paint before transitionend');
    const inheritedWidth=parseFloat(Reflect.apply(originalStyle,window,[inherited]).borderImageWidth);
    await frames(6);
    check(!ended && parseFloat(Reflect.apply(originalStyle,window,[inherited]).borderImageWidth)>inheritedWidth,'explicit-inherit child G4 output must advance without its own transition event');
    delayed.style.setProperty('transition','none');await frames(4);
    const colorParent=document.createElement('div'), colorChild=document.createElement('div');
    shape(colorParent);shape(colorChild);colorParent.style.backgroundColor='rgb(200,10,20)';
    colorParent.style.setProperty('--zxs-contour-extent','12px');colorChild.style.backgroundColor='inherit';
    colorChild.style.setProperty('--zxs-contour-extent','12px');colorChild.style.setProperty('transition','--zxs-contour-extent 5s linear');
    colorParent.appendChild(colorChild);host.appendChild(colorParent);await frames(5);
    colorChild.style.setProperty('--zxs-contour-extent','30px');await frames(5);
    const source=Reflect.apply(originalStyle,window,[colorChild]).borderImageSource;
    const match=source.match(/^url\(["']?(blob:[^"')]+)["']?\)$/);
    check(!!match,'inherited background has actual emitted SVG paint');
    const paint=new Image();paint.src=match[1];await paint.decode();
    const canvas=document.createElement('canvas');canvas.width=paint.naturalWidth;canvas.height=paint.naturalHeight;
    const context=canvas.getContext('2d');context.drawImage(paint,0,0);
    const center=[...context.getImageData(Math.floor(canvas.width/2),Math.floor(canvas.height/2),1,1).data];
    check(center.join(',')==='200,10,20,255','animated child must inherit original parent background, not controller-owned transparent paint');
    colorChild.style.setProperty('transition','none');await frames(4);
    // Transition policy writes can affect a same-depth sibling's author CSS.
    // Restoring both siblings before either is read loses this dependency.
    const policyA=document.createElement('div'), policyB=document.createElement('div');
    policyA.id='p5-policy-a';policyB.id='p5-policy-b';
    for(const node of [policyA,policyB]){shape(node);node.style.setProperty('--zxs-contour-extent','12px');}
    rule('#p5-policy-a, #p5-policy-b { transition:background-color 1s; }');
    rule('#p5-policy-a[style*="transition"] + #p5-policy-b { transition:opacity 2s; }');
    host.append(policyA,policyB);await frames(5);
    check(policyA.dataset.zxsContourState==='paint' && policyB.dataset.zxsContourState==='paint',
      'sibling transition-policy fixtures paint');
    const siblingPolicy=()=>Reflect.apply(originalStyle,window,[policyB]).transitionProperty==='opacity';
    check(siblingPolicy(),'initial sibling selector sees the preceding transition override');
    policyA.setAttribute('data-p5-author-change','1');await frames(4);
    check(siblingPolicy(),'author invalidation keeps sibling-dependent opacity transition');
    policyA.remove();policyB.remove();await frames(4);
    reset();text.data='changed observer text';await frames(4);
    const textUpdate=counts();
    check(scans===0 && decoyReads>0 && stableReads>0,'text still reconciles all CSS properties without repeating document discovery');
    const direction=document.createElement('div'), directionText=document.createTextNode('abc'), directionCandidate=document.createElement('div');
    direction.id='p13-direction';direction.dir='auto';directionCandidate.id='p13-direction-candidate';shape(directionCandidate);
    direction.append(directionText,directionCandidate);host.appendChild(direction);
    rule('#p13-direction:has(:dir(rtl)) #p13-direction-candidate { --zxs-contour-extent:18px; }');
    await frames(4);check(!directionCandidate.hasAttribute('data-zxs-contour-state'),'initial LTR direction fixture');
    directionText.data='אבג';await frames(4);
    check(directionCandidate.dataset.zxsContourState==='paint','nonempty RTL text must discover a :has(:dir()) candidate');
    const grid=document.createElement('div'), label=document.createElement('span'), labelText=document.createTextNode('x'), container=document.createElement('div'), queryCandidate=document.createElement('div'), sentinel=document.createElement('div');
    grid.id='p13-query-grid';grid.style.cssText='display:grid;grid-template-columns:max-content minmax(0,1fr);font:10px monospace;';
    const widthRule=rule('#p13-query-grid { width:200px; }');
    container.style.cssText='container:p13-query / inline-size;min-width:0;';
    queryCandidate.id='p13-query-candidate';shape(queryCandidate);queryCandidate.style.position='relative';queryCandidate.style.width='100%';
    shape(sentinel);sentinel.style.position='relative';sentinel.style.width='100%';sentinel.style.setProperty('--zxs-contour-extent','8px');
    label.appendChild(labelText);container.append(sentinel,queryCandidate);grid.append(label,container);host.appendChild(grid);
    rule('@container p13-query (max-width:80px) { #p13-query-candidate { --zxs-contour-extent:18px; } }');
    await frames(5);check(!queryCandidate.hasAttribute('data-zxs-contour-state'),'initial wide container query fixture');
    // CSSOM rule changes do not produce attribute/text mutations. An existing
    // contour resizes, and its RO notification must rediscover the dormant one.
    reset();widthRule.style.width='80px';await frames(5);
    check(queryCandidate.dataset.zxsContourState==='paint' && scans===0 && decoyReads>0 && stableReads>0,'ResizeObserver rechecks dormant CSS qualification without document discovery');
    widthRule.style.width='200px';await frames(5);
    check(!queryCandidate.hasAttribute('data-zxs-contour-state'),'expanded container restores zero-extent paint');
    labelText.data='abcdefghijklmnopqrstuv';await frames(5);
    check(queryCandidate.dataset.zxsContourState==='paint','nonempty sibling text changes container-query membership');
    dormant.setAttribute('data-zxs-contour-extent','18');await frames(4);
    check(dormant.dataset.zxsContourState==='paint','attribute activates previously zero extent');
    dormant.removeAttribute('data-zxs-contour-extent');await frames(4);
    restoredPaint(dormant,dormantOriginal,'attribute removal restores original paint',true);
    // Pseudo-class state changes without any DOM mutation.
    const focusable=document.createElement('div');focusable.id='p5-focus';focusable.tabIndex=0;shape(focusable);
    rule('#p5-focus:focus { --zxs-contour-extent:14px; }');
    host.appendChild(focusable);await frames(4);
    check(!focusable.hasAttribute('data-zxs-contour-state'),'unfocused selector-only fixture stays unpainted');
    focusable.focus();await frames(4);
    check(document.activeElement===focusable && focusable.dataset.zxsContourState==='paint','focus activates a :focus-only contour');
    focusable.blur();await frames(4);
    check(!focusable.hasAttribute('data-zxs-contour-state'),'blur restores the :focus-only candidate');
    // Hide and show keep the same G4 output once the box is back.
    const hidden=document.createElement('div');shape(hidden);hidden.style.setProperty('--zxs-contour-extent','15px');
    host.appendChild(hidden);await frames(4);
    check(hidden.dataset.zxsContourState==='paint','hide/show fixture paints');
    const shownImage=Reflect.apply(originalStyle,window,[hidden]).borderImageSource;
    hidden.style.display='none';await frames(4);
    const hiddenState=hidden.getAttribute('data-zxs-contour-state');
    hidden.style.display='';await frames(4);
    check(hidden.dataset.zxsContourState==='paint' && Reflect.apply(originalStyle,window,[hidden]).borderImageSource===shownImage,
      'show after hide restores the identical G4 paint');
    // Observe the real read/write/verify/correct phases, including identities.
    let foregroundTrace=null;
    const foregroundStyles=new WeakMap(), foregroundIds=new WeakMap();
    let perturbForeground=null;
    SVGGraphicsElement.prototype.getScreenCTM=function(){
      if(foregroundTrace && foregroundIds.has(this))foregroundTrace.push({op:'read',id:foregroundIds.get(this)});
      return Reflect.apply(originalScreenCtm,this,[]);
    };
    CSSStyleDeclaration.prototype.setProperty=function(name,...rest){
      if(foregroundTrace && foregroundStyles.has(this) && (name==='left' || name==='top'))
        foregroundTrace.push({op:'write',id:foregroundStyles.get(this),axis:name});
      const result=Reflect.apply(originalSetProperty,this,[name,...rest]);
      // A separate fault-injection pass displaces one real SVG after its first
      // offset pair; native CTM reads must discover and correct the displacement.
      if(this===perturbForeground && name==='top'){
        perturbForeground=null;
        const left=parseFloat(Reflect.apply(originalProperty,this,['left']));
        Reflect.apply(originalSetProperty,this,['left',`${left+3}px`,'important']);
      }
      return result;
    };
    const outlined=[0,1].map(()=>{
      const node=document.createElement('div');shape(node);node.style.overflow='hidden';
      node.style.setProperty('--zxs-contour-foreground','1');node.style.setProperty('--zxs-contour-extent','12px');
      host.appendChild(node);return node;
    });
    await frames(5);
    const outlines=outlined.map(node=>node.querySelector(':scope > svg[data-zxs-contour-decoration="foreground"]'));
    check(outlines.every(Boolean),'foreground fixture paints SVG outlines');
    [...document.querySelectorAll('svg[data-zxs-contour-decoration="foreground"]')].forEach((svg,id)=>{
      foregroundIds.set(svg,id);foregroundStyles.set(svg.style,id);
    });
    const traceScroll=()=>{
      foregroundTrace=[];document.dispatchEvent(new Event('scroll'));
      const trace=foregroundTrace;foregroundTrace=null;return trace;
    };
    const checkForegroundPhases=(trace,label,correctedId=null)=>{
      let cursor=0;
      const phase=op=>{const entries=[];while(trace[cursor]?.op===op)entries.push(trace[cursor++]);return entries;};
      const reads=phase('read'), writes=phase('write'), verification=phase('read'), corrections=phase('write');
      const ids=reads.map(entry=>entry.id);
      check(cursor===trace.length && ids.length>=2 && new Set(ids).size===ids.length &&
        JSON.stringify(verification.map(entry=>entry.id))===JSON.stringify(ids),
        label+': complete read/write/verify/correct batches '+JSON.stringify(trace));
      const pairs=entries=>{
        check(entries.length%2===0,label+': complete offset pairs');
        const written=[];
        for(let index=0;index<entries.length;index+=2){
          const left=entries[index],top=entries[index+1];
          check(left.axis==='left' && top.axis==='top' && left.id===top.id && ids.includes(left.id),
            label+': paired offsets belong to a measured foreground');
          written.push(left.id);
        }
        check(new Set(written).size===written.length && written.every((id,index)=>
          index===0 || ids.indexOf(written[index-1])<ids.indexOf(id)),label+': each batch keeps record order');
        return written;
      };
      const written=pairs(writes),corrected=pairs(corrections);
      check(outlines.every(svg=>written.includes(foregroundIds.get(svg))),label+': both fixtures receive initial offsets');
      check(corrected.every(id=>written.includes(id)),label+': corrections only follow initial writes');
      if(correctedId!==null)check(corrected.includes(correctedId),label+': displaced foreground is corrected');
    };
    const scrollTrace=traceScroll();checkForegroundPhases(scrollTrace,'scroll');
    perturbForeground=outlines[0].style;
    const scrollCorrectionTrace=traceScroll();
    check(perturbForeground===null,'foreground displacement injection ran');
    checkForegroundPhases(scrollCorrectionTrace,'displaced scroll',foregroundIds.get(outlines[0]));
    const correctedRect=outlines[0].getBoundingClientRect(), outlinedRect=outlined[0].getBoundingClientRect();
    check(Math.abs(correctedRect.left-outlinedRect.left)<0.1 && Math.abs(correctedRect.top-outlinedRect.top)<0.1,
      'verification corrects the real SVG position, not only the call sequence');
    for(const node of outlined)node.remove();
    hidden.remove();focusable.remove();await frames(4);
    // Structural membership includes dormant HTML, independent of paint records.
    const branch=document.createElement('div'), left=document.createElement('div'), right=document.createElement('div');
    const first=document.createElement('div'), second=document.createElement('div'), third=document.createElement('div');
    for(const node of [first,second,third]){shape(node);node.style.setProperty('--zxs-contour-extent','0px');}
    left.append(first,second);right.append(third);branch.append(left,right);host.append(branch);
    await frames(6);
    check([first,second,third].every(node=>!node.hasAttribute('data-zxs-contour-state')),'dynamic subtree starts with zero qualification');
    const beforeReparentQueries=subtreeQueries,beforeReparentEntries=subtreeEntries;
    orderNodes=new Set([first,second,third]);orderReads=[];
    right.append(second);await frames(4);
    const expectedOrder=[first,second,third].sort((a,b)=>a===b?0:Reflect.apply(originalPosition,a,[b])&Node.DOCUMENT_POSITION_FOLLOWING?-1:1);
    check(orderReads.length>=expectedOrder.length && orderReads.length%expectedOrder.length===0,'reparent reads every dormant registry member');
    check(orderReads.every((node,index)=>node===expectedOrder[index%expectedOrder.length]),'same-document reparent preserves document-order CSS reads');
    const reparentReads=orderReads.length;orderNodes=null;orderReads=[];
    const reparentDiscovery={queries:subtreeQueries-beforeReparentQueries,entries:subtreeEntries-beforeReparentEntries};
    second.style.setProperty('--zxs-contour-extent','16px');await frames(4);
    check(second.dataset.zxsContourState==='paint','reparented formerly-zero HTML remains eligible');
    const adopted=document.createElement('div');shape(adopted);adopted.style.setProperty('--zxs-contour-extent','20px');
    const adoptedOriginal=originalPaint(adopted);left.append(adopted);await frames(5);
    check(adopted.dataset.zxsContourState==='paint','nested dynamic insertion paints');
    const otherDocument=document.implementation.createHTMLDocument('contour adoption fixture');
    otherDocument.body.append(otherDocument.adoptNode(adopted));await frames(5);
    check(adopted.isConnected && adopted.ownerDocument===otherDocument,'adoption fixture is connected in another document');
    restoredPaint(adopted,adoptedOriginal,'cross-document departure restores author paint',false);
    right.append(document.adoptNode(adopted));await frames(5);
    check(adopted.dataset.zxsContourState==='paint','adopted-back HTML is rediscovered and painted');

    // A fresh iframe's initial document has a distinct Window realm without
    // navigating to a resource or relaxing the application's CSP.
    const realmFrame=document.createElement('iframe');host.append(realmFrame);
    const foreignWindow=realmFrame.contentWindow, foreignDocument=realmFrame.contentDocument;
    const foreignNode=foreignDocument.createElement('div');
    check(foreignNode instanceof foreignWindow.HTMLElement && !(foreignNode instanceof Element),
      'iframe fixture really has a foreign Element prototype');
    shape(foreignNode);foreignNode.style.setProperty('--zxs-contour-extent','17px');
    const foreignOriginal=originalPaint(foreignNode);
    left.append(document.adoptNode(foreignNode));await frames(5);
    check(!(foreignNode instanceof Element) && foreignNode.ownerDocument===document,
      'adoption preserves foreign prototype in controller document');
    check(foreignNode.dataset.zxsContourState==='paint','foreign-realm HTML insertion paints');
    const nonElementErrors=window.__zxsSmokeErrors.length;
    document.dispatchEvent(new Event('pointerover',{bubbles:true}));
    text.dispatchEvent(new Event('pointerover',{bubbles:true}));await frames(6);
    check(window.__zxsSmokeErrors.length===nonElementErrors,'Document and Text event targets remain non-candidates');
    // Run each extent transition alone, without pointer events or unrelated
    // active contours keeping the controller alive. Borderless paint encodes
    // its live extent in border-image-width, not just in the author CSS value.
    for(const [label,node] of [['same-realm',document.createElement('div')],['foreign-realm',foreignNode]]) {
      const temporary=node!==foreignNode;
      if(temporary){shape(node);left.append(node);}
      node.style.border='0';node.style.setProperty('transition','none');
      node.style.setProperty('--zxs-contour-extent','8px');await frames(6);
      node.style.setProperty('transition','--zxs-contour-extent 3s linear 0.2s');await frames(4);
      let running=false, ended=false;
      const onRun=event=>{if(event.propertyName==='--zxs-contour-extent')running=true;};
      const onEnd=event=>{if(event.propertyName==='--zxs-contour-extent')ended=true;};
      node.addEventListener('transitionrun',onRun);node.addEventListener('transitionend',onEnd);
      node.style.setProperty('--zxs-contour-extent','44px');
      const deadline=performance.now()+10000;
      while(!running && performance.now()<deadline)await frames(1);
      check(running && !ended,label+' native extent transition runs');
      // Let the positive delay elapse without supplying another input event.
      while(parseFloat(Reflect.apply(originalStyle,window,[node]).getPropertyValue('--zxs-contour-extent'))<=9 && performance.now()<deadline)await frames(1);
      await frames(2);
      const first=parseFloat(Reflect.apply(originalStyle,window,[node]).borderImageWidth);
      await frames(6);
      const second=parseFloat(Reflect.apply(originalStyle,window,[node]).borderImageWidth);
      check(!ended && second>first,label+' actual G4 output advances during isolated transition');
      node.style.setProperty('transition','none');await frames(5);
      node.removeEventListener('transitionrun',onRun);node.removeEventListener('transitionend',onEnd);
      if(temporary){node.remove();await frames(4);}
    }
    // Restore the author's original geometry before the adoption/cleanup checks.
    shape(foreignNode);foreignNode.style.setProperty('--zxs-contour-extent','17px');await frames(5);
    foreignDocument.body.append(foreignDocument.adoptNode(foreignNode));await frames(5);
    restoredPaint(foreignNode,foreignOriginal,'foreign-realm departure restores author paint',false);
    orderNodes=new Set([foreignNode]);orderReads=[];
    host.dispatchEvent(new Event('pointerover',{bubbles:true}));await frames(4);
    check(orderReads.length===0,'departed foreign-realm node is no longer read');
    orderNodes=null;orderReads=[];
    right.append(document.adoptNode(foreignNode));await frames(5);
    check(foreignNode.dataset.zxsContourState==='paint','foreign-realm HTML adoption back repaints');
    foreignNode.remove();realmFrame.remove();await frames(5);
    restoredPaint(foreignNode,foreignOriginal,'foreign-realm final removal restores author paint',false);

    const transientParent=document.createElement('div'), transientChild=document.createElement('div');
    shape(transientChild);transientChild.style.setProperty('--zxs-contour-extent','18px');
    const transientOriginal=originalPaint(transientChild);
    transientParent.append(transientChild);host.append(transientParent);await frames(5);
    check(transientChild.dataset.zxsContourState==='paint','transient subtree fixture initially paints');
    transientParent.remove();transientChild.remove();await frames(5);
    restoredPaint(transientChild,transientOriginal,'transient detached descendant restores paint',false);
    orderNodes=new Set([transientParent,transientChild]);orderReads=[];
    host.dispatchEvent(new Event('pointerover',{bubbles:true}));await frames(4);
    check(orderReads.length===0,'explicit refresh never reads departed parent or extracted descendant');
    orderNodes=null;orderReads=[];
    transientParent.append(transientChild);host.append(transientParent);await frames(5);
    check(transientChild.dataset.zxsContourState==='paint','transient subtree reinsertion paints again');

    // This node is already RO-observed in fallback state before arming. Its
    // fallback -> paint transition inserts a sibling synchronously while the
    // style observer is disconnected. Fixed absolute boxes do not resize any
    // previously observed contour, so later discovery cannot rely on that RO.
    const transitionTarget=document.createElement('div');shape(transitionTarget);
    transitionTarget.id='p13-transition-target';transitionTarget.style.setProperty('--zxs-contour-extent','12px');
    rule('#p13-transition-target { transition-property:opacity; transition-duration:.25s; }');
    rule('body:has(#p13-reentrant-flag) #p13-transition-target { transition-property:background-color,opacity; }');
    host.append(transitionTarget);await frames(6);
    check(transitionTarget.dataset.zxsContourState==='paint' && Reflect.apply(originalStyle,window,[transitionTarget]).transitionProperty==='opacity',
      'structural transition fixture starts painted with only an unowned transition');
    let inserted=null, armed=false, insertions=0;
    class PaintInsertionProbe extends HTMLElement {
      static observedAttributes=['data-zxs-contour-state'];
      attributeChangedCallback(name,oldValue,newValue){
        if(armed && newValue==='paint'){
          armed=false;insertions++;
          inserted=document.createElement('div');shape(inserted);inserted.id='p13-reentrant-flag';
          inserted.style.setProperty('--zxs-contour-extent','14px');
          host.append(inserted);
        }
      }
    }
    customElements.define('zxs-paint-insertion-probe',PaintInsertionProbe);
    const reentrant=document.createElement('zxs-paint-insertion-probe');shape(reentrant);
    reentrant.style.setProperty('--zxs-contour-extent','12px');
    reentrant.style.maskImage='linear-gradient(black,black)';host.append(reentrant);await frames(8);
    check(reentrant.dataset.zxsContourState==='fallback' && reentrant.dataset.zxsContourReason==='existing-mask','reentrant fixture has established fallback ownership');
    const reentrantBox=reentrant.getBoundingClientRect();
    armed=true;reentrant.style.maskImage='none';
    // This is the only stimulus. Do not dispatch a second event to rescue an
    // insertion lost during paint; discovery itself must schedule its update.
    await frames(8);
    check(insertions===1 && inserted?.dataset.zxsContourState==='paint','synchronous custom-element insertion during paint must be discovered');
    check(Reflect.apply(originalStyle,window,[transitionTarget]).transitionProperty==='opacity',
      'author childList during paint invalidates transition policy and filters owned background-color');
    transitionTarget.style.transition='none';inserted.removeAttribute('id');await frames(4);
    const afterReentrantBox=reentrant.getBoundingClientRect();
    check(afterReentrantBox.width===reentrantBox.width && afterReentrantBox.height===reentrantBox.height,'reentrant insertion does not resize its already-observed source');

    // Finite quiet-window check, not a claim that unrelated events never occur.
    // All extent transitions above were canceled before these observations.
    const quietWindow=async label=>{
      await frames(8);
      const before=totalHandleAcquisitions, reads=totalPropertyReads;await frames(4);
      check(totalHandleAcquisitions===before && totalPropertyReads===reads,label+': first quiet window');
      await frames(4);check(totalHandleAcquisitions===before && totalPropertyReads===reads,label+': second quiet window');
      return before;
    };
    // Author removal of a known output is not a controller write. The source
    // box is already observed and settled; its callback only removes one node.
    let removeArmed=false, authorRemovals=0, removedOutput=null;
    class PaintRemovalProbe extends HTMLElement {
      static observedAttributes=['data-zxs-contour-state'];
      attributeChangedCallback(name,oldValue,newValue){
        if(removeArmed && newValue==='paint'){
          removeArmed=false;authorRemovals++;
          removedOutput=this.querySelector(':scope > [data-zxs-contour-decoration]');
          // The saved method excludes this fixture action from production-write
          // counters while preserving its real native MutationObserver record.
          Reflect.apply(originalRemove,this,[removedOutput]);
        }
      }
    }
    customElements.define('zxs-paint-removal-probe',PaintRemovalProbe);
    const removalProbe=document.createElement('zxs-paint-removal-probe');shape(removalProbe);
    removalProbe.style.backgroundImage='linear-gradient(rgb(30,50,70),rgb(90,110,130))';
    removalProbe.style.setProperty('--zxs-contour-extent','14px');host.append(removalProbe);await frames(8);
    const originalOutput=removalProbe.querySelector(':scope > [data-zxs-contour-decoration]');
    check(!!originalOutput && removalProbe.dataset.zxsContourState==='paint','author-removal fixture has settled owned output');
    const removalBox=removalProbe.getBoundingClientRect();
    removalProbe.style.maskImage='linear-gradient(black,black)';await frames(8);
    check(removalProbe.dataset.zxsContourState==='fallback' && removalProbe.dataset.zxsContourReason==='existing-mask'
      && removalProbe.querySelector(':scope > [data-zxs-contour-decoration]')===originalOutput,
      'author-removal fixture retains the same output in settled mask fallback');
    const fallbackBox=removalProbe.getBoundingClientRect();
    check(fallbackBox.width===removalBox.width && fallbackBox.height===removalBox.height,
      'mask fallback does not resize the already-observed source');
    resetJournal();removeArmed=true;removalProbe.style.maskImage='none';
    await frames(8);
    check(authorRemovals===1 && removedOutput===originalOutput && originalOutput.parentNode===removalProbe,
      'author removal during paint schedules reattachment of the same output identity');
    const afterRemovalBox=removalProbe.getBoundingClientRect();
    check(afterRemovalBox.width===removalBox.width && afterRemovalBox.height===removalBox.height,
      'author-removal callback does not resize the observed source');
    const authorOutputRepair=journalCounts();
    await quietWindow('author output repair settles');
    resetJournal();
    host.dispatchEvent(new Event('pointerover',{bubbles:true}));await frames(4);
    const stableRefreshJournal=journalCounts();
    check(stableRefreshJournal.takeRecordsCalls>=2 && Object.values(stableRefreshJournal.outputCalls).every(count=>count===0),
      'stable explicit refresh drains discovery without output writes');
    resetJournal();
    const decorated=document.createElement('div');shape(decorated);
    decorated.style.backgroundImage='linear-gradient(rgb(20,40,60),rgb(80,100,120))';
    decorated.style.setProperty('--zxs-contour-extent','16px');host.append(decorated);await frames(6);
    check(decorated.dataset.zxsContourState==='paint' && !!decorated.querySelector(':scope > [data-zxs-contour-decoration]'),'image-backed contour creates real HTML decoration');
    const decorationCreationJournal=journalCounts();
    const afterDecoration=await quietWindow('owned decoration creation settles');
    resetJournal();
    decorated.style.setProperty('--zxs-contour-extent','0px');await frames(6);
    check(!decorated.hasAttribute('data-zxs-contour-state') && !decorated.querySelector(':scope > [data-zxs-contour-decoration]'),'qualification removal releases owned HTML decoration');
    const decorationRemovalJournal=journalCounts();
    const afterDecorationRemoval=await quietWindow('owned decoration removal settles');
    // Explicit host fault model: this wrapper performs a real insertion, then
    // throws once. This does not claim native Chrome behaves this way. The
    // controller must retain the returned error and reconcile queued changes.
    resetJournal();
    const faulted=document.createElement('div');shape(faulted);
    faulted.style.backgroundImage='linear-gradient(rgb(40,60,80),rgb(100,120,140))';
    faulted.style.setProperty('--zxs-contour-extent','16px');
    failAfterInsert=faulted;host.append(faulted);await frames(8);
    check(expectedHostFailures.length===1 && !!insertedBeforeFailure,
      'one synthetic post-insert host failure retains its concrete diagnostic');
    check(faulted.dataset.zxsContourState==='paint' && insertedBeforeFailure.parentNode===faulted,
      'post-error author reconciliation recovers paint with actual inserted output');
    const failedWriteJournal=journalCounts();
    stable.remove();await frames(4);
    // Computed styles depend on document attachment; only authored inline
    // values/priorities and original markers are comparable after detachment.
    restoredPaint(stable,stableOriginal,'detached record restores paint and marker',false);
    // Public non-bfcache lifecycle: close with queued structural work, then
    // observe four frames without invoking a dropped native observer closure.
    const late=document.createElement('div');shape(late);late.style.setProperty('--zxs-contour-extent','16px');
    const errorsBefore=window.__zxsSmokeErrors.length;
    host.append(late);
    window.dispatchEvent(new PageTransitionEvent('pagehide',{persisted:false}));
    const stoppedHandles=totalHandleAcquisitions, stoppedReads=totalPropertyReads;
    await frames(4);
    check(totalHandleAcquisitions===stoppedHandles && totalPropertyReads===stoppedReads && !late.hasAttribute('data-zxs-contour-state'),
      'closed controller ignores queued discovery and paints no late candidate in four-frame window');
    restoredPaint(adopted,adoptedOriginal,'pagehide restores connected author paint and markers',false);
    check(window.__zxsSmokeErrors.length===errorsBefore,'pagehide late observation window has no new uncaught errors');
    return {passed:true,decoys:decoys.size,animation,textUpdate,reparentReads,reparentDiscovery,hiddenState,scrollTrace,insertions,authorRemovals,
      scrollCorrectionTrace,authorOutputRepair,stableRefreshJournal,decorationCreationJournal,decorationRemovalJournal,
      afterDecoration,afterDecorationRemoval,expectedHostFailures,failedWriteJournal,stoppedHandles,lateObservationFrames:4,finalCounters:counts()};
  } finally {
    window.getComputedStyle=originalStyle;Document.prototype.querySelectorAll=originalQuery;
    SVGGraphicsElement.prototype.getScreenCTM=originalScreenCtm;CSSStyleDeclaration.prototype.setProperty=originalSetProperty;
    Element.prototype.querySelectorAll=originalElementQuery;
    CSSStyleDeclaration.prototype.getPropertyValue=originalProperty;
    Node.prototype.compareDocumentPosition=originalPosition;
    if(takeRecordsDescriptor)Object.defineProperty(MutationObserver.prototype,'takeRecords',takeRecordsDescriptor);
    else delete MutationObserver.prototype.takeRecords;
    console.error=originalConsoleError;
    Node.prototype.appendChild=originalAppend;Node.prototype.insertBefore=originalInsert;Node.prototype.removeChild=originalRemove;
    host.remove();for(const index of rules.reverse())sheet.deleteRule(index);await frames(3);
  }
})().then(done,error=>done({passed:false,error:String(error),stack:error.stack}));
"#;
