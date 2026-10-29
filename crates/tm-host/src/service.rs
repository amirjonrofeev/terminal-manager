//! Windows service mode: the SCM entry point, and `install` / `uninstall`.

use crate::ServiceConfig;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tm_core::paths::{self, HostMode};
use tm_core::persist;
use windows_service::service::{
    ServiceAccess, ServiceAction, ServiceActionType, ServiceControl, ServiceControlAccept, ServiceErrorControl,
    ServiceExitCode, ServiceFailureActions, ServiceFailureResetPeriod, ServiceInfo, ServiceStartType, ServiceState,
    ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
use windows_service::{define_windows_service, service_dispatcher};

pub const SERVICE_NAME: &str = "TerminalManagerHost";
const DISPLAY_NAME: &str = "Terminal Manager Host";
const DESCRIPTION: &str = "Runs the terminals configured in Terminal Manager and keeps them running without a signed-in user.";

define_windows_service!(ffi_service_main, service_main);

/// Hands the process to the Service Control Manager (for `tm-host service`).
pub fn dispatch() -> windows_service::Result<()> {
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
}

fn service_main(_args: Vec<OsString>) {
    if let Err(e) = run_service() {
        log::error!("service failed: {e}");
    }
}

fn status(state: ServiceState, exit_code: u32) -> ServiceStatus {
    ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: if state == ServiceState::Running {
            ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
        } else {
            ServiceControlAccept::empty()
        },
        exit_code: ServiceExitCode::Win32(exit_code),
        checkpoint: 0,
        wait_hint: Duration::from_secs(if state == ServiceState::StopPending { 20 } else { 0 }),
        process_id: None,
    }
}

