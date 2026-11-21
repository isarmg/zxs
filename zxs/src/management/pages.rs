//! Built-in HTML only. Dynamic values never become markup, CSS or arbitrary
//! URLs. The public sign-in view has no parameter capable of carrying metadata.
use crate::{FileId,OperationId,system::reader::account::{history::{Timeline,TimelinePlace},manual_work,overview::Overview,share_mode::Mode}};
use super::Reply;
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
    out.push_str(r#"<section id="zxs-shares" class="zxs-panel"><h2>共享配置</h2>"#);
    out.push_str(r#"<p>SMB 只有 <code>\\服务器\ZXS</code> 一个业务入口。这里管理首装已选的数据盘目录及其访问模式，不新建共享或暴露宿主路径。</p>"#);
    if data.shares.is_empty(){out.push_str("<p>本页没有首装已选的数据盘目录。</p>");}
    else{
        out.push_str(r#"<div class="zxs-table-scroll"><table class="zxs-table"><thead><tr><th scope="col">ZXS 下的目录</th><th scope="col">普通卷</th><th scope="col">SMB 设置</th><th scope="col">修改设置</th></tr></thead><tbody>"#);
        for share in &data.shares{
            let mode=match share.mode{Mode::Disabled=>"停用",Mode::ReadOnly=>"只读",Mode::ReadWrite=>"读写"};
            out.push_str("<tr><td><bdi>");text(&mut out,&share.label);
            write!(out,r#"</bdi></td><td><code>{}</code></td><td>{mode}</td><td><form method="post" action="/shares/mode"><input type="hidden" name="share" value="{}"><input type="hidden" name="previous" value="{}">"#,share.volume,share.share,share.previous).unwrap();
            hidden(&mut out,csrf)?;out.push_str(r#"<label>新模式 <select name="mode">"#);
            for (value,label,active) in [("off","停用",Mode::Disabled),("ro","只读",Mode::ReadOnly),("rw","读写",Mode::ReadWrite)]{
                let selected=if active==share.mode{" selected"}else{""};write!(out,r#"<option value="{value}"{selected}>{label}</option>"#).unwrap();
            }
            out.push_str(r#"</select></label><button type="submit" class="zxs-button">保存</button></form></td></tr>"#);
        }
        out.push_str("</tbody></table></div><p>请使用站点提供的服务器地址。这里显示的是本池授权，不代表网络已上线或磁盘可用。</p>");
    }
    if let Some(k)=data.next{write!(out,r##"<p><a href="/?after={k}#zxs-shares">下一页目录</a></p>"##).unwrap();}
    out.push_str(r#"</section><section id="zxs-versions" class="zxs-panel"><h2>手动提交版本</h2><p>保存只更新工作稿。关闭应用写入后，在此选择文件并明确提交才会生成正式版本。</p><p><a href="/versions">从文件列表选择</a></p></section><section id="zxs-credentials" class="zxs-panel"><h2>Web 与 SMB 共用账号</h2><div class="zxs-account-grid"><article class="zxs-account-card"><h3>登录名</h3><bdi>"#);
    text(&mut out,&data.login);
    out.push_str(r#"</bdi><small>Web 仅用于管理，SMB 只能按已启用目录的模式访问文件。</small></article></div></section><form method="post" action="/logout"><input type="hidden" name="csrf" value=""#);
    text(&mut out,csrf);out.push_str(r#""><button class="zxs-button" type="submit">退出管理</button></form></main></div></div></body></html>"#);Ok(out)
}

fn hidden(out:&mut String,csrf:&str)->io::Result<()>{
    // The given view retains one original Op per independent form. Retrying
    // the same POST reuses it; a refreshed view isn't an automatic retry.
    let op=OperationId::new()?;write!(out,r#"<input type="hidden" name="op" value="{op}"><input type="hidden" name="csrf" value=""#).unwrap();
    text(out,csrf);out.push_str(r#"">"#);Ok(())
}

pub(super) fn manual_index(items:&[manual_work::PickerItem],next:Option<FileId>)->Reply{
    let mut out=String::from(HEAD);
    out.push_str(r#"<title>选择文件 · ZXS</title></head><body><main data-zxs-scope><h1>选择文件</h1><p>仅显示已登记并确认的文件，不探测数据盘或打开正文。</p>"#);
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

#[cfg(test)]
#[test]
fn an_unsettled_parent_never_becomes_an_old_path_or_a_submit_link(){
    use crate::VolumeId;
    use manual_work::{Item,PickerItem};
    let blocked=FileId([0x20;16]);let other=FileId([0x22;16]);
    let page=manual_index(&[PickerItem::ParentPending(blocked),PickerItem::Named(Item{
        file:other,name:"Other.txt".into(),volume:VolumeId([0x42;16]),
    })],None);
    assert!(page.body.contains(&format!("<code>{blocked}</code>"))&&page.body.contains("所属目录改名尚未确认"));
    assert!(!page.body.contains(&format!("?file={blocked}")));
    assert!(page.body.contains(&format!("/versions?file={other}"))&&page.body.contains("Other.txt"));
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
    out.push_str("<p>这里只列已提交的版本，不读取正文或核验磁盘。</p>");
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

#[cfg(test)]
#[test]
fn pending_parent_history_displays_only_original_identity_versions_and_read_navigation(){
    use crate::{NodeId,VersionId,system::reader::account::history::Version};
    let file=FileId([0x41;16]);let version=VersionId([0x42;16]);let node=NodeId([0x43;16]);
    let history=Timeline{file,place:TimelinePlace::ParentPending,next:Some(1),
        rows:vec![Version{order:2,node,version,parent:Some(NodeId([0x44;16])),revision:3,length:7}]};
    let body=version_history(&history).body;
    assert!(body.contains(&format!("<code>{file}</code>"))&&body.contains("所属目录改名尚未确认"));
    assert!(body.contains(&format!("<code>{version}</code>"))&&body.contains(&format!("<code>{node}</code>")));
    assert!(body.contains(&format!("/versions/history?file={file}&amp;before=1")));
    for unwarranted in ["<bdi>","当前卷内路径","数据卷","回收卷","/versions?file=","<form","/download"]{
        assert!(!body.contains(unwarranted));
    }
}

pub(super) fn manual(item:&manual_work::Item,t:manual_work::Target,op:OperationId,csrf:&str)->Reply{
    let mut out=String::from(HEAD);
    out.push_str("<title>手动提交版本 · ZXS</title></head><body><main data-zxs-scope><h1>手动提交版本</h1>");
    out.push_str("<p>卷内路径 <bdi>");text(&mut out,&item.name);
    write!(out,r#"</bdi>，数据卷 <code>{}</code>，当前这一轮的工作修订 {}。</p><p>提交会冻结这一版，不做全量复制。仍有写入时不会提交。</p>
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
