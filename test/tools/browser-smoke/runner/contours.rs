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
  let scans=0, decoyReads=0, stableReads=0, activeReads=0;
  const reset=()=>{scans=0;decoyReads=0;stableReads=0;activeReads=0;};
  const counts=()=>({scans,decoyReads,stableReads,activeReads});
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
  window.getComputedStyle=function(node,...rest){if(decoys.has(node))decoyReads++;if(node===stable)stableReads++;if(node===active)activeReads++;return Reflect.apply(originalStyle,this,[node,...rest]);};
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
    check(activeReads>0,'actual extent-animation measurement');
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
    reset();text.data='changed observer text';await frames(4);
    const textUpdate=counts();
    check(scans>0 && decoyReads>0 && stableReads>0,'text can affect direction or container queries and must rediscover CSS');
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
    check(queryCandidate.dataset.zxsContourState==='paint' && scans>0,'ResizeObserver must discover newly active container-query candidate');
    widthRule.style.width='200px';await frames(5);
    check(!queryCandidate.hasAttribute('data-zxs-contour-state'),'expanded container restores zero-extent paint');
    labelText.data='abcdefghijklmnopqrstuv';await frames(5);
    check(queryCandidate.dataset.zxsContourState==='paint','nonempty sibling text changes container-query membership');
    dormant.setAttribute('data-zxs-contour-extent','18');await frames(4);
    check(dormant.dataset.zxsContourState==='paint','attribute activates previously zero extent');
    dormant.removeAttribute('data-zxs-contour-extent');await frames(4);
    restoredPaint(dormant,dormantOriginal,'attribute removal restores original paint',true);
    stable.remove();await frames(4);
    // Computed styles depend on document attachment; only authored inline
    // values/priorities and original markers are comparable after detachment.
    restoredPaint(stable,stableOriginal,'detached record restores paint and marker',false);
    return {passed:true,decoys:decoys.size,animation,textUpdate};
  } finally {
    window.getComputedStyle=originalStyle;Document.prototype.querySelectorAll=originalQuery;
    host.remove();for(const index of rules.reverse())sheet.deleteRule(index);await frames(3);
  }
})().then(done,error=>done({passed:false,error:String(error),stack:error.stack}));
"#;
