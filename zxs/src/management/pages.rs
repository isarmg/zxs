//! Built-in HTTP templates. Dynamic values never become markup, CSS or arbitrary
//! URLs. The public sign-in view has no parameter capable of carrying metadata.
use crate::{FileId,OperationId,system::reader::account::{history::{Timeline,TimelinePlace},manual_work,overview::Overview,share_mode::Mode}};
use super::Reply;
use std::{fmt::Write,io};

pub(super) const CSS:&str=include_str!("assets/management.css");
const G4_DEFS:&str=include_str!("assets/g4-corner.svg");
const G4_PAINT:&str=include_str!("assets/g4-paint.html");
const HEAD:&str=r#"<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><link rel="stylesheet" href="/management.css">"#;
fn text(out:&mut String,s:&str){
    for c in s.chars(){match c{'&'=>out.push_str("&amp;"),'<'=>out.push_str("&lt;"),'>'=>out.push_str("&gt;"),
        '"'=>out.push_str("&quot;"),'\''=>out.push_str("&#39;"),_=>out.push(c)}}
}
pub(super) fn sign_in(failed: bool) -> String {
    let mut out = String::from(HEAD);
    out.push_str("<title>管理员登录 · ZXS</title></head><body>");
    out.push_str(G4_DEFS);
    out.push_str(r#"<main class="authentication-shell" data-zxs-scope><div class="authentication-content"><section class="authentication-card zxs-g4-surface" aria-labelledby="zxs-login-title">"#);
    out.push_str(G4_PAINT);
    out.push_str(r#"<h1 id="zxs-login-title" class="zxs-visually-hidden">管理员登录 · ZXS</h1><form method="post" action="/login" accept-charset="UTF-8"><fieldset class="zxs-auth-group"><legend class="zxs-visually-hidden">Web 管理员登录</legend><div class="zxs-auth-fields">"#);
    out.push_str(include_str!("assets/login-fields.html"));
    out.push_str(r#"<div class="zxs-auth-information">"#);
    out.push_str(r#"<p class="zxs-notice zxs-notice-warning">HTTP 未加密。请仅在可信网络登录。</p>"#);
    if failed {
        out.push_str(r#"<p class="zxs-notice zxs-notice-error" role="alert">登录名或口令不正确。</p>"#);
    }
    out.push_str(r#"</div><div class="zxs-auth-actions"><button class="zxs-button" type="submit"><svg viewBox="0 0 24 24" width="1em" height="1em" fill="none" stroke="currentColor" stroke-width="1.4" aria-hidden="true" focusable="false"><path d="M13 4h7v16h-7M3 12h12m-4-4 4 4-4 4"/></svg>登录</button></div></div></fieldset></form></section></div></main></body></html>"#);
    out
}

pub(super) fn browser_overview(data: &Overview, csrf: &str) -> io::Result<String> {
    let mut out = String::from(HEAD);
    out.push_str(r#"<link rel="stylesheet" href="/ui/v1/folder.css"><script type="module" src="/ui/v1/boot.js"></script>"#);
    out.push_str("<title>管理概览 · ZXS</title></head><body>");
    out.push_str(G4_DEFS);
    out.push_str(include_str!("assets/shell-start.html"));
    out.push_str(r#"<h1 class="zxs-visually-hidden">管理概览</h1>"#);
    browser_panel(&mut out, data);
    out.push_str(r#"<section id="zxs-shares" class="zxs-section" aria-labelledby="zxs-shares-title"><div class="zxs-section-heading zxs-flow"><h2 id="zxs-shares-title" class="zxs-row">共享配置</h2><dl class="zxs-endpoint zxs-row"><dt>SMB 共享名</dt><dd><code>ZXS</code></dd></dl></div>"#);
    if data.shares.is_empty() {
        out.push_str(r#"<p class="zxs-share-frame zxs-share-empty">暂无共享目录。</p>"#);
    } else {
        out.push_str(r#"<div class="zxs-share-frame">
<div class="zxs-share-line zxs-share-head" aria-hidden="true"><span>共享目录</span><span>SMB 权限</span><span>修改权限</span></div>
<ul class="zxs-share-rows" role="list" aria-labelledby="zxs-shares-title">"#);
        for share in &data.shares {
            let mode = match share.mode {
                Mode::Disabled => "停用",
                Mode::ReadOnly => "只读",
                Mode::ReadWrite => "读写",
            };
            let volume = share.volume.to_string();
            let suffix = volume.get(volume.len().saturating_sub(8)..).unwrap_or(&volume);
            let id = share.share;
            write!(out, r#"<li class="zxs-share-line">
<div class="zxs-share-directory"><bdi class="zxs-share-name" id="zxs-share-{id}">"#).unwrap();
            text(&mut out, &share.label);
            write!(out, r#"</bdi><details class="zxs-volume-id"><summary>卷 <code>…{suffix}</code></summary>
<code class="zxs-volume-value">{volume}</code></details></div>
<span class="zxs-mode zxs-status"><span class="zxs-visually-hidden">SMB 权限：</span>{mode}</span>
<form class="zxs-mode-form" method="post" action="/shares/mode">
<input type="hidden" name="share" value="{id}"><input type="hidden" name="previous" value="{}">"#, share.previous).unwrap();
            hidden(&mut out, csrf)?;
            write!(out, r#"<label class="zxs-visually-hidden" id="zxs-mode-label-{id}" for="zxs-mode-{id}">修改权限</label>
<select class="zxs-input" name="mode" id="zxs-mode-{id}" aria-labelledby="zxs-mode-label-{id} zxs-share-{id}">"#).unwrap();
            for (value, label, active) in [
                ("off", "停用", Mode::Disabled),
                ("ro", "只读", Mode::ReadOnly),
                ("rw", "读写", Mode::ReadWrite),
            ] {
                let selected = if active == share.mode { " selected" } else { "" };
                write!(out, r#"<option value="{value}"{selected}>{label}</option>"#).unwrap();
            }
            write!(out, r#"</select>
<button type="submit" class="zxs-button" id="zxs-save-{id}" aria-labelledby="zxs-save-{id} zxs-share-{id}">保存</button></form></li>"#).unwrap();
        }
        out.push_str("</ul></div>");
    }
    if let Some(k) = data.next {
        write!(out, r##"<p class="zxs-pagination"><a class="zxs-button" href="/?after={k}#zxs-shares">下一页目录</a></p>"##).unwrap();
    }
    out.push_str(r#"</section><div id="zxs-console-secondary" class="zxs-overview-actions"><section id="zxs-versions" class="zxs-utility-card zxs-g4-surface" aria-labelledby="zxs-versions-title">"#);
    out.push_str(G4_PAINT);
    out.push_str(r#"<h2 id="zxs-versions-title">手动提交版本</h2><p><a class="zxs-button" href="/versions">选择文件</a></p></section>
<section id="zxs-credentials" class="zxs-utility-card zxs-g4-surface" aria-labelledby="zxs-credentials-title">"#);
    out.push_str(G4_PAINT);
    out.push_str(r#"<h2 id="zxs-credentials-title">Web 与 SMB 共用账号</h2><div class="zxs-account-line"><dl><dt>登录名</dt><dd><bdi>"#);
    text(&mut out, &data.login);
    out.push_str(r#"</bdi></dd></dl><form id="zxs-logout" method="post" action="/logout"><input type="hidden" name="csrf" value=""#);
    text(&mut out, csrf);
    out.push_str(r#""></form></div></section></div></main></div></div></body></html>"#);
    Ok(out)
}

fn browser_panel(out: &mut String, data: &Overview) {
    out.push_str(r#"<section id="zxs-files" class="zxs-file-manager zxs-stack" aria-labelledby="zxs-folder-title" hidden><div id="zxs-file-corner-source" hidden>"#);
    out.push_str(G4_PAINT);
    out.push_str(r#"</div><h2 id="zxs-folder-title" class="zxs-row" tabindex="-1">目录</h2><div id="zxs-folder-content"></div></section>
<p id="zxs-folder-load-error" role="alert" hidden>目录未加载，请刷新。</p>
<div id="zxs-browser-roots" hidden"#);
    if let Some(next) = data.next {
        write!(out, r#" data-zxs-next="{next}""#).unwrap();
    }
    out.push('>');
    for share in &data.shares {
        if share.mode != Mode::Disabled {
            write!(out, r#"<span data-zxs-share="{}">"#, share.share).unwrap();
            text(out, &share.label);
            out.push_str("</span>");
        }
    }
    out.push_str("</div>");
}



fn hidden(out:&mut String,csrf:&str)->io::Result<()>{
    // The given view retains one original Op per independent form. Retrying
    // the same POST reuses it; a refreshed view isn't an automatic retry.
    let op=OperationId::new()?;write!(out,r#"<input type="hidden" name="op" value="{op}"><input type="hidden" name="csrf" value=""#).unwrap();
    text(out,csrf);out.push_str(r#"">"#);Ok(())
}

pub(super) fn manual_index(items:&[manual_work::PickerItem],next:Option<FileId>)->Reply{
    let mut out=String::from(HEAD);
    out.push_str(r#"<title>选择文件 · ZXS</title></head><body><main data-zxs-scope><h1>选择文件</h1>"#);
    if items.is_empty(){out.push_str("<p>没有已确认的文件。</p>");}
    else{
        out.push_str(r#"<table class="zxs-table"><thead><tr><th>FileId</th><th>卷内路径</th><th>数据卷</th><th>工作稿</th></tr></thead><tbody>"#);
        for i in items{
            match i{
                manual_work::PickerItem::Named(row)=>{
                    write!(out,r#"<tr><td><code>{}</code></td><td><a href="/versions/history?file={}"><bdi>"#,row.file,row.file).unwrap();
                    text(&mut out,&row.name);write!(out,r#"</bdi></a></td><td><code>{}</code></td><td><a href="/versions?file={}">手动提交</a></td></tr>"#,row.volume,row.file).unwrap();
                },
                manual_work::PickerItem::ParentPending(file)=>write!(out,
                    "<tr><td><code>{file}</code></td><td>所属目录改名尚未确认</td><td>未显示</td><td>暂不能提交</td></tr>").unwrap(),
            }
        }
        out.push_str("</tbody></table>");
    }
    if let Some(after)=next{write!(out,r#"<p><a href="/versions?after={after}">下一页</a></p>"#).unwrap();}
    out.push_str(r#"<p><a href="/">返回概览</a></p></main></body></html>"#);
    Reply::html("200 OK",out)
}


pub(super) fn version_history(history:&Timeline)->Reply{
    let mut out=String::from(HEAD);
    out.push_str("<title>已提交版本 · ZXS</title></head><body><main data-zxs-scope><h1>已提交版本</h1>");
    let named=match &history.place{
        TimelinePlace::Ordinary{name,volume}=>Some(("当前卷内路径","数据卷",name,volume)),
        TimelinePlace::Retained{name,volume}=>Some(("当前保留名称","回收卷",name,volume)),
        TimelinePlace::ParentPending=>{
            write!(out,"<p>文件 <code>{}</code>。所属目录改名尚未确认，暂不显示路径，也不提供提交入口。</p>",history.file).unwrap();
            None
        },
    };
    if let Some((label,role,name,volume))=named{
        write!(out,"<p>{label} <bdi>").unwrap();text(&mut out,name);
        write!(out,"</bdi> · {role} <code>{volume}</code></p>").unwrap();
    }
    if history.rows.is_empty(){out.push_str("<p>尚无已提交版本。</p>");}
    else{
        out.push_str(r#"<table class="zxs-table"><thead><tr><th>序号</th><th>版本 ID</th><th>节点 ID</th><th>父节点</th><th>工作修订</th><th>逻辑字节</th></tr></thead><tbody>"#);
        for v in &history.rows{
            write!(out,"<tr><td>{}</td><td><code>{}</code></td><td><code>{}</code></td><td>",v.order,v.version,v.node).unwrap();
            if let Some(parent)=v.parent{write!(out,"<code>{parent}</code>").unwrap();}else{out.push('—');}
            write!(out,"</td><td>{}</td><td>{}</td></tr>",v.revision,v.length).unwrap();
        }
        out.push_str("</tbody></table>");
    }
    if let Some(upper)=history.next{
        write!(out,r#"<p><a href="/versions/history?file={}&amp;before={upper}">更早的版本</a></p>"#,history.file).unwrap();
    }
    if matches!(&history.place,TimelinePlace::Ordinary{..}){
        write!(out,r#"<p><a href="/versions?file={}">打开手动提交表单</a></p>"#,history.file).unwrap();
    }
    out.push_str(r#"<p><a href="/versions">返回文件列表</a></p></main></body></html>"#);
    Reply::html("200 OK",out)
}


pub(super) fn manual(item:&manual_work::Item,t:manual_work::Target,op:OperationId,csrf:&str)->Reply{
    let mut out=String::from(HEAD);
    out.push_str("<title>手动提交版本 · ZXS</title></head><body><main data-zxs-scope><h1>手动提交版本</h1>");
    out.push_str("<p>卷内路径 <bdi>");text(&mut out,&item.name);
    write!(out,r#"</bdi>，数据卷 <code>{}</code>，当前这一轮的工作修订 {}。</p><p>提交会冻结这一版。请先停止写入。</p>
<form method="post" action="/versions/commit"><input type="hidden" name="op" value="{op}"><input type="hidden" name="csrf" value=""#,item.volume,t.revision).unwrap();
    text(&mut out,csrf);
    write!(out,r#""><button class="zxs-button" type="submit">确认提交此版本</button><button type="submit" formaction="{}">只查本次回执</button></form><p><a href="/">返回概览</a></p></main></body></html>"#,
        super::manual::RECEIPT).unwrap();
    Reply::html("200 OK",out)
}
pub(super) fn manual_receipt(r:manual_work::Receipt)->Reply{
    let mut out=String::from(HEAD);
    write!(out,r#"<title>原提交回执 · ZXS</title></head><body><main data-zxs-scope><h1>已查到原提交回执</h1>
<p>本查询没有提交或重试。</p><p>文件 <code>{}</code>，工作修订 {}，已确认第 {} 个正式版本。</p>
<p>提交操作：<code>{}</code><br>版本 ID：<code>{}</code></p><p><a href="/versions/history?file={}">查看已提交版本</a></p></main></body></html>"#,
        r.target.file,r.target.revision,r.ord,r.original,r.version,r.target.file).unwrap();
    Reply::html("200 OK",out)
}
pub(super) fn frozen(r:manual_work::Receipt)->Reply{
    let mut out=String::from(HEAD);
    write!(out,r#"<title>正式版本已提交 · ZXS</title></head><body><main data-zxs-scope><h1>正式版本已提交</h1>
<p>文件 <code>{}</code> 的第 {} 个版本。</p><p>版本 ID：<code>{}</code></p><p><a href="/">返回概览</a></p></main></body></html>"#,
        r.target.file,r.ord,r.version).unwrap();
    Reply::html("200 OK",out)
}

#[cfg(test)]
include!("../../tests/unit/attached/management/pages/items.rs");
