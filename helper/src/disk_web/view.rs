//! No row may contribute HTML, control fields, device paths or arguments.
use super::http::Reply;
use crate::{block_tools as disks, fs_tools, hardware, pinned::storage_status as storage};
use std::fmt::Display;

fn clean(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { '\u{fffd}' } else { c })
        .collect()
}
fn line(out: &mut String, row: impl IntoIterator<Item = String>) {
    let mut it = row.into_iter();
    if let Some(c) = it.next() {
        out.push_str(&clean(&c));
    }
    for c in it {
        out.push('\t');
        out.push_str(&clean(&c));
    }
    out.push('\n');
}
fn field<T: Display>(f: &Result<T, disks::Reason>) -> String {
    match f {
        Ok(v) => v.to_string(),
        Err(e) => format!("未知：{}", e.label()),
    }
}
fn attachment_cells(a: Result<disks::Attachment, disks::Reason>) -> [String; 3] {
    let source = a.map(|v| match v.source {
        disks::AttachmentSource::KernelAncestor => "当次内核祖先",
        disks::AttachmentSource::InferredParent => "由已核父盘推断",
    });
    [
        field(&a.map(|v| v.subsystem)),
        field(&source),
        field(&a.and_then(|v| v.nvme_transport)),
    ]
}
pub(super) fn disks(rates: bool) -> Reply {
    let report = match disks::Query::inventory(rates).observe() {
        Ok(x) => x,
        Err(_) => {
            return Reply::plain(
                503,
                "未能从当前命名空间核得系统设备盘点；未检查健康或登记。",
            );
        }
    };
    let mut out = String::new();
    let sampled = match (report.start_ms, report.end_ms) {
        (Some(a), Some(b)) if a <= b => format!("Unix 毫秒 {a} 至 {b}"),
        _ => "时间未知".into(),
    };
    line(
        &mut out,
        [format!(
            "内核 sysfs / 本线程 mountinfo / 宿主 udev 别名；{sampled}。子系统是内核祖先证据，不是物理接口或协商速率；不叠加分区与整盘。无直接匹配不代表未使用；未检查健康、温度或池登记。"
        )],
    );
    let mut cols = vec![
        "设备号",
        "内核名称",
        "类别",
        "块层容量（字节）",
        "型号（sysfs）",
        "祖先子系统",
        "子系统证据",
        "NVMe 传输类型",
        "FS UUID（别名）",
        "PARTUUID（别名）",
        "直接挂载 ID",
    ];
    if rates {
        cols.extend(["采样间隔（纳秒）", "读取 B/s", "写入 B/s"]);
    }
    line(&mut out, cols.iter().map(|s| s.to_string()));
    for (dev, row) in &report.rows {
        let mut cells = vec![dev.to_string()];
        match row {
            Err(why) => {
                cells.push(format!("不可核：{}", why.label()));
                cells.resize(cols.len(), "—".into());
            }
            Ok(r) => {
                let kind = match r.id.kind {
                    disks::Kind::Disk => "整块设备".into(),
                    disks::Kind::Partition { number, parent, .. } => {
                        format!("分区 {number}，父 {parent}")
                    }
                };
                let mounts = match &r.mount_ids {
                    Ok(ids) if ids.is_empty() => "无直接匹配".into(),
                    Ok(ids) => ids
                        .iter()
                        .map(u64::to_string)
                        .collect::<Vec<_>>()
                        .join(", "),
                    Err(e) => format!("未知：{}", e.label()),
                };
                cells.extend([
                    r.id.name.clone(),
                    kind,
                    field(&r.id.sectors.map(|s| u128::from(s) * disks::SECTOR_BYTES)),
                    field(&r.model),
                ]);
                cells.extend(attachment_cells(r.attachment));
                cells.extend([field(&r.fs_uuid), field(&r.partuuid), mounts]);
                if rates {
                    match r.rate {
                        Ok(x) => cells.extend([
                            x.interval_ns.to_string(),
                            x.bps(x.read_bytes),
                            x.bps(x.write_bytes),
                        ]),
                        Err(why) => {
                            cells.extend([format!("未知：{}", why.label()), "—".into(), "—".into()])
                        }
                    }
                }
            }
        }
        line(&mut out, cells);
    }
    Reply::plain(200, out)
}
pub(super) fn storage(report: &storage::Report) -> Reply {
    use storage::{Capacity, Mounted, Observed, Role, Space};
    let sampled = match (report.started, report.ended) {
        (Some(a), Some(b)) if a <= b => format!("Unix 毫秒 {a} 至 {b}"),
        _ => "时间未知".into(),
    };
    let mut out = String::new();
    line(
        &mut out,
        [format!(
            "终端所选目录的内核 statx / fstatvfs；{sampled}。报告目录当时实际所在的文件系统，不证明预期磁盘已挂载。角色是终端标签，不表示已登记。容量不是块设备容量，不相加，也未核健康或可写权限。"
        )],
    );
    line(
        &mut out,
        [
            "终端选择",
            "挂载 ID",
            "挂载根",
            "只读标志",
            "文件系统总字节",
            "空闲字节",
            "非特权可用字节",
            "证据",
        ]
        .map(str::to_owned),
    );
    let unknown = |why: &dyn Display| format!("未知：{why}");
    for (role, result) in &report.rows {
        let role = match role {
            Role::System => "SYSTEM 候选".into(),
            Role::Recycle => "回收候选".into(),
            Role::Data(i) => format!("普通候选 {i}"),
        };
        let mut row = [
            role,
            "—".into(),
            "—".into(),
            "—".into(),
            "—".into(),
            "—".into(),
            "—".into(),
            String::new(),
        ];
        row[7] = match result {
            Err(why)
            | Ok(Observed {
                mount: Err(why), ..
            }) => unknown(why),
            Ok(Observed {
                mount: Ok(Mounted { mount, space }),
                ..
            }) => {
                row[1] = mount.id.to_string();
                row[2] = match mount.at_root {
                    Some(true) => "是",
                    Some(false) => "否",
                    None => "未知",
                }
                .into();
                match space {
                    Err(why) => unknown(why),
                    Ok(Space {
                        readonly_flag,
                        capacity,
                    }) => {
                        row[3] = if *readonly_flag {
                            "已设置"
                        } else {
                            "未设置"
                        }
                        .into();
                        match capacity {
                            None => "容量未知：内核计数不可用".into(),
                            Some(Capacity {
                                total,
                                free,
                                available,
                            }) => {
                                row[4] = total.to_string();
                                row[5] = free.to_string();
                                row[6] = available.to_string();
                                "当次采样".into()
                            }
                        }
                    }
                }
            }
        };
        line(&mut out, row);
    }
    Reply::plain(200, out)
}
pub(super) fn hardware(report: &hardware::Report) -> Reply {
    let mut out = String::new();
    let sampled = match (report.started_ms, report.ended_ms) {
        (Some(a), Some(b)) => format!("Unix 毫秒 {a} 至 {b}"),
        _ => "时间未知".into(),
    };
    line(
        &mut out,
        [format!(
            "Linux /proc/stat 与 /proc/meminfo；{sampled}。内核汇总，不代表 ZXS 进程或容器配额。"
        )],
    );
    line(&mut out, ["指标", "数值", "说明"].map(str::to_owned));
    let mut row =
        |name: &str, value: String, note: String| line(&mut out, [name.into(), value, note]);
    let unknown = |why: hardware::Unknown| format!("未知：{}", why.label());
    match report.cpu {
        Ok(c) => {
            let ms = c.interval_ns / 1_000_000;
            row(
                "CPU 执行",
                format!("{}%", c.active),
                format!("两次计数相隔 {ms} 毫秒"),
            );
            row(
                "I/O 等待（估计）",
                format!("{}%", c.iowait),
                "不是磁盘忙碌率".into(),
            );
            row(
                "虚拟化宿主占用的 CPU 时间",
                format!("{}%", c.steal),
                "内核 steal 计数占比".into(),
            );
        }
        Err(why) => row("CPU", unknown(why), "本次没有可比较的区间".into()),
    }
    match report.memory {
        Ok(m) => {
            row(
                "内存总量",
                m.total_bytes.to_string(),
                "字节，内核可管理的 RAM 总量".into(),
            );
            row(
                "内存可用（估计）",
                m.available_bytes.to_string(),
                "字节，含可回收空间，不是 MemFree".into(),
            );
        }
        Err(why) => row("内存", unknown(why), "本次没有可信的总量与可用量".into()),
    }
    Reply::plain(200, out)
}