fn run_service() -> windows_service::Result<()> {
    let (stop_tx, stop_rx) = mpsc::channel();
    let handle = service_control_handler::register(SERVICE_NAME, move |control| match control {
        ServiceControl::Stop | ServiceControl::Shutdown | ServiceControl::Preshutdown => {
            let _ = stop_tx.send(());
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    })?;
    handle.set_service_status(status(ServiceState::StartPending, 0))?;

    match crate::start(HostMode::Service, false) {
        Ok(host) => {
            handle.set_service_status(status(ServiceState::Running, 0))?;
            let _ = stop_rx.recv();
            log::info!("service stopping");
            handle.set_service_status(status(ServiceState::StopPending, 0))?;
            host.shutdown();
            handle.set_service_status(status(ServiceState::Stopped, 0))?;
        }
        Err(e) => {
            log::error!("could not start the host: {e}");
            handle.set_service_status(status(ServiceState::Stopped, 1))?;
        }
    }
    Ok(())
}

// ---------- install / uninstall ----------

pub struct InstallOptions {
    pub account: String,
    pub password: String,
    /// Extra accounts or groups (besides Administrators) allowed to use the GUI with this service.
    pub allow: Vec<String>,
    pub start: bool,
}

fn install_dir() -> PathBuf {
    std::env::var_os("ProgramFiles").map(PathBuf::from).unwrap_or_else(|| r"C:\Program Files".into()).join("TerminalManager")
}

/// `user`, `.\user` -> `COMPUTER\user`; `DOMAIN\user` and `user@domain` are left alone.
pub fn qualify_account(account: &str) -> String {
    let computer = std::env::var("COMPUTERNAME").unwrap_or_else(|_| ".".into());
    if let Some(rest) = account.strip_prefix(".\\") {
        format!("{computer}\\{rest}")
    } else if account.contains('\\') || account.contains('@') {
        account.to_string()
    } else {
        format!("{computer}\\{account}")
    }
}

pub fn install(opts: InstallOptions) -> Result<(), String> {
    let account = qualify_account(&opts.account);
    let sid = sid::lookup(&account).map_err(|e| format!("Unknown account {account}: {e}"))?;
    let mut allow = Vec::new();
    for name in &opts.allow {
        let s = sid::lookup(name).map_err(|e| format!("Unknown account or group {name}: {e}"))?;
        allow.push(sid::to_string(&s).map_err(|e| format!("{name}: {e}"))?);
    }

    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE)
        .map_err(|e| format!("Could not open the Service Control Manager (run this as administrator): {e}"))?;
    if manager.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS).is_ok() {
        return Err(format!("{DISPLAY_NAME} is already installed. Run `tm-host uninstall` first."));
    }

    // 1. The account needs the "Log on as a service" right; CreateService doesn't grant it.
    sid::grant_service_logon(&sid).map_err(|e| format!("Could not grant {account} the \"Log on as a service\" right: {e}"))?;
    println!("Granted {account} the \"Log on as a service\" right.");

    // 2. Copy the binary somewhere the service account can read and other users can't change.
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = install_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    let target = dir.join("tm-host.exe");
    if !same_file(&exe, &target) {
        std::fs::copy(&exe, &target).map_err(|e| format!("Could not copy to {}: {e}", target.display()))?;
    }
    println!("Installed {}.", target.display());

    // 3. Data folder: only SYSTEM, Administrators and the service account.
    let data = paths::data_dir(HostMode::Service);
    std::fs::create_dir_all(&data).map_err(|e| format!("Could not create {}: {e}", data.display()))?;
    let account_sid = sid::to_string(&sid)?;
    let acl = std::process::Command::new("icacls")
        .arg(&data)
        .args(["/inheritance:r", "/grant:r"])
        .args(["*S-1-5-18:(OI)(CI)F", "*S-1-5-32-544:(OI)(CI)F", &format!("*{account_sid}:(OI)(CI)M")])
        .output()
        .map_err(|e| format!("Could not run icacls: {e}"))?;
    if !acl.status.success() {
        return Err(format!("Setting permissions on {} failed: {}", data.display(), String::from_utf8_lossy(&acl.stdout)));
    }
    persist::save(&data.join("service.json"), &ServiceConfig { allow })
        .map_err(|e| format!("Could not write service.json: {e}"))?;
    println!("Data folder {} (SYSTEM, Administrators and {account} only).", data.display());

    // 4. Register: automatic (delayed) start, restart on crash.
    let info = ServiceInfo {
        name: SERVICE_NAME.into(),
        display_name: DISPLAY_NAME.into(),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: target.clone(),
        launch_arguments: vec![OsString::from("service")],
        dependencies: vec![],
        account_name: Some(account.clone().into()),
        account_password: Some(opts.password.into()),
    };
    let service = manager
        .create_service(&info, ServiceAccess::CHANGE_CONFIG | ServiceAccess::START | ServiceAccess::QUERY_STATUS)
        .map_err(|e| format!("Could not create the service: {e}"))?;
    let _ = service.set_description(DESCRIPTION);
    let _ = service.set_delayed_auto_start(true);
    let restart = |secs| ServiceAction { action_type: ServiceActionType::Restart, delay: Duration::from_secs(secs) };
    let _ = service.update_failure_actions(ServiceFailureActions {
        reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(24 * 3600)),
        reboot_msg: None,
        command: None,
        actions: Some(vec![restart(5), restart(15), restart(60)]),
    });
    println!("Registered service {SERVICE_NAME} ({DISPLAY_NAME}), runs as {account}.");

    if opts.start {
        service.start(&[] as &[&OsStr]).map_err(|e| {
            format!("The service was installed but did not start: {e}. Check the password, and {}.", data.join("host.log").display())
        })?;
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let state = service.query_status().map(|s| s.current_state).unwrap_or(ServiceState::Stopped);
            match state {
                ServiceState::Running => {
                    println!("Service started.");
                    break;
                }
                ServiceState::Stopped => {
                    return Err(format!("The service stopped right after starting. See {}.", data.join("host.log").display()));
                }
                _ if Instant::now() > deadline => {
                    println!("The service is still starting; check `sc query {SERVICE_NAME}`.");
                    break;
                }
                _ => std::thread::sleep(Duration::from_millis(250)),
            }
        }
    }
    Ok(())
}

