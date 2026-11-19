//! Small tabular, text-only view of the same typed observations as the CLI.
//! No row may contribute HTML, control fields, device paths or arguments.
use super::http::Reply;
use crate::{block_tools as disks,fs_tools};
use std::fmt::Display;

fn clean(s:&str)->String{s.chars().map(|c|if c.is_control(){'\u{fffd}'}else{c}).collect()}
fn line(out:&mut String,row:impl IntoIterator<Item=String>){
    let mut it=row.into_iter();if let Some(c)=it.next(){out.push_str(&clean(&c));}
    for c in it{out.push('\t');out.push_str(&clean(&c));}out.push('\n');
}
fn field<T:Display>(f:&Result<T,disks::Reason>)->String{match f{Ok(v)=>v.to_string(),Err(e)=>format!("未知：{}",e.label())}}
pub(super) fn disks(rates:bool)->Reply{
    let report=match disks::Query::inventory(rates).observe(){Ok(x)=>x,Err(_)=>return Reply::plain(503,"未能从当前命名空间核得系统设备盘点；未检查健康或登记。")};
    let mut out=String::new();
    let sampled=match (report.start_ms,report.end_ms){(Some(a),Some(b)) if a<=b=>format!("Unix 毫秒 {a} 至 {b}"),_=>"时间未知".into()};
    line(&mut out,[format!("内核 sysfs / 本线程 mountinfo / 宿主 udev 别名；{sampled}。不叠加分区与整盘；没有直接挂载匹配不代表未使用。")]);
    let mut cols=vec!["设备号","内核名称","类别","块层容量（字节）","型号（sysfs）","FS UUID（别名）","PARTUUID（别名）","直接挂载 ID"];
    if rates{cols.extend(["采样间隔（纳秒）","读取 B/s","写入 B/s"]);}
    line(&mut out,cols.iter().map(|s|s.to_string()));
    for (dev,row) in &report.rows{
        let mut cells=vec![dev.to_string()];
        match row{
            Err(why)=>{cells.push(format!("不可核：{}",why.label()));cells.resize(cols.len(),"—".into());}
            Ok(r)=>{
                let kind=match r.id.kind{disks::Kind::Disk=>"整块设备".into(),disks::Kind::Partition{number,parent,..}=>format!("分区 {number}，父 {parent}")};
                let mounts=match &r.mount_ids{Ok(ids) if ids.is_empty()=>"无直接匹配".into(),
                    Ok(ids)=>ids.iter().map(u64::to_string).collect::<Vec<_>>().join(", "),Err(e)=>format!("未知：{}",e.label())};
                cells.extend([r.id.name.clone(),kind,field(&r.id.sectors.map(|s|u128::from(s)*disks::SECTOR_BYTES)),
                    field(&r.model),field(&r.fs_uuid),field(&r.partuuid),mounts]);
                if rates{match r.rate{Ok(x)=>cells.extend([x.interval_ns.to_string(),x.bps(x.read_bytes),x.bps(x.write_bytes)]),
                    Err(why)=>cells.extend([format!("未知：{}",why.label()),"—".into(),"—".into()])}}
            }
        }
        line(&mut out,cells);
    }
    Reply::plain(200,out)
}
pub(super) fn tools()->Reply{
    let mut out=String::new();let report=fs_tools::Query::catalog().observe();
    line(&mut out,["固定位置的安装元数据，不执行工具、读取镜像或检查磁盘。程序存在不代表版本、签名、健康、可修复或已获授权。".into()]);
    line(&mut out,["格式","工具","来源","安装证据","实际动作边界"].map(str::to_owned));
    for row in report.rows{let p=row.profile;line(&mut out,[p.format.into(),p.tool.into(),
        (if p.private(){"随包固定位置"}else{"宿主固定位置（仅观察）"}).into(),row.installed.slot().into(),p.action().into()]);}
    Reply::plain(200,out)
}