pub(super) fn tools() -> Reply {
    let mut out = String::new();
    let report = fs_tools::Query::catalog().observe();
    line(&mut out,["固定位置的安装元数据，不执行工具、读取镜像或检查磁盘。程序存在不代表版本、签名、健康、可修复或已获授权。".into()]);
    line(
        &mut out,
        ["格式", "工具", "来源", "安装证据", "实际动作边界"].map(str::to_owned),
    );
    for row in report.rows {
        let p = row.profile;
        line(
            &mut out,
            [
                p.format.into(),
                p.tool.into(),
                (if p.private() {
                    "随包固定位置"
                } else {
                    "宿主固定位置（仅观察）"
                })
                .into(),
                row.installed.slot().into(),
                p.action().into(),
            ],
        );
    }
    Reply::plain(200, out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hardware_renders_only_the_typed_partial_report() {
        let report = hardware::Report {
            started_ms: Some(12),
            ended_ms: Some(34),
            cpu: Err(hardware::Unknown::CounterDecrease),
            memory: Ok(hardware::Memory {
                total_bytes: 8192,
                available_bytes: 0,
            }),
        };
        let reply = hardware(&report);
        assert_eq!(reply.code, 200);
        assert!(reply.body.contains("CPU\t未知：counter-decreased"));
        assert!(reply.body.contains("内存可用（估计）\t0\t"));
        let records: Vec<_> = reply.body.lines().collect();
        assert_eq!(records.len(), 5);
        assert!(records[1..].iter().all(|r| r.split('\t').count() == 3));
    }
    #[test]
    fn disk_attachment_labels_preserve_source_and_optional_failure() {
        let direct = disks::Attachment {
            subsystem: "nvme",
            source: disks::AttachmentSource::KernelAncestor,
            nvme_transport: Ok("pcie"),
        };
        assert_eq!(
            attachment_cells(Ok(direct)),
            ["nvme", "当次内核祖先", "pcie"]
        );
        let inferred = disks::Attachment {
            subsystem: "scsi",
            source: disks::AttachmentSource::InferredParent,
            nvme_transport: Err(disks::Reason::Unsupported),
        };
        assert_eq!(
            attachment_cells(Ok(inferred)),
            ["scsi", "由已核父盘推断", "未知：unsupported"]
        );
        assert_eq!(
            attachment_cells(Err(disks::Reason::NotReported)),
            ["未知：not-reported"; 3]
        );
    }
}