pub fn uninstall() -> Result<(), String> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .map_err(|e| format!("Could not open the Service Control Manager (run this as administrator): {e}"))?;
    let service = manager
        .open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE)
        .map_err(|e| format!("{DISPLAY_NAME} is not installed or not accessible: {e}"))?;
    if service.query_status().is_ok_and(|s| s.current_state != ServiceState::Stopped) {
        let _ = service.stop();
        let deadline = Instant::now() + Duration::from_secs(30);
        while service.query_status().is_ok_and(|s| s.current_state != ServiceState::Stopped) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(250));
        }
        println!("Stopped the service; its terminals were stopped.");
    }
    service.delete().map_err(|e| format!("Could not remove the service: {e}"))?;
    println!("Removed service {SERVICE_NAME}.");
    println!(
        "Kept {} (projects and logs) and {}. Delete them by hand if you no longer need them.",
        paths::data_dir(HostMode::Service).display(),
        install_dir().display()
    );
    Ok(())
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Account SIDs and the "Log on as a service" right, via advapi32.
mod sid {
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{LocalFree, GetLastError};
    use windows_sys::Win32::Security::Authentication::Identity::{
        LSA_HANDLE, LSA_OBJECT_ATTRIBUTES, LSA_UNICODE_STRING, LsaAddAccountRights, LsaClose, LsaNtStatusToWinError,
        LsaOpenPolicy, POLICY_CREATE_ACCOUNT, POLICY_LOOKUP_NAMES,
    };
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{LookupAccountNameW, PSID, SID_NAME_USE};

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn os_error(code: u32) -> String {
        std::io::Error::from_raw_os_error(code as i32).to_string()
    }

    pub fn lookup(account: &str) -> Result<Vec<u8>, String> {
        let name = wide(account);
        let (mut sid_len, mut dom_len, mut kind): (u32, u32, SID_NAME_USE) = (0, 0, 0);
        // SAFETY: the first call only reports buffer sizes; the second gets buffers of those sizes.
        unsafe {
            LookupAccountNameW(null(), name.as_ptr(), null_mut(), &mut sid_len, null_mut(), &mut dom_len, &mut kind);
            if sid_len == 0 {
                return Err(os_error(GetLastError()));
            }
            let mut sid = vec![0u8; sid_len as usize];
            let mut dom = vec![0u16; dom_len as usize];
            if LookupAccountNameW(
                null(),
                name.as_ptr(),
                sid.as_mut_ptr() as PSID,
                &mut sid_len,
                dom.as_mut_ptr(),
                &mut dom_len,
                &mut kind,
            ) == 0
            {
                return Err(os_error(GetLastError()));
            }
            Ok(sid)
        }
    }

    pub fn to_string(sid: &[u8]) -> Result<String, String> {
        let mut out: *mut u16 = null_mut();
        // SAFETY: `sid` came from LookupAccountNameW; the returned string is freed with LocalFree.
        unsafe {
            if ConvertSidToStringSidW(sid.as_ptr() as PSID, &mut out) == 0 {
                return Err(os_error(GetLastError()));
            }
            let len = (0..).take_while(|&i| *out.add(i) != 0).count();
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(out, len));
            LocalFree(out as _);
            Ok(s)
        }
    }

    pub fn grant_service_logon(sid: &[u8]) -> Result<(), String> {
        let right: Vec<u16> = "SeServiceLogonRight".encode_utf16().collect();
        let mut right_str = LSA_UNICODE_STRING {
            Length: (right.len() * 2) as u16,
            MaximumLength: (right.len() * 2) as u16,
            Buffer: right.as_ptr() as *mut u16,
        };
        // SAFETY: zeroed object attributes are what LsaOpenPolicy expects; handles are closed.
        unsafe {
            let attrs: LSA_OBJECT_ATTRIBUTES = std::mem::zeroed();
            let mut policy: LSA_HANDLE = 0;
            let status = LsaOpenPolicy(null(), &attrs, (POLICY_CREATE_ACCOUNT | POLICY_LOOKUP_NAMES) as u32, &mut policy);
            if status != 0 {
                return Err(os_error(LsaNtStatusToWinError(status)));
            }
            let status = LsaAddAccountRights(policy, sid.as_ptr() as PSID, &mut right_str, 1);
            LsaClose(policy);
            if status != 0 {
                return Err(os_error(LsaNtStatusToWinError(status)));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accounts_are_qualified_with_the_computer_name() {
        let pc = std::env::var("COMPUTERNAME").unwrap();
        assert_eq!(qualify_account("svc"), format!("{pc}\\svc"));
        assert_eq!(qualify_account(".\\svc"), format!("{pc}\\svc"));
        assert_eq!(qualify_account("CORP\\svc"), "CORP\\svc");
        assert_eq!(qualify_account("svc@corp.local"), "svc@corp.local");
    }

    #[test]
    fn well_known_sids_resolve() {
        let sid = sid::lookup("NT AUTHORITY\\SYSTEM").unwrap();
        assert_eq!(sid::to_string(&sid).unwrap(), "S-1-5-18");
    }
}
