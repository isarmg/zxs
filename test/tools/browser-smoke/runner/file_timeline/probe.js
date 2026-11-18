// Observe/delay real successful loopback GET deliveries only. Do not invent row
// identities, rewrite URLs, forge a response or drive focus from the harness.
const e=window.__zxsTimelineExpected, native=window.fetch;
const base='/api/file-manager/files/'+e.file+'/history', life=base+'/lifecycle';
const rows=()=>[...document.querySelectorAll('.zxs-lifecycle-record')];
const op=id=>rows().find(n=>n.dataset.zxsLifecycleOperation===id);
const managed=()=>[...document.querySelectorAll('.zxs-version-node[tabindex="-1"]')].find(n=>n.dataset.versionId===e.old);
const graph=()=>op(e.newer)?.parentElement.querySelector('[data-interval="complete"]');
const inside=id=>[...(graph()?.querySelectorAll('.zxs-interval-node')||[])].find(n=>n.dataset.versionId===id);
const t=window.__zxsTimeline={events:[], loads:0, finished:0, frames:0, releaseFn:null, waiting:false, exactObservation:true,
  originalProjection(){
    const first=op(e.older), last=op(e.newer), g=graph(), a=inside(e.one), b=inside(e.two);
    if(!this.exactObservation||!first||!last||!g||!a||!b||managed()||rows()[0]!==last||rows()[1]!==first)return false;
    const edge=[...g.querySelectorAll('[data-edge-kind=ancestry]')].find(n=>n.dataset.parentVersion===e.one && n.dataset.childVersion===e.two);
    if(!edge?.getAttribute('d')||a.getBoundingClientRect().top<=b.getBoundingClientRect().top)return false;
    const guides=[...g.querySelectorAll('[data-edge-kind=event-guide]')];
    const outside=rows().at(-1)?.parentElement.querySelector('[data-interval=open]');
    return g.querySelectorAll('.zxs-interval-node').length===2
      && first.textContent.includes('smoke-admin') && last.textContent.includes('smoke-admin')
      && [...first.querySelectorAll('[data-zxs-related-node]')].some(n=>n.dataset.zxsRelatedNode===e.old)
      && !!outside
      && guides.some(n=>n.dataset.endpointNode===e.two&&n.getAttribute('d')&&n.getAttribute('marker-end')==='none');
  },
  remember(){this.prior=op(e.older);this.card=inside(e.one);this.scroller=graph().querySelector('.zxs-interval-scroll');this.left=this.scroller.scrollLeft;this.loadAt=this.loads;},
  sameProjection(){return this.loads>this.loadAt && !document.querySelector('[data-zxs-lifecycle-load]').disabled
      && op(e.older)===this.prior && inside(e.one)===this.card && this.scroller.scrollLeft===this.left;},
  installed(){return !!managed();},
  held(){return !!this.releaseFn && this.events.at(-1)==='original' && this.events.at(-2)==='anchored' && this.lastHref===this.firstHref;},
  release(){this.releaseFn?.();this.releaseFn=null;},
  dismissed(){return this.frames>=1&&!op(e.older)&&!managed()&&document.activeElement?.dataset.versionId!==e.old;},
  done(){return this.exactObservation&&this.finished===2 && this.events.join(',')==='anchored,original,anchored,original'
      && !!managed() && document.activeElement===managed() && !!op(e.older);},
  restore(){this.release();window.fetch=native;delete window.__zxsTimelineExpected;delete window.__zxsTimeline;}
};
window.fetch=async function(...args){
  const r=args[0], u=new URL(r.url||r,location.href);
  const response=await native.apply(this,args);
  if(u.origin!==location.origin || !response.ok || (r.method||args[1]?.method||'GET')!=='GET')return response;
  if(u.pathname===base&&u.searchParams.get('anchor_node_id')===e.old){t.events.push('anchored');t.waiting=true;}
  if(u.pathname===life){
    const page=await response.clone().json();
    t.exactObservation &&= page.file_id===e.file && page.observation_sequence===e.observation;
    t.loads++;t.firstHref??=u.href;
    if(t.waiting){t.waiting=false;t.events.push('original');t.lastHref=u.href;
      await new Promise(resolve=>{t.releaseFn=resolve;});t.finished++;
      requestAnimationFrame(()=>requestAnimationFrame(()=>t.frames++));
    }
  }
  return response;
};return true;
