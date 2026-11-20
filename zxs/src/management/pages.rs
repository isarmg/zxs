//! Built-in HTML only. Dynamic values never become markup, CSS or arbitrary
//! URLs. The public sign-in view has no parameter capable of carrying metadata.
use crate::{OperationId,Role,system::reader::account::{overview::Overview,share_mode::Mode}};
use std::{fmt::Write,io};

pub(super) const CSS:&str=include_str!("../../web/management.css");
const HEAD:&str=r#"<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><link rel="stylesheet" href="/management.css">"#;
fn text(out:&mut String,s:&str){
    for c in s.chars(){match c{'&'=>out.push_str("&amp;"),'<'=>out.push_str("&lt;"),'>'=>out.push_str("&gt;"),
        '"'=>out.push_str("&quot;"),'\''=>out.push_str("&#39;"),_=>out.push(c)}}
}
pub(super) fn sign_in(failed:bool,secure:bool)->String{
    let mut out=String::from(HEAD);
    out.push_str(r#"<title>管理员登录 · ZXS</title></head><body><main class="authentication-shell" data-zxs-scope><div class="authentication-content"><div class="authentication-card"><p>ZXS 管理</p><h1>管理员登录</h1><p>请使用 Web 管理员账号。SMB 文件账号不能在此登录。</p>"#);
    if !secure{out.push_str(r#"<p>此连接使用 HTTP，不加密口令。请仅在可信网络登录，或使用你自己的 HTTPS 入口。</p>"#);}
    if failed{out.push_str(r#"<p class="zxs-error" role="alert">管理员登录名或口令不正确。</p>"#);}
    out.push_str(r#"<form method="post" action="/login" accept-charset="UTF-8">"#);
    out.push_str(include_str!("../../web/login-fields.html"));
    out.push_str("</form></div></div></main></body></html>");out
}
pub(super) fn overview(data:&Overview,csrf:&str)->io::Result<String>{
    let mut out=String::from(HEAD);out.push_str("<title>管理概览 · ZXS</title></head><body>");
    out.push_str(include_str!("../../web/shell-start.html"));
    out.push_str(r#"<section id="zxs-status" class="zxs-panel"><h2>系统与磁盘</h2><div class="zxs-metric-grid"><article class="zxs-metric-card"><h3>SYSTEM 登记</h3><strong class="zxs-metric-current">已核对</strong><small>本次 SQLite 读取与原 SYSTEM 卷标相符。</small></article>"#);
    write!(out,r#"<article class="zxs-metric-card"><h3>普通数据卷</h3><strong class="zxs-metric-current">{}</strong><small>已登记数量，不是在线数量。</small></article></div><p>PoolId：<code>{}</code></p>"#,data.volumes.iter().filter(|(r,_)|*r==Role::File).count(),data.pool).unwrap();
    out.push_str(r#"<div class="zxs-table-scroll"><table class="zxs-table"><caption>本池登记卷</caption><thead><tr><th scope="col">角色</th><th scope="col">VolumeId</th><th scope="col">本次观察</th></tr></thead><tbody>"#);
    for &(r,id) in &data.volumes{
        let (label,state)=match r{Role::System=>("SYSTEM","已核登记与卷标"),Role::File=>("普通数据","仅有登记"),Role::Recycle=>("专用回收","仅有登记")};
        write!(out,"<tr><td>{label}</td><td><code>{id}</code></td><td>{state}</td></tr>").unwrap();
    }
    out.push_str(r#"</tbody></table></div><p class="zxs-status">本页未采集文件系统 UUID、健康及读写速率，不能据此判定健康或空闲。</p></section><section id="zxs-shares" class="zxs-panel"><h2>共享配置</h2>"#);
    out.push_str(r#"<form class="zxs-fields" method="post" action="/shares/publish"><h3>登记新共享</h3><p>只可选择已登记的普通卷。新共享默认停用。显示名称与 SMB 服务名分开。</p><label for="publish-volume">普通卷</label><select id="publish-volume" required name="volume">"#);
    for &(r,id) in &data.volumes{if r==Role::File{write!(out,r#"<option value="{id}">{id}</option>"#).unwrap();}}
    out.push_str(r#"</select><label for="publish-label">共享显示名称</label><input class="zxs-input" id="publish-label" name="label" required maxlength="255">"#);
    hidden(&mut out,csrf)?;out.push_str(r#"<button class="zxs-button" type="submit">登记为停用共享</button></form>"#);
    if data.shares.is_empty(){out.push_str("<p>本页没有已登记的共享。</p>");}
    else{
        out.push_str(r#"<div class="zxs-table-scroll"><table class="zxs-table"><thead><tr><th scope="col">共享显示名称</th><th scope="col">SMB 服务名</th><th scope="col">普通卷</th><th scope="col">SMB 设置</th><th scope="col">修改设置</th></tr></thead><tbody>"#);
        for share in &data.shares{
            let mode=match share.mode{Mode::Disabled=>"停用",Mode::ReadOnly=>"只读",Mode::ReadWrite=>"读写"};
            out.push_str("<tr><td><bdi>");text(&mut out,&share.label);
            write!(out,r#"</bdi></td><td><code>{}</code></td><td><code>{}</code></td><td>{mode}</td><td><form method="post" action="/shares/mode"><input type="hidden" name="share" value="{}"><input type="hidden" name="previous" value="{}">"#,share.share.smb_service(),share.volume,share.share,share.previous).unwrap();
            hidden(&mut out,csrf)?;out.push_str(r#"<label>新模式 <select name="mode">"#);
            for (value,label,active) in [("off","停用",Mode::Disabled),("ro","只读",Mode::ReadOnly),("rw","读写",Mode::ReadWrite)]{
                let selected=if active==share.mode{" selected"}else{""};write!(out,r#"<option value="{value}"{selected}>{label}</option>"#).unwrap();
            }
            out.push_str(r#"</select></label><button type="submit" class="zxs-button">保存</button></form></td></tr>"#);
        }
        out.push_str("</tbody></table></div><p>用站点提供的服务器名与 SMB 服务名组成网络路径。这里显示的是本池登记，不代表 SMB 已上线。</p>");
    }
    if let Some(k)=data.next{write!(out,r##"<p><a href="/?after={k}#zxs-shares">下一页共享</a></p>"##).unwrap();}
    out.push_str(r#"</section><section id="zxs-versions" class="zxs-panel"><h2>版本元信息</h2><p>此版本管理页尚未接入正式节点查询。工作稿不计为正式版本。</p></section><section id="zxs-credentials" class="zxs-panel"><h2>固定账号</h2><div class="zxs-account-grid"><article class="zxs-account-card"><h3>Web 管理员</h3><bdi>"#);
    text(&mut out,&data.web);out.push_str(r#"</bdi><small>仅用于管理，不能读取文件正文。</small></article><article class="zxs-account-card"><h3>SMB 文件账号</h3><bdi>"#);text(&mut out,&data.smb);
    out.push_str(r#"</bdi><small>只能按已启用共享的模式访问文件。</small></article></div></section><form method="post" action="/logout"><input type="hidden" name="csrf" value=""#);
    text(&mut out,csrf);out.push_str(r#""><button class="zxs-button" type="submit">退出管理</button></form></main></div></div></body></html>"#);Ok(out)
}

fn hidden(out:&mut String,csrf:&str)->io::Result<()>{
    // The given view retains one original Op per independent form. Retrying
    // the same POST reuses it; a refreshed view isn't an automatic retry.
    let op=OperationId::new()?;write!(out,r#"<input type="hidden" name="op" value="{op}"><input type="hidden" name="csrf" value=""#).unwrap();
    text(out,csrf);out.push_str(r#"">"#);Ok(())
}
