//! Pure rendering of the explicitly approved first SMB site. No provision,
//! network probe, pidfd, database, account operation, or command selects this.
pub(crate) mod first;

use first::{Endpoint, Plan};
use std::{fmt::Write, net::{IpAddr, Ipv4Addr, Ipv6Addr}};

const BASE: &str = include_str!("../resources/samba-domain/initial-live.conf");
const LOOPBACK: &str = "    interfaces = 127.0.0.1\n";
const PREFIX: &str = "    zxs:first plan = ";
pub(crate) const LIVE_MAX: usize = 64 << 10;
const BAD: &str = "not the exact single-tree configuration from the approved first plan";

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for x in bytes { write!(out, "{x:02x}").expect("writing a String"); }
    out
}
fn addresses(e: Endpoint) -> (String, String) {
    let n = e.prefix();
    match e.address() {
        IpAddr::V4(a) => {
            let mask = u32::MAX << (32 - n);
            let network = Ipv4Addr::from(u32::from(a) & mask);
            (format!("{a}/255.255.255.255"), format!("{network}/{}", Ipv4Addr::from(mask)))
        }
        IpAddr::V6(a) => {
            let mask = u128::MAX << (128 - n);
            let network = Ipv6Addr::from(u128::from(a) & mask);
            (format!("{a}/ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff"), format!("{network}/{}", Ipv6Addr::from(mask)))
        }
    }
}

pub(crate) fn render(plan: &Plan) -> Result<String, &'static str> {
    let (head, tail) = BASE.split_once(LOOPBACK).ok_or(BAD)?;
    let mut cfg = head.to_owned();
    writeln!(cfg, "# ZXS initial share plan SHA256: {}", hex(&plan.sha())).expect("writing a String");
    let (bind, allow): (Vec<_>, Vec<_>) = plan.endpoints().iter().copied().map(addresses).unzip();
    // Full host masks for Samba's interface selection, not peer CIDRs. Peer
    // masks are only a supplemental global allow list; the native service
    // must still enforce each approved local-address/peer-prefix pairing.
    writeln!(cfg, "    interfaces = {}", bind.join(" ")).expect("writing a String");
    writeln!(cfg, "    hosts allow = {}", allow.join(" ")).expect("writing a String");
    cfg.push_str("    hosts deny = ALL\n    hostname lookups = no\n    change notify = no\n    unix charset = UTF-8\n");
    cfg.push_str("    enable core files = no\n    panic action =\n    logging = file\n    log file = /dev/null\n    log level = 0\n    debug encryption = no\n");
    cfg.push_str("    locking = yes\n    strict locking = yes\n    posix locking = no\n");
    cfg.push_str(tail);
    cfg.push_str("\n[ZXS]\n    zxs:share role = namespace\n");
    writeln!(cfg, "{PREFIX}{}", hex(plan.wire())).expect("writing a String");
    // /zxs-root is not a host path or a source projection. read only=no
    // permits per-folder SQL restrictions; it never grants an operation.
    cfg.push_str("    path = /zxs-root\n    browseable = no\n    case sensitive = yes\n    kernel oplocks = no\n    vfs objects = zxs_file\n    read only = no\n");
    if cfg.len() > LIVE_MAX { return Err(BAD); }
    Ok(cfg)
}
/// For the verified foreground only. Show the exact original scope without
/// letting a name emit an escape, bidi formatting or line separator.
pub(crate) fn approval_text(plan: &Plan) -> String {
    let mut out = format!(r"唯一共享 \\服务器\ZXS；首装计划 SHA-256 {}。", hex(&plan.sha()));
    out.push('\n');
    for e in plan.endpoints() {
        writeln!(out, "  监听 {} TCP 445；许可客户前缀 {}/{}。", e.address(), e.address(), e.prefix()).expect("writing a String");
    }
    for (f, _) in plan.folders() {
        let mode = match f.mode { first::Mode::ReadOnly => "只读", first::Mode::ReadWrite => "读写" };
        out.push_str("  一级目录 \"");
        escaped(&mut out,f.name.as_bytes());
        writeln!(out, "\"；原数据卷 {}；初始{}。", hex(&f.volume), mode).expect("writing a String");
    }
    out.push_str("  逻辑回收站 \"");
    escaped(&mut out,plan.recycle().as_str().as_bytes());
    out.push_str("\"；仅使用原 INIT 的独立回收盘。\n");
    out
}
pub(crate) fn escaped(out:&mut String,name:&[u8]){
    match std::str::from_utf8(name) {
        Ok(name)=>for c in name.chars(){
            if c.is_control() || matches!(c, '\u{061c}'|'\u{200b}'..='\u{200f}'|'\u{2028}'..='\u{202e}'|'\u{2060}'..='\u{206f}'|'\u{feff}'){
                write!(out, "\\u{{{:x}}}", c as u32).expect("writing a String");
            } else {
                if matches!(c,'\\'|'"'){out.push('\\');}
                out.push(c);
            }
        },
        Err(_)=>out.extend(name.iter().copied().flat_map(std::ascii::escape_default).map(char::from)),
    }
}

