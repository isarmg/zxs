"use strict";
(() => {
  const el = id => document.getElementById(id);
  let bearer = "", csrf = "", ticket = "", busy = false;
  const labels = {disks:"设备与挂载", rates:"被动 I/O 速率", storage:"终端预选挂载容量", tools:"随包与宿主工具"};
  const buttons = ["pair",...Object.keys(labels),"guide","guide-refresh","preview","confirm","receipt","close"].map(el);
  const leasedTools=new Map([
    ["ext4",["ext 文件系统（ext2/3/4） · e2fsck -fn","check-ext4-copy"]],
    ["XFS",["XFS · xfs_repair -n -f","check-xfs-copy"]],
    ["NTFS",["NTFS · ntfsfix -n（有限元数据观察）","check-ntfs-copy"]],
    ["Btrfs",["Btrfs 单设备副本 · btrfs check --readonly","check-btrfs-copy"]]
  ]);
  const setupRows=["setup-sam","setup-smb","setup-http"];
  const setupLabels=new Map([
    ["equal","与当前 helper 内置的模板字节一致"],
    ["different","与当前 helper 的模板不同；请在终端核实同版发行"],
    ["unconfirmed","未能核对；不要视为模板合格"]
  ]);
  const text = (tag, value) => {const n=document.createElement(tag); n.textContent=value; return n;};
  const show = (id, yes) => {el(id).hidden=!yes;};
  const clearGuide = () => {for(const row of setupRows)el(row).textContent="未观察";};
  const erase = () => {bearer="";csrf="";ticket="";clearGuide();show("setup",false);show("checks",false);show("selection",false);el("imgdigest").textContent="";el("imggeometry").textContent="";el("code").value="";el("cols").replaceChildren();el("rows").replaceChildren();el("evidence").textContent="";show("result",false);};
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
    [bearer,csrf]=credentials;show("pairing",false);show("actions",true);show("checks",true);state("已配对。十分钟后会话关闭；刷新页面不会保留凭据。");
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
  async function guide() {
    clearGuide();show("setup",true);state("正在核对固定安装模板……");
    const response=await post("/api/install/guide",auth());
    const msg=await response.text();
    if (!response.ok){state(msg);return;}
    const [header,...result]=msg.split("\n");
    if (header!=="helper-install-guide-2"||result.length!==setupRows.length||!result.every(v=>setupLabels.has(v)))
      throw Error("invalid fixed template observation");
    result.forEach((v,i)=>{el(setupRows[i]).textContent=setupLabels.get(v);});
    state("只观察模板字节。请用另一个本机终端完成实际安装并核对原回执。");
  }
  async function preview() {
    ticket="";show("selection",false);
    const response=await post("/api/copy/preview",auth());const msg=await response.text();
    if(!response.ok){ticket="";show("selection",false);state(msg);return;}
    const [key,hash,format,bytes,sector,detail,phase,...extra]=msg.split("\n");
    const leased=leasedTools.get(format);
    const knownSector=["512","1024","2048","4096"].includes(sector);
    const sizeValid=/^[1-9][0-9]{0,18}$/.test(bytes)&&(bytes.length<19||bytes<="9223372036854775807");
    const validShape=leased?sector==="-"&&detail==="-":
      knownSector&&(format==="FAT32"&&/^[0-9]{1,10}$/.test(detail)||format==="exFAT"&&detail==="-");
    if(extra.length||![key,hash].every(x=>/^[0-9a-f]{64}$/.test(x))||
       !sizeValid||!["prepared","claimed"].includes(phase)||!validShape)throw Error("invalid preview");
    ticket=key;el("imgdigest").textContent=hash;
    const check=format==="FAT32"?detail+" 个数据簇；fsck.fat -n":"单 FAT；fsck.exfat -n";
    el("imggeometry").textContent=leased ?
      leased[0]+" · "+bytes+" 字节；确认后才读取并核对摘要" :
      format+" · "+bytes+" 字节；逻辑扇区 "+sector+" 字节；"+check;
    show("selection",true);show("confirm",phase==="prepared");
    const remainder=leased?"读完整摘要和检查共用配对后剩余时间；大镜像可另在终端用 helper "+
      leased[1]+" IMAGE --sha256 HEX。":"";
    state(phase==="prepared"?"请对照你的独立摘要。90 秒内确认才会开始；不确认不会执行。"+remainder:"此目标已认领，只能查询原结果。");
  }
  async function check(submit) {
    if(!ticket)return;
    const headers=auth().headers;headers["Content-Type"]="application/octet-stream";
    if(submit)show("confirm",false); // The server may claim it even if our response is lost.
    const response=await post(submit?"/api/copy/check":"/api/copy/result",{headers,body:ticket});
    // If the outcome is unknown after disconnect, query the original receipt.
    // Never substitute a new target or silently issue another command.
    state(await response.text());
  }
  for(const action of ["guide","guide-refresh"])el(action).addEventListener("click",()=>once(guide));
  el("preview").addEventListener("click",()=>once(preview));
  el("confirm").addEventListener("click",()=>once(()=>check(true)));
  el("receipt").addEventListener("click",()=>once(()=>check(false)));
  el("pair").addEventListener("click",()=>once(pair));
  el("code").addEventListener("keydown",event=>{if(event.key==="Enter")once(pair);});
  for(const id of Object.keys(labels))el(id).addEventListener("click",()=>once(()=>observe(id)));
  el("close").addEventListener("click",()=>once(async()=>{
    const response=await post("/api/close",auth());if(!response.ok){state("无法确认会话已结束。");return;}
    const message=await response.text();erase();show("actions",false);state(message);
  }));
  window.addEventListener("pagehide",()=>{erase();show("actions",false);});
})();
