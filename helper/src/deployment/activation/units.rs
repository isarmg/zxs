use crate::{Result, system_program::Executable};
use data_serialization::Serialize;
use std::collections::BTreeMap;

pub(super) const ZXS: &str = "zxs.service";
pub(super) const SMB: &str = "zxs-smb.service";
pub(super) const SMB_PATH: &str = "/etc/systemd/system/zxs-smb.service";
pub(super) const SMB_BYTES: &str = include_str!("../../../deploy/zxs-smb.service.in");

#[derive(Serialize)]
pub(super) struct Unit {
    pub name: String,
    pub properties: BTreeMap<String, String>,
}
impl Unit {
    fn value(&self, name: &str) -> Result<&str> {
        self.properties
            .get(name)
            .map(String::as_str)
            .ok_or_else(|| format!("systemd omitted required property {name}").into())
    }
    pub fn absent(&self) -> Result<bool> {
        Ok(self.value("LoadState")? == "not-found")
    }
    pub fn active(&self) -> Result<bool> {
        Ok(!self.absent()?
            && self.value("ActiveState")? == "active"
            && self.value("SubState")? == "running"
            && self.value("MainPID")?.parse::<u32>()? > 0)
    }
    pub fn enabled(&self) -> Result<bool> {
        Ok(!self.absent()? && self.value("UnitFileState")? == "enabled")
    }
    pub fn may_start(&self) -> Result<bool> {
        Ok(self.absent()? || matches!(self.value("ActiveState")?, "inactive" | "failed"))
    }
    pub fn require_stopped_for_vfs(&self) -> Result<()> {
        if !self.absent()? && (!self.may_start()? || self.value("MainPID")?.parse::<u32>()? != 0) {
            return Err(format!("{} is active or changing; stop it explicitly before VFS installation, no service was stopped by this helper", self.name).into());
        }
        Ok(())
    }
    pub fn require_loaded(&self, path: &str, command: &str, user: &str, group: &str) -> Result<()> {
        let exec = self.value("ExecStart")?;
        let argv = exec
            .split(" ; ")
            .find_map(|field| field.strip_prefix("argv[]="));
        if self.value("LoadState")? != "loaded"
            || self.value("FragmentPath")? != path
            || !self.value("DropInPaths")?.is_empty()
            || self.value("Transient")? != "no"
            || self.value("NeedDaemonReload")? != "no"
            || self.value("User")? != user
            || self.value("Group")? != group
            || argv != Some(command)
            || !exec.starts_with(&format!(
                "{{ path={} ; ",
                command.split(' ').next().unwrap_or("")
            ))
            || exec.matches("argv[]=").count() != 1
            || [
                "ExecStartPre",
                "ExecStartPost",
                "ExecCondition",
                "ExecStop",
                "ExecStopPost",
                "ExecReload",
            ]
            .iter()
            .any(|key| {
                self.properties
                    .get(*key)
                    .is_none_or(|value| !value.is_empty())
            })
        {
            return Err(format!("{} is not the exact loaded helper unit, or has overrides/pending reload; no different service was started or stopped", self.name).into());
        }
        Ok(())
    }
}
pub(super) fn read(program: &Executable, name: &'static str) -> Result<Unit> {
    let (status, bytes) = program.capture_status(&[
        "--no-pager", "show", "--all",
        "--property=LoadState,ActiveState,SubState,MainPID,UnitFileState,FragmentPath,DropInPaths,Transient,NeedDaemonReload,User,Group,ExecStart,ExecStartPre,ExecStartPost,ExecCondition,ExecStop,ExecStopPost,ExecReload",
        "--", name,
    ])?;
    let text = std::str::from_utf8(&bytes)?;
    let mut properties = BTreeMap::new();
    for line in text.lines() {
        let (key, value) = line
            .split_once('=')
            .ok_or("malformed systemd unit observation")?;
        if properties.insert(key.into(), value.into()).is_some() {
            return Err("duplicate property in systemd unit observation".into());
        }
    }
    let result = Unit {
        name: name.into(),
        properties,
    };
    if !status.success() && !result.absent()? {
        return Err("systemd unit observation failed; no activation outcome was inferred".into());
    }
    Ok(result)
}
pub(super) fn conflict(program: &Executable, name: &'static str) -> Result<()> {
    let unit = read(program, name)?;
    if !unit.absent()? && !unit.may_start()? {
        return Err(format!("external {name} is active or changing; stop it explicitly before activating the separate ZXS Samba share").into());
    }
    Ok(())
}
