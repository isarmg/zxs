//! Explicit Debian Samba dependency installation. APT owns package signatures,
//! dependency resolution, dpkg locking and maintainer-script execution.
mod policy;
use crate::{
    Result,
    private_io::{BundleDestination, BundlePublication},
    system_program::{Executable, Program, Snapshot},
};
use data_serialization::{Deserialize, Serialize};
use platform_io::{
    fs_safety::{
        RelativePath,
        linux::{MountPolicy, OpenAt2Root},
    },
    service_lease::HOST_ROOT,
};
use sha2::{Digest, Sha256};
use std::{io::Read, path::Path};

const ORIGINAL: &str = ".zxs-helper-system-packages";
const COMPLETE: &str = ".zxs-helper-system-packages-complete";
const LIMIT: usize = 64 * 1024;
const PACKAGES: [&str; 3] = ["samba", "samba-common-bin", "systemd"];
#[derive(Deserialize)]
struct Target {
    package_version: String,
}
fn version() -> Result<String> {
    let target: Target =
        json_codec::from_str(include_str!("../../../.github/samba-debian-target.json"))?;
    Ok(target.package_version)
}
#[derive(Serialize, Deserialize)]
enum Format {
    #[serialization(rename = "zxs-helper-debian-samba-packages")]
    DebianSamba,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Plan {
    format: Format,
    package_version: String,
    host_device: String,
    host_inode: String,
    apt: Snapshot,
    simulation: String,
    new_samba: bool,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Receipt {
    request_digest: String,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Commit {
    pub request_digest: String,
    pub confirmation: String,
    pub external_administration_stopped: bool,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Refresh {
    pub confirmation: String,
    pub external_administration_stopped: bool,
}
#[derive(Serialize)]
#[serialization(tag = "state", rename_all = "snake_case")]
pub(crate) enum Observation {
    RequiredPackagesInstalled {
        package_version: String,
    },
    Installable {
        request_digest: String,
        plan: Plan,
    },
    OriginalPending {
        request_digest: String,
        packages_installed: bool,
        simulation: String,
    },
    Completed {
        request_digest: String,
        package_version: String,
    },
}
fn host() -> Result<BundleDestination> {
    if rustix::process::getuid().as_raw() != 0 || rustix::process::geteuid().as_raw() != 0 {
        return Err("system package installation requires the manually run root helper".into());
    }
    let host = BundleDestination::open(Path::new(HOST_ROOT))?;
    let root = OpenAt2Root::open(Path::new("/usr/lib"), MountPolicy::AllowNestedMounts)?;
    let file = root.open_file(&RelativePath::new("os-release")?, false)?;
    let mut release = String::new();
    file.take(4097).read_to_string(&mut release)?;
    if release.len() > 4096 || !debian_13(&release) || std::env::consts::ARCH != "x86_64" {
        return Err("the package installer supports only Debian 13 amd64; no source or platform settings were changed".into());
    }
    Ok(host)
}
fn debian_13(release: &str) -> bool {
    let values = |key: &str| {
        release
            .lines()
            .filter_map(move |line| line.strip_prefix(key))
            .collect::<Vec<_>>()
    };
    matches!(values("ID=").as_slice(), ["debian"] | ["\"debian\""])
        && matches!(values("VERSION_ID=").as_slice(), ["13"] | ["\"13\""])
}
fn installed(version: &str) -> Result<[bool; 3]> {
    let query = Executable::open(Program::DpkgQuery)?;
    let (status, bytes) = query.capture_status(&[
        "-W",
        "-f=${Package}\t${Status}\t${Version}\t${Architecture}\n",
        PACKAGES[0],
        PACKAGES[1],
        PACKAGES[2],
    ])?;
    if !status.success() && status.code() != Some(1) {
        return Err(
            "dpkg-query could not inspect package state; no installation was started".into(),
        );
    }
    package_rows(std::str::from_utf8(&bytes)?, version)
}
fn package_rows(text: &str, version: &str) -> Result<[bool; 3]> {
    let mut result = [false; 3];
    for line in text.lines() {
        let fields = line.split('\t').collect::<Vec<_>>();
        let [name, status, actual, architecture] = fields.as_slice() else {
            return Err("unexpected dpkg-query package record".into());
        };
        let index = PACKAGES
            .iter()
            .position(|package| package == name)
            .ok_or("unexpected package in dpkg-query result")?;
        if result[index]
            || *status != "install ok installed"
            || (index < 2 && *actual != version)
            || actual.is_empty()
            || *architecture != "amd64"
        {
            return Err("existing dependency package is partial or Samba differs from the required version; inspect/upgrade it outside this first-install operation".into());
        }
        result[index] = true;
    }
    Ok(result)
}
fn apt_args(version: &str, simulate: bool) -> Vec<String> {
    let mut args = vec![
        "--assume-yes",
        "--no-remove",
        "--no-upgrade",
        "--no-install-recommends",
        "-o",
        "APT::Get::AllowUnauthenticated=false",
        "-o",
        "Acquire::AllowInsecureRepositories=false",
        "-o",
        "Acquire::AllowDowngradeToInsecureRepositories=false",
        "-o",
        "Dpkg::Use-Pty=0",
        "-o",
        "DPkg::Options::=--force-confold",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    if simulate {
        args.push("--simulate".into());
    }
    args.push("install".into());
    args.extend(PACKAGES[..2].iter().map(|name| format!("{name}={version}")));
    args.push("systemd".into());
    args
}
fn digest(bytes: &[u8]) -> String {
    platform_io::lower_hex(&Sha256::digest(bytes))
}
struct PreparedPlan {
    plan: Plan,
    bytes: Vec<u8>,
    digest: String,
}
fn original(host: &BundleDestination) -> Result<Option<(Plan, String)>> {
    let Some(bytes) = host.read_operation(ORIGINAL, "plan.json", LIMIT)? else {
        return Ok(None);
    };
    let plan: Plan = json_codec::from_slice(&bytes)?;
    if plan.package_version != version()?
        || plan.host_device != host.identity().device.to_string()
        || plan.host_inode != host.identity().inode.to_string()
    {
        return Err(
            "package request belongs to another host or supported target; it was not replaced"
                .into(),
        );
    }
    let digest = digest(&bytes);
    Ok(Some((plan, digest)))
}
fn observe(host: &BundleDestination, plan: Plan, digest: String) -> Result<Observation> {
    let ready = installed(&plan.package_version)? == [true; 3];
    if let Some(bytes) = host.read_operation(COMPLETE, "receipt.json", LIMIT)? {
        let receipt: Receipt = json_codec::from_slice(&bytes)?;
        if receipt.request_digest != digest || !ready {
            return Err("completed system packages no longer match their original receipt; nothing was reinstalled".into());
        }
        return Ok(Observation::Completed {
            request_digest: digest,
            package_version: plan.package_version,
        });
    }
    Ok(Observation::OriginalPending {
        request_digest: digest,
        packages_installed: ready,
        simulation: plan.simulation,
    })
}
fn fresh(
    host: &BundleDestination,
    version: String,
    apt: &Executable,
    new_samba: bool,
) -> Result<PreparedPlan> {
    if new_samba {
        require_unclaimed_units()?;
    }
    policy::Policy::open()?.absent_or_original(None)?;
    let args = apt_args(&version, true);
    let simulation =
        String::from_utf8(apt.capture(&args.iter().map(String::as_str).collect::<Vec<_>>())?)?;
    let plan = Plan {
        format: Format::DebianSamba,
        package_version: version,
        host_device: host.identity().device.to_string(),
        host_inode: host.identity().inode.to_string(),
        apt: apt.snapshot(),
        simulation,
        new_samba,
    };
    let bytes = json_codec::to_vec(&plan)?;
    let digest = digest(&bytes);
    Ok(PreparedPlan {
        plan,
        bytes,
        digest,
    })
}
fn require_unclaimed_units() -> Result<()> {
    for directory in [
        "/etc/systemd/system",
        "/run/systemd/system",
        "/usr/lib/systemd/system",
        "/etc/systemd/system/multi-user.target.wants",
    ] {
        for name in ["smbd.service", "nmbd.service"] {
            match std::fs::symlink_metadata(Path::new(directory).join(name)) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
                Ok(_) => return Err("an existing Samba unit or enablement belongs to the administrator; package installation will not take it over".into()),
            }
        }
    }
    Ok(())
}
pub(crate) fn preview() -> Result<Observation> {
    let host = host()?;
    if let Some((plan, digest)) = original(&host)? {
        return observe(&host, plan, digest);
    }
    let version = version()?;
    let current = installed(&version)?;
    if current == [true; 3] {
        return Ok(Observation::RequiredPackagesInstalled {
            package_version: version,
        });
    }
    let apt = Executable::open(Program::AptGet)?;
    let prepared = fresh(&host, version, &apt, !current[0])?;
    Ok(Observation::Installable {
        request_digest: prepared.digest,
        plan: prepared.plan,
    })
}
pub(crate) fn refresh(input: Refresh) -> Result<&'static str> {
    if input.confirmation != "refresh-system-package-indexes"
        || !input.external_administration_stopped
    {
        return Err("confirm the download of indexes from configured signed APT sources and stop external package administrators".into());
    }
    let host = host()?;
    if original(&host)?.is_some() {
        return Err(
            "a package request is already retained; inspect it without changing indexes".into(),
        );
    }
    let apt = Executable::open(Program::AptGet)?;
    apt.package_command(&[
        "-o",
        "Acquire::AllowInsecureRepositories=false",
        "-o",
        "Acquire::AllowDowngradeToInsecureRepositories=false",
        "-o",
        "APT::Update::Error-Mode=any",
        "update",
    ])?;
    Ok("configured_apt_indexes_refreshed")
}
pub(crate) fn install(input: Commit) -> Result<Observation> {
    confirm(&input, "install-system-samba-packages")?;
    let host = host()?;
    if original(&host)?.is_some() {
        return Err("this package request was already admitted; query it and explicitly finish, never automatically repeat APT".into());
    }
    let version = version()?;
    let current = installed(&version)?;
    if current == [true; 3] {
        return Err("the required packages are already installed; recheck them without a new APT transaction".into());
    }
    let apt = Executable::open(Program::AptGet)?;
    let PreparedPlan {
        plan,
        bytes,
        digest,
    } = fresh(&host, version, &apt, !current[0])?;
    if input.request_digest != digest {
        return Err("APT resolution or host changed after preview; inspect a fresh plan before installation".into());
    }
    if !matches!(
        host.publish(ORIGINAL, &[("plan.json", &bytes)])?,
        BundlePublication::Created
    ) {
        return Err("original package staging already exists; APT was not repeated, inspect the retained request".into());
    }
    let policy = policy::Policy::open()?;
    policy.install(&policy::contents(&digest))?;
    let args = apt_args(&plan.package_version, false);
    apt.package_command(&args.iter().map(String::as_str).collect::<Vec<_>>())
        .map_err(|source| {
            Box::new(Pending { source }) as Box<dyn std::error::Error + Send + Sync>
        })?;
    finish_original(&host, plan, digest)
}
#[derive(Debug)]
struct Pending {
    source: Box<dyn std::error::Error + Send + Sync>,
}
impl std::fmt::Display for Pending {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("APT did not complete; preserve the original request and /usr/sbin/policy-rc.d, inspect package state before explicit finish, do not automatically retry")
    }
}
impl std::error::Error for Pending {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}
fn confirm(input: &Commit, expected: &str) -> Result<()> {
    if input.confirmation != expected || !input.external_administration_stopped {
        return Err("explicit package-operation and stopped external-administrator confirmations are required".into());
    }
    Ok(())
}
pub(crate) fn finish(input: Commit) -> Result<Observation> {
    confirm(&input, "finish-original-system-packages")?;
    let host = host()?;
    let (plan, digest) = original(&host)?.ok_or("no retained package installation is readable")?;
    if input.request_digest != digest {
        return Err("finish request differs from original package installation".into());
    }
    if host
        .read_operation(COMPLETE, "receipt.json", LIMIT)?
        .is_some()
    {
        return observe(&host, plan, digest);
    }
    finish_original(&host, plan, digest)
}
fn finish_original(host: &BundleDestination, plan: Plan, digest: String) -> Result<Observation> {
    if installed(&plan.package_version)? != [true; 3] {
        return Err("packages are incomplete; preserve the original request and service-start policy, resolve dpkg state before finishing; APT was not repeated".into());
    }
    let policy = policy::Policy::open()?;
    policy.absent_or_original(Some(&policy::contents(&digest)))?;
    // Only disable Samba defaults when this request installed Samba itself.
    if plan.new_samba {
        Executable::open(Program::Systemctl)?.capture(&[
            "--no-reload",
            "disable",
            "--",
            "smbd.service",
            "nmbd.service",
        ])?;
    }
    policy.remove_original(&policy::contents(&digest))?;
    host.publish(
        COMPLETE,
        &[(
            "receipt.json",
            &json_codec::to_vec(&Receipt {
                request_digest: digest.clone(),
            })?,
        )],
    )?;
    Ok(Observation::Completed {
        request_digest: digest,
        package_version: plan.package_version,
    })
}

#[cfg(test)]
#[path = "../../../test/helper/unit/initialization/packages.rs"]
mod tests;
