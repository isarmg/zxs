"use strict";
(() => {
  const el = id => document.getElementById(id);
  let bearer = "", csrf = "", busy = false;
  const labels = {disks:"设备与挂载", rates:"被动 I/O 速率", tools:"随包与宿主工具"};
  const buttons = ["pair","disks","rates","tools","close"].map(el);
  const text = (tag, value) => {const n=document.createElement(tag); n.textContent=value; return n;};
  const show = (id, yes) => {el(id).hidden=!yes;};
  const erase = () => {bearer="";csrf="";el("code").value="";el("cols").replaceChildren();el("rows").replaceChildren();el("evidence").textContent="";show("result",false);};
  const state = msg => {el("notice").textContent=msg;};
  async function once(fn) {
    if (busy) return;
    busy=true; buttons.forEach(b=>{b.disabled=true;});
    try {await fn();} catch (_) {state("未取得完整结果。请检查终端中的服务状态；不要将断线当作已确认。");}
    finally {busy=false;buttons.forEach(b=>{b.disabled=false;});}
  }
  async function post(path, options) {
    return fetch(path,{method:"POST",mode:"same-origin",credentials:"omit",cache:"no-store",
      redirect:"error",referrerPolicy:"same-origin",...options});
  }
  async function pair() {
    const code=el("code").value;el("code").value="";
    if (!/^[0-9a-f]{64}$/.test(code)) {state("请输入本次终端中完整的 64 位配对码。");return;}
    const response=await post("/api/pair",{headers:{"Content-Type":"application/octet-stream"},body:code});
    const msg=await response.text();
    if (!response.ok){state(msg);return;}
    const credentials=msg.split("\n");
    if (credentials.length!==2||!credentials.every(v=>/^[0-9a-f]{64}$/.test(v))) throw Error("invalid session");
    [bearer,csrf]=credentials;show("pairing",false);show("actions",true);state("已配对。十分钟后会话关闭；刷新页面不会保留凭据。");
  }
  const auth=()=>({headers:{"Authorization":"Bearer "+bearer,"X-Helper-CSRF":csrf}});
  function render(action,body) {
    const records=body.replace(/\n$/,"").split("\n");if(records.length<2)throw Error("incomplete view");
    const [evidence,head,...rows]=records;const labels=head.split("\t");
    const tr=document.createElement("tr");for(const value of labels)tr.append(text("th",value));
    const fragment=document.createDocumentFragment();
    for(const row of rows){const cells=row.split("\t");if(cells.length!==labels.length)throw Error("invalid row");
      const item=document.createElement("tr");for(const value of cells)item.append(text("td",value));fragment.append(item);}
    el("resulttitle").textContent=action;el("evidence").textContent=evidence;el("cols").replaceChildren(tr);
    el("rows").replaceChildren(fragment);show("result",true);state(rows.length ? "已取得本次观察。未知、缺失或无权限的项不会补成正常值。" : "此次观察没有行。");
  }
  async function observe(id){
    show("result",false);state(id==="rates"?"正在等待第二次计数采样……":"正在观察……");
    const response=await post("/api/"+id,auth());const msg=await response.text();
    if(!response.ok){state(msg);return;}render(labels[id],msg);
  }
  el("pair").addEventListener("click",()=>once(pair));
  el("code").addEventListener("keydown",event=>{if(event.key==="Enter")once(pair);});
  for(const id of Object.keys(labels))el(id).addEventListener("click",()=>once(()=>observe(id)));
  el("close").addEventListener("click",()=>once(async()=>{
    const response=await post("/api/close",auth());if(!response.ok){state("无法确认会话已结束。");return;}
    erase();show("actions",false);state("本次面板已关闭。");
  }));
  window.addEventListener("pagehide",()=>{erase();show("actions",false);});
})();
