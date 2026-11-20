//! A post-fragment observation of the genuine systemd effective unit. It is
//! deliberately not a guarantee against a separate concurrent host admin.
use super::{Id,bad,inactive_web_unit::UNIT};
use rustix::process::Pid;
use std::{collections::BTreeMap,io,path::Path};

pub(super) const FRAGMENT:&str="/etc/systemd/system/zxs-management-http.service";
pub(super) const CGROUP:&str="/system.slice/zxs-management-http.service";
const FIXED:&[(&str,&str)]=&[
    ("Id",UNIT),("Names",UNIT),("LoadState","loaded"),("NeedDaemonReload","no"),
    ("DropInPaths",""),("SourcePath",""),("FragmentPath",FRAGMENT),("Transient","no"),
    ("Type","exec"),("Restart","no"),("DynamicUser","no"),("NoNewPrivileges","yes"),
    ("Job",""),("ControlPID","0"),("RootDirectory",""),("RootImage",""),
    ("KillMode","control-group"),("PassEnvironment",""),("ExitType","main"),("Delegate","no"),
    ("RemainAfterExit","no"),("FreezerState","running"),
    ("OnSuccess",""),("OnFailure",""),("SuccessAction","none"),("FailureAction","none"),
    ("StartLimitAction","none"),("PropagatesStopTo",""),("ConsistsOf",""),
    ("Triggers",""),("TriggeredBy",""),("Wants",""),("Upholds",""),("BindsTo",""),("Requisite","sysinit.target"),
    ("RequiredBy",""),("BoundBy",""),("RequisiteOf",""),("UpheldBy",""),("ConflictedBy",""),
    ("Conflicts","shutdown.target"),("Slice","system.slice"),("DefaultDependencies","no"),
    ("StopWhenUnneeded","no"),("RefuseManualStart","no"),("RefuseManualStop","no"),
    ("JobTimeoutAction","none"),("PrivateNetwork","no"),("ProtectSystem","strict"),("ProtectHome","read-only"),
    ("PrivateDevices","yes"),("WorkingDirectory",""),("Requires","system.slice"),
];
const OPTIONAL_EMPTY:&[&str]=&[
    "ExecConditionEx","ExecStartPreEx","ExecStartPostEx","ExecReloadEx","ExecStopEx","ExecStopPostEx",
    "EnvironmentFiles","RestartForceExitStatus",
];
const VARY:&[&str]=&["Environment","ReadWritePaths","User","Group","ExecStartEx",
    "MainPID","ControlGroup","ActiveState","SubState","InvocationID","UnsetEnvironment"];