pub(crate) fn exact_plan(config: &[u8]) -> Result<Plan, &'static str> {
    if config.len() > LIVE_MAX { return Err(BAD); }
    let s = std::str::from_utf8(config).map_err(|_| BAD)?;
    let mut selected = s.lines().filter_map(|l| l.strip_prefix(PREFIX));
    let p = selected.next().ok_or(BAD)?;
    if p.is_empty() || p.len() % 2 != 0 || p.len() > 2 * first::MAX_WIRE || selected.next().is_some()
        || !p.bytes().all(|v| v.is_ascii_digit() || matches!(v, b'a'..=b'f')) { return Err(BAD); }
    let wire = p.as_bytes().chunks_exact(2).map(|v| {
        let s = std::str::from_utf8(v).expect("checked ASCII");
        u8::from_str_radix(s, 16).map_err(|_| BAD)
    }).collect::<Result<Vec<_>, _>>()?;
    let plan = Plan::decode(&wire)?;
    if render(&plan)?.as_bytes() != config { return Err(BAD); }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use first::{Folder, Mode, Origin, RecycleName};

    fn plan() -> Plan {
        Plan::selected(Origin { pool: [1; 16], init: [2; 16], instance: [3; 16], domain: [4; 16], setup: [5; 16] },
            vec![Endpoint::named("2001:db8:7::2/64").unwrap(), Endpoint::named("192.0.2.17/24").unwrap()],
            vec![Folder { volume: [0x22; 16], name: "docs".into(), mode: Mode::ReadWrite },
                 Folder { volume: [0x21; 16], name: "Docs".into(), mode: Mode::ReadOnly }],
            RecycleName::new("回收站".into()).unwrap()).unwrap()
    }
    fn vector(key: &str) -> &str {
        include_str!("../tests/smb-host/first-ordinary-plan-v1.txt").lines()
            .filter_map(|l| l.split_once('=')).find(|(k, _)| *k == key).expect("fixed public vector").1
    }

    #[test]
    fn canonical_plan_and_initial_ids_match_the_public_nonsecret_bytes() {
        let p = plan();
        assert_eq!(p.wire().len().to_string(), vector("bytes"));
        assert_eq!(hex(p.wire()), vector("wire_hex"));
        assert_eq!(hex(&p.sha()), vector("plan_sha256"));
        for (i, (_, k)) in p.folders().iter().enumerate() {
            assert_eq!(hex(&k.share), vector(if i == 0 { "f1_share" } else { "f2_share" }));
            assert_eq!(hex(&k.publish), vector(if i == 0 { "f1_publish" } else { "f2_publish" }));
            assert_eq!(hex(&k.setting), vector(if i == 0 { "f1_mode" } else { "f2_mode" }));
        }
        assert_eq!(p.recycle().as_str(), "回收站");
        let at=p.wire().len()-2-p.recycle().as_str().len();
        let mut old=p.wire()[..at].to_vec();old.extend([0,0]);
        assert!(Plan::decode(&old).is_err());
        let mut duplicate=p.wire()[..at].to_vec();duplicate.extend(b"\0\x04Docs");
        assert!(Plan::decode(&duplicate).is_err());
        let mut different_case=p.wire()[..at].to_vec();different_case.extend(b"\0\x04DOCS");
        assert_eq!(Plan::decode(&different_case).unwrap().recycle().as_str(), "DOCS");
        assert!(Plan::decode(&p.wire()[..p.wire().len()-1]).is_err());
        let mut extra = p.wire().to_vec();
        extra.push(0);
        assert!(Plan::decode(&extra).is_err());
        let mut shuffled = p.wire().to_vec();
        let at = 16 + 5 * 16 + 1;
        let mut reordered = shuffled[at + 6..at + 24].to_vec();
        reordered.extend_from_slice(&shuffled[at..at + 6]);
        shuffled[at..at + 24].copy_from_slice(&reordered);
        assert!(Plan::decode(&shuffled).is_err());
    }

    #[test]
    fn only_the_canonical_single_business_service_can_represent_the_plan() {
        let p = plan();
        let cfg = render(&p).unwrap();
        assert_eq!(cfg.lines().filter(|s| s.starts_with('[')).collect::<Vec<_>>(), ["[global]", "[ZXS]"]);
        assert_eq!(exact_plan(cfg.as_bytes()).unwrap(), p);
        assert!(exact_plan(BASE.as_bytes()).is_err());
        for other in [cfg.replace("server max protocol = SMB3_11", "server max protocol = SMB3_02"),
            cfg.replace("    path = /zxs-root", "    path = /mnt/actual"),
            cfg.replace("    hostname lookups = no", "    hostname lookups = yes"),
            format!("{cfg}\n[docs]\n    path = /zxs-root\n")] {
            assert!(exact_plan(other.as_bytes()).is_err());
        }
    }
}
