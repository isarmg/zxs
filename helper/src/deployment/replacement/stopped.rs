//! Read only fixed unit status via the held same-program systemctl. Never ask
//! systemd for Exec*, environment, command lines, a config value or unit text.
use crate::{Result, system_program::{Executable, Program}};
use data_serialization::Serialize;
use std::collections::BTreeMap;

const PROPERTIES: &str = "--property=LoadState,ActiveState,SubState,MainPID,ControlPID,ControlGroup,Job,NeedDaemonReload,Transient,FragmentPath,DropInPaths,TriggeredBy";
const KEYS: [&str; 12] = ["LoadState","ActiveState","SubState","MainPID","ControlPID","ControlGroup","Job","NeedDaemonReload","Transient","FragmentPath","DropInPaths","TriggeredBy"];
const ZXS: &str = "zxs.service";
const BASE: [&str; 2] = [ZXS, "zxs-smb.service"];
const SAMBA: [&str; 6] = ["smbd.service", "smb.service", "samba.service", "samba-ad-dc.service", "nmbd.service", "nmb.service"];

#[derive(Clone, Copy, Eq, PartialEq, Serialize)]
#[serialization(rename_all = "snake_case")]
enum State { Stopped, NotInstalled }
#[derive(Eq, PartialEq, Serialize)]
struct Observation { unit: &'static str, state: State }
pub(super) struct Stopped {
    systemctl: Executable,
    include_samba: bool,
    observations: Vec<Observation>,
}
fn invalid() -> Box<dyn std::error::Error + Send + Sync> {
    "cannot prove the exact fixed systemd service is inactive with no process, job or pending reload; nothing was replaced".into()
}
/// Returned text is untrusted; errors report only this bounded state class.
fn admit(unit: &'static str, code: Option<i32>, output: &[u8]) -> Result<Observation> {
    if output.len() > 2048 || !output.is_ascii() { return Err(invalid()); }
    let text = std::str::from_utf8(output).map_err(|_| invalid())?;
    let mut values = BTreeMap::new();
    for line in text.lines() {
        let (key, value) = line.split_once('=').ok_or_else(invalid)?;
        if !KEYS.contains(&key) || values.insert(key, value).is_some() { return Err(invalid()); }
    }
    let get = |key: &str| values.get(key).copied();
    if values.len() != KEYS.len() || get("ActiveState") != Some("inactive")
        || get("SubState") != Some("dead") || get("MainPID") != Some("0") || get("ControlPID") != Some("0")
        || get("Job") != Some("") || get("ControlGroup") != Some("")
        || get("NeedDaemonReload") != Some("no") || get("Transient") != Some("no")
        || get("DropInPaths") != Some("") || get("TriggeredBy") != Some("")
    { return Err(invalid()); }
    let state = match (get("LoadState"), code) {
        (Some("loaded"), Some(0)) => {
            let fragment = get("FragmentPath").ok_or_else(invalid)?;
            if fragment.is_empty() || (unit == ZXS && fragment != crate::private_io::SYSTEMD_PATH)
                || (unit == "zxs-smb.service" && fragment != "/etc/systemd/system/zxs-smb.service")
            { return Err(invalid()); }
            State::Stopped
        }
        (Some("not-found"), Some(0 | 4)) if unit != ZXS && get("FragmentPath") == Some("") => State::NotInstalled,
        _ => return Err(invalid()),
    };
    Ok(Observation { unit, state })
}
fn observe(program: &Executable, include_samba: bool) -> Result<Vec<Observation>> {
    let mut result = Vec::with_capacity(BASE.len() + if include_samba { SAMBA.len() } else { 0 });
    for name in BASE.into_iter().chain(SAMBA.into_iter().filter(|_| include_samba)) {
        let (status, bytes) = program.capture_status(&["--no-pager", "show", "--all", PROPERTIES, "--", name])?;
        result.push(admit(name, status.code(), &bytes)?);
    }
    Ok(result)
}
impl Stopped {
    pub(super) fn capture(include_samba: bool) -> Result<Self> {
        let systemctl = Executable::open(Program::Systemctl)?;
        let observations = observe(&systemctl, include_samba)?;
        Ok(Self { systemctl, include_samba, observations })
    }
    pub(super) fn recheck(&self) -> Result<()> {
        self.systemctl.recheck()?;
        if observe(&self.systemctl, self.include_samba)? != self.observations { return Err(invalid()); }
        self.systemctl.recheck()
    }
    pub(super) fn plan(&self) -> json_codec::Value {
        json_codec::json!({"systemctl":self.systemctl.snapshot(),"units":self.observations})
    }
}

#[cfg(test)]
#[path = "../../../../test/helper/unit/replacement_stopped.rs"]
mod tests;