const EXEC_PREFIX:&str="{ path=/usr/bin/zxs ; argv[]=/usr/bin/zxs ; flags= ; start_time=[";
pub(super) const CAP:usize=24<<10;
fn no()->io::Error{bad("the fixed native Web systemd unit is not loaded with its exact approved configuration or stable state")}
pub(super) fn selectors()->String{
    FIXED.iter().map(|(k,_)|*k).chain(OPTIONAL_EMPTY.iter().copied()).chain(VARY.iter().copied())
        .collect::<Vec<_>>().join(",")
}
fn quoted(value:&str)->String{
    // v257.13 bus-print-properties calls shell_maybe_quote(...,0).
    // Compare its bounded text; never interpret or execute a shell expression.
    const ESC:&[char]=&['"','\\','$','\u{60}'];
    const QUOTE:&[char]=&[' ','*','?','[','\'','(',')','<','>','|','&',';','!'];
    if !value.chars().any(|c|ESC.contains(&c)||QUOTE.contains(&c)){return value.to_owned();}
    let mut out=String::from("\"");
    for c in value.chars(){if ESC.contains(&c){out.push('\\');}out.push(c);}
    out.push('"');out
}
fn accepts_exec(x:&str)->bool{
    let Some(body)=x.strip_prefix(EXEC_PREFIX)else{return false;};
    let Some((start,tail))=body.split_once("] ; stop_time=[")else{return false;};
    let Some((stop,tail))=tail.split_once("] ; pid=")else{return false;};
    let Some((pid,tail))=tail.split_once(" ; code=")else{return false;};
    let Some((code,status))=tail.split_once(" ; status=")else{return false;};
    let Some(status)=status.strip_suffix(" }")else{return false;};
    let stamp=|v:&str|!v.is_empty()&&v.len()<128&&!v.contains(['[',']','{','}',';','\r','\n']);
    stamp(start)&&stamp(stop)&&pid.parse::<i32>().is_ok_and(|x|x>=0)
        &&code.len()<33&&status.len()<65&&!code.contains(['{','}',';'])&&!status.contains(['{','}',';'])
}
#[derive(Clone,Copy,PartialEq,Eq,Debug)]
pub(super) enum State{Inactive,Draining,Running{pid:Pid,invocation:Id}}
pub(super) fn read(data:&[u8],system:&Path,pool:&Id,uid:u32,gid:u32)->io::Result<State>{
    if data.len()>CAP{return Err(no());}
    let text=std::str::from_utf8(data).map_err(|_|no())?.strip_suffix('\n').ok_or_else(no)?;
    let mut kv=BTreeMap::new();
    for line in text.split('\n'){
        let (k,v)=line.split_once('=').ok_or_else(no)?;
        if !VARY.contains(&k)&&!OPTIONAL_EMPTY.contains(&k)&&!FIXED.iter().any(|(a,_)|*a==k){return Err(no());}
        if kv.insert(k,v).is_some(){return Err(no());}
    }
    let get=|k:&str|kv.get(k).copied();
    if FIXED.iter().any(|(k,v)|get(k)!=Some(*v))
        ||OPTIONAL_EMPTY.iter().any(|k|get(k).is_some_and(|v|!v.is_empty())){return Err(no());}
    // The native unit may require the existing normal service slice. Its
    // sysinit dependency is Requisite (verify-active, never pull-in).
    if !matches!(get("UnsetEnvironment"),Some("ZXS_WEB_PUBLIC_ORIGIN ZXS_WEB_TRUSTED_PROXY"|"ZXS_WEB_TRUSTED_PROXY ZXS_WEB_PUBLIC_ORIGIN")){
        return Err(no());
    }
    let original=system.to_str().ok_or_else(no)?;
    let pool=pool.iter().map(|b|format!("{b:02x}")).collect::<String>();
    let path_var=quoted(&format!("ZXS_SYSTEM_ROOT={original}"));let pool_var=format!("ZXS_POOL_ID={pool}");
    let expected=format!("{path_var} {pool_var}");let other_order=format!("{pool_var} {path_var}");
    if !matches!(get("Environment"),Some(v) if v==expected || v==other_order)
        || get("ReadWritePaths")!=Some(quoted(original).as_str())
        ||get("User")!=Some(uid.to_string().as_str())||get("Group")!=Some(gid.to_string().as_str())
        ||!get("ExecStartEx").is_some_and(accepts_exec){return Err(no());}
    match (get("ActiveState"),get("SubState"),get("MainPID"),get("ControlGroup")){
        (Some("inactive"),Some("dead"),Some("0"),Some(""|CGROUP))=>Ok(State::Inactive),
        (Some("deactivating"),Some("stop"|"stop-sigterm"|"stop-sigkill"|"stop-post"|"final-sigterm"|"final-sigkill"),_,Some(""|CGROUP))
            =>Ok(State::Draining),
        (Some("active"),Some("running"),Some(pid),Some(CGROUP))=>{
            let pid=pid.parse::<i32>().ok().and_then(Pid::from_raw).filter(|p|p.as_raw_pid()>1).ok_or_else(no)?;
            let invocation=crate::sqlite_install::id(get("InvocationID").ok_or_else(no)?.into()).ok_or_else(no)?;
            Ok(State::Running{pid,invocation})
        }
        _=>Err(no()),
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    fn properties()->String{
        let mut out=FIXED.iter().map(|(k,v)|format!("{k}={v}\n")).collect::<String>();
        out.push_str("Environment=\"ZXS_SYSTEM_ROOT=/srv/磁盘 A\" ZXS_POOL_ID=01010101010101010101010101010101\nReadWritePaths=\"/srv/磁盘 A\"\nUser=1000\nGroup=1001\nActiveState=inactive\nSubState=dead\nMainPID=0\nControlGroup=\nInvocationID=\nUnsetEnvironment=ZXS_WEB_PUBLIC_ORIGIN ZXS_WEB_TRUSTED_PROXY\n");
        out.push_str("ExecStartEx={ path=/usr/bin/zxs ; argv[]=/usr/bin/zxs ; flags= ; start_time=[n/a] ; stop_time=[n/a] ; pid=0 ; code=(null) ; status=0/ }\n");
        out
    }
    #[test]
    fn effective_unit_and_exact_pool_are_distinct_from_an_installed_file(){
        let data=properties();let check=|s:&str|read(s.as_bytes(),Path::new("/srv/磁盘 A"),&[1;16],1000,1001);
        assert_eq!(check(&data).unwrap(),State::Inactive);
        for altered in [data.replace("NeedDaemonReload=no","NeedDaemonReload=yes"),data.replace("DropInPaths=\n","DropInPaths=/run/a.conf\n"),
            data.replace("argv[]=/usr/bin/zxs","argv[]=/usr/bin/zxs --extra"),
            data.replace("flags= ;","flags=full-privileges ;"),
            data.replace("ZXS_SYSTEM_ROOT=/srv/磁盘 A","ZXS_SYSTEM_ROOT=/srv/another"),
            data.clone()+"ExecStopPostEx={ path=/other }\n",data.replace("Job=\n","Job=9\n"),
            data.replace("User=1000","User=0"),data.replace("ActiveState=inactive","ActiveState=active"),
            data.replace("Wants=\n","Wants=unrelated.service\n"),data.replace("Requires=system.slice","Requires=unrelated.service sysinit.target"),
            data.replace("UnsetEnvironment=ZXS_WEB_PUBLIC_ORIGIN ZXS_WEB_TRUSTED_PROXY","UnsetEnvironment="),data.clone()+"RestartForceExitStatus=0\n",
            data.replace("RequiredBy=\n","RequiredBy=unrelated.service\n")]{
            assert!(check(&altered).is_err());
        }
        let active=data.replace("ActiveState=inactive","ActiveState=active").replace("SubState=dead","SubState=running")
            .replace("MainPID=0","MainPID=123").replace("ControlGroup=\n",&format!("ControlGroup={CGROUP}\n"))
            .replace("InvocationID=\n",&format!("InvocationID={}\n","01".repeat(16)));
        assert_eq!(check(&active).unwrap(),State::Running{pid:Pid::from_raw(123).unwrap(),invocation:[1;16]});
    }
}
