//! Private one-shot pipe, not an HTTP format. Both ends already own the target
//! digest and framing. A valid receipt for an incomplete check is not a clean result.
use super::{Framing, Outcome, Report, valid_counts};
use crate::pinned::invalid;
use std::{fmt::Write as _, io};
impl Report {
    pub(in crate::volume_images) const WIRE_SIZE: usize = 57;
    pub(in crate::volume_images) fn packet(&self, digest: &[u8; 32]) -> [u8; 57] {
        let mut out = [0; 57];
        out[..4].copy_from_slice(b"ECR1");
        out[4] = match self.state {
            Outcome::NoFindings => 0,
            Outcome::VolumeFlags => 1,
            Outcome::ExtraDiagnostics => 2,
            Outcome::Uncorrected => 3,
            Outcome::IncompleteUnresolved => 4,
        };
        out[5..37].copy_from_slice(digest);
        out[37..45].copy_from_slice(&self.frame.length.to_be_bytes());
        out[45..47].copy_from_slice(&self.frame.sector.to_be_bytes());
        out[47..49].copy_from_slice(&self.frame.flags.to_be_bytes());
        let (dirs, files) = self.counts.unwrap_or((0, 0));
        out[49..53].copy_from_slice(&dirs.to_be_bytes());
        out[53..57].copy_from_slice(&files.to_be_bytes());
        out
    }
    pub(in crate::volume_images) fn from_packet(
        p: &[u8],
        exit: Option<i32>,
        frame: Framing,
        digest: &[u8; 32],
    ) -> io::Result<Self> {
        let bad = || invalid("no matching complete receipt from the selected exFAT worker");
        if p.len() != Self::WIRE_SIZE
            || &p[..4] != b"ECR1"
            || &p[5..37] != digest
            || p[37..45] != frame.length.to_be_bytes()
            || p[45..47] != frame.sector.to_be_bytes()
            || p[47..49] != frame.flags.to_be_bytes()
        {
            return Err(bad());
        }
        let d = u32::from_be_bytes(p[49..53].try_into().expect("four bytes"));
        let f = u32::from_be_bytes(p[53..57].try_into().expect("four bytes"));
        let state = match (p[4], exit) {
            (0, Some(0)) if frame.flags & 6 == 0 => Outcome::NoFindings,
            (1, Some(1)) if frame.flags & 6 != 0 => Outcome::VolumeFlags,
            (2, Some(2)) => Outcome::ExtraDiagnostics,
            (3, Some(1)) => Outcome::Uncorrected,
            (4, Some(2)) if (d, f) == (0, 0) => {
                return Ok(Self {
                    state: Outcome::IncompleteUnresolved,
                    frame,
                    counts: None,
                });
            }
            _ => return Err(bad()),
        };
        if !valid_counts(d, f, frame) {
            return Err(bad());
        }
        Ok(Self {
            state,
            frame,
            counts: Some((d, f)),
        })
    }
    pub(in crate::volume_images) fn panel_text(&self) -> String {
        let msg = match self.state {
            Outcome::NoFindings => {
                "fsck.exfat 本次只读检查未报告问题；并非正文、备用引导区或整盘健康证明"
            }
            Outcome::VolumeFlags => "工具返回 0，但卷状态需注意，不能判为正常",
            Outcome::ExtraDiagnostics => "未能确认：工具返回 0，但还有额外诊断，不能判为正常",
            Outcome::Uncorrected => "工具报告有未修复的问题（退出 4），不能保证所有目录均已检查",
            Outcome::IncompleteUnresolved => {
                "未能确认：工具退出 4，未得到一致的未修复统计，可能提前终止"
            }
        };
        let mut s = format!(
            "exFAT：{msg}。卷脏标记：{}；介质失败标记：{}。",
            if self.frame.flags & 2 == 0 {
                "未置位"
            } else {
                "已置位"
            },
            if self.frame.flags & 4 == 0 {
                "未置位"
            } else {
                "已置位"
            }
        );
        if let Some((d, f)) = self.counts {
            write!(s, "上游报告目录数：{d}（含根）；文件数：{f}。").expect("writing to String");
        }
        s.push_str("只检查这份封存副本，未修复。");
        s
    }
}
