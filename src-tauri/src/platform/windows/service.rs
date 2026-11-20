//! The Windows service. It runs as SYSTEM in session 0, which has no screen,
//! so it starts the agent ("--agent") in the console session as SYSTEM too.
//! There the agent can join the "Winlogon" desktop and capture and control
//! the lock screen, the sign-in screen and UAC prompts. The service restarts
//! the agent when it exits or when another session gets the console, and
//! sends Ctrl+Alt+Del for it, which only a service may do.

use std::ffi::OsString;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, HANDLE, WAIT_TIMEOUT};
use windows::Win32::Security::Authentication::Identity::SendSAS;
use windows::Win32::Security::{
    DuplicateTokenEx, SecurityImpersonation, SetTokenInformation, TokenPrimary, TokenSessionId,
    TOKEN_ACCESS_MASK, TOKEN_ADJUST_DEFAULT, TOKEN_ADJUST_SESSIONID, TOKEN_ASSIGN_PRIMARY,
    TOKEN_DUPLICATE, TOKEN_QUERY,
};
use windows::Win32::System::Registry::{
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_LOCAL_MACHINE, REG_DWORD, REG_SZ,
    RRF_RT_REG_DWORD,
};
use windows::Win32::System::Threading::{
    CreateProcessAsUserW, GetCurrentProcess, GetExitCodeProcess, OpenProcessToken,
    TerminateProcess, WaitForSingleObject, CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, INFINITE,
    PROCESS_INFORMATION, STARTUPINFOW,
};
use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
use windows_service::service::{
    ServiceAccess, ServiceAction, ServiceActionType, ServiceControl, ServiceControlAccept,
    ServiceErrorControl, ServiceExitCode, ServiceFailureActions, ServiceFailureResetPeriod,
    ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
use windows_service::{define_windows_service, service_dispatcher};

use super::dpapi::wide;
use super::pipe;
use crate::config::{self, Config, Scope};

pub const SERVICE_NAME: &str = "CasvisorDesktop";
const DISPLAY_NAME: &str = "Casvisor Desktop";
const RUN_KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run";
const POLICY_KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System";

pub fn is_installed() -> bool {
    ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .and_then(|manager| manager.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS))
        .is_ok()
}

define_windows_service!(ffi_service_main, service_main);

pub fn run() -> Result<()> {
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)?;
    Ok(())
}

fn service_main(_args: Vec<OsString>) {
    if let Err(err) = run_service() {
        log::error!("the service stopped: {err:#}");
    }
}

fn set_state(
    handle: &service_control_handler::ServiceStatusHandle,
    state: ServiceState,
) -> Result<()> {
    let controls_accepted = if state == ServiceState::Running {
        ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
    } else {
        ServiceControlAccept::empty()
    };
    handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    })?;
    Ok(())
}

struct Agent {
    process: HANDLE,
    session_id: u32,
}

impl Agent {
    fn is_running(&self) -> bool {
        unsafe { WaitForSingleObject(self.process, 0) == WAIT_TIMEOUT }
    }

    fn stop(self) {
        unsafe {
            let _ = TerminateProcess(self.process, 0);
            let _ = CloseHandle(self.process);
        }
    }
}

fn run_service() -> Result<()> {
    let (stop_tx, stop_rx) = std::sync::mpsc::channel();
    let handle = service_control_handler::register(SERVICE_NAME, move |control| match control {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            let _ = stop_tx.send(());
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    })?;
    set_state(&handle, ServiceState::Running)?;
    log::info!("the service started");

    let runtime = tokio::runtime::Runtime::new()?;
    runtime.spawn(pipe::serve_service(|command| {
        if command == "sas" {
            log::info!("sending Ctrl+Alt+Del");
            unsafe { SendSAS(false) };
        }
    }));

    let mut agent: Option<Agent> = None;
    let mut last_start = Instant::now() - Duration::from_secs(60);
    while let Err(std::sync::mpsc::RecvTimeoutError::Timeout) =
        stop_rx.recv_timeout(Duration::from_millis(500))
    {
        let session_id = pipe::console_session_id();
        if session_id == u32::MAX {
            continue;
        }
        if agent
            .as_ref()
            .is_some_and(|a| a.session_id == session_id && a.is_running())
        {
            continue;
        }
        if let Some(old) = agent.take() {
            old.stop();
        }
        if last_start.elapsed() < Duration::from_secs(2) {
            continue;
        }
        last_start = Instant::now();
        match start_agent(session_id) {
            Ok(process) => {
                log::info!("started the agent in session {session_id}");
                agent = Some(Agent {
                    process,
                    session_id,
                });
            }
            Err(err) => log::warn!("failed to start the agent in session {session_id}: {err:#}"),
        }
    }

    if let Some(agent) = agent.take() {
        agent.stop();
    }
    runtime.shutdown_background();
    set_state(&handle, ServiceState::Stopped)?;
    log::info!("the service stopped");
    Ok(())
}

/// Starts "--agent" as SYSTEM in the session, on its interactive desktop.
fn start_agent(session_id: u32) -> Result<HANDLE> {
    let exe = std::env::current_exe()?;
    let exe_path = exe.to_string_lossy().to_string();
    let mut command = wide(&format!("\"{exe_path}\" --agent"));
    let exe_wide = wide(&exe_path);
    let mut desktop = wide("winsta0\\default");

    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ACCESS_MASK(
                TOKEN_DUPLICATE.0
                    | TOKEN_QUERY.0
                    | TOKEN_ASSIGN_PRIMARY.0
                    | TOKEN_ADJUST_SESSIONID.0
                    | TOKEN_ADJUST_DEFAULT.0,
            ),
            &mut token,
        )?;
        let mut primary = HANDLE::default();
        let duplicated = DuplicateTokenEx(
            token,
            TOKEN_ACCESS_MASK(0x02000000),
            None,
            SecurityImpersonation,
            TokenPrimary,
            &mut primary,
        );
        let _ = CloseHandle(token);
        duplicated?;

        let result = (|| -> Result<HANDLE> {
            SetTokenInformation(
                primary,
                TokenSessionId,
                &session_id as *const u32 as *const _,
                std::mem::size_of::<u32>() as u32,
            )?;
            let startup = STARTUPINFOW {
                cb: std::mem::size_of::<STARTUPINFOW>() as u32,
                lpDesktop: PWSTR(desktop.as_mut_ptr()),
                ..Default::default()
            };
            let mut info = PROCESS_INFORMATION::default();
            CreateProcessAsUserW(
                Some(primary),
                PCWSTR(exe_wide.as_ptr()),
                Some(PWSTR(command.as_mut_ptr())),
                None,
                None,
                false,
                CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT,
                None,
                None,
                &startup,
                &mut info,
            )?;
            let _ = CloseHandle(info.hThread);
            Ok(info.hProcess)
        })();
        let _ = CloseHandle(primary);
        result
    }
}

/// Asks the service for Ctrl+Alt+Del.
pub async fn send_ctrl_alt_del() -> Result<()> {
    pipe::request_service("sas").await
}

fn set_dword(key: &str, name: &str, value: u32) -> Result<()> {
    let (key, name) = (wide(key), wide(name));
    unsafe {
        RegSetKeyValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(key.as_ptr()),
            PCWSTR(name.as_ptr()),
            REG_DWORD.0,
            Some(&value as *const u32 as *const _),
            4,
        )
        .ok()?;
    }
    Ok(())
}

fn get_dword(key: &str, name: &str) -> Option<u32> {
    let (key, name) = (wide(key), wide(name));
    let mut value = 0u32;
    let mut size = 4u32;
    unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(key.as_ptr()),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut _),
            Some(&mut size),
        )
        .ok()
        .ok()?;
    }
    Some(value)
}

fn set_string(key: &str, name: &str, value: &str) -> Result<()> {
    let (key, name, value) = (wide(key), wide(name), wide(value));
    unsafe {
        RegSetKeyValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(key.as_ptr()),
            PCWSTR(name.as_ptr()),
            REG_SZ.0,
            Some(value.as_ptr() as *const _),
            (value.len() * 2) as u32,
        )
        .ok()?;
    }
    Ok(())
}

fn delete_value(key: &str, name: &str) {
    let (key, name) = (wide(key), wide(name));
    unsafe {
        let _ = RegDeleteKeyValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(key.as_ptr()),
            PCWSTR(name.as_ptr()),
        );
    }
}

/// Installs and starts the service, moving the settings of the user who runs
/// it to the machine. Needs an administrator.
pub fn install() -> Result<()> {
    let exe = std::env::current_exe()?;
    let manager = ServiceManager::local_computer(
        None::<&str>,
        ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
    )
    .context("installing the service needs an administrator")?;

    let machine = Config::load(Scope::Machine);
    if machine.is_complete() {
        machine.save(Scope::Machine)?;
    } else {
        let user = Config::load(Scope::User);
        if user.is_complete() {
            user.save(Scope::Machine)?;
        } else {
            std::fs::create_dir_all(config::dir(Scope::Machine))?;
            super::restrict_to_admins(&config::dir(Scope::Machine))?;
        }
    }

    let info = ServiceInfo {
        name: SERVICE_NAME.into(),
        display_name: DISPLAY_NAME.into(),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe.clone(),
        launch_arguments: vec!["--service".into()],
        dependencies: vec![],
        account_name: None,
        account_password: None,
    };
    let access = ServiceAccess::QUERY_STATUS
        | ServiceAccess::START
        | ServiceAccess::STOP
        | ServiceAccess::CHANGE_CONFIG;
    let service = match manager.open_service(SERVICE_NAME, access) {
        Ok(service) => {
            service.change_config(&info)?;
            service
        }
        Err(_) => manager.create_service(&info, access)?,
    };
    service.set_description(
        "Lets Casvisor control this computer, also at the lock screen, the sign-in screen and UAC prompts.",
    )?;
    let restart = ServiceAction {
        action_type: ServiceActionType::Restart,
        delay: Duration::from_secs(5),
    };
    service.update_failure_actions(ServiceFailureActions {
        reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(86400)),
        reboot_msg: None,
        command: None,
        actions: Some(vec![restart.clone(), restart.clone(), restart]),
    })?;

    // Services may send Ctrl+Alt+Del only when this policy allows it.
    if !matches!(
        get_dword(POLICY_KEY, "SoftwareSASGeneration"),
        Some(1) | Some(3)
    ) {
        set_dword(POLICY_KEY, "SoftwareSASGeneration", 1)?;
    }
    // The app runs for everyone who signs in, to ask them and show the bar.
    set_string(
        RUN_KEY,
        DISPLAY_NAME,
        &format!("\"{}\" --hidden", exe.to_string_lossy()),
    )?;

    if service.query_status()?.current_state == ServiceState::Running {
        service.stop()?;
        wait_for(&service, ServiceState::Stopped)?;
    }
    service.start::<&str>(&[])?;
    Ok(())
}

fn wait_for(service: &windows_service::service::Service, state: ServiceState) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(15);
    while service.query_status()?.current_state != state {
        if Instant::now() > deadline {
            bail!("the service didn't get to {state:?}");
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Ok(())
}

pub fn uninstall() -> Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .context("uninstalling the service needs an administrator")?;
    let service = manager.open_service(
        SERVICE_NAME,
        ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE,
    )?;
    if service.query_status()?.current_state != ServiceState::Stopped {
        service.stop()?;
        wait_for(&service, ServiceState::Stopped)?;
    }
    service.delete()?;
    delete_value(RUN_KEY, DISPLAY_NAME);
    Ok(())
}

/// Runs this program as an administrator with the arguments and waits for it.
/// The elevated process writes "ok" or the error to the result file.
pub fn run_elevated(args: &[&str]) -> Result<()> {
    let exe = std::env::current_exe()?;
    let result_path = std::env::temp_dir().join(format!(
        "casvisor-desktop-result-{}.txt",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&result_path);
    let mut params: Vec<String> = args.iter().map(|a| format!("\"{a}\"")).collect();
    params.push("--result".into());
    params.push(format!("\"{}\"", result_path.to_string_lossy()));

    let (verb, file, params) = (
        wide("runas"),
        wide(&exe.to_string_lossy()),
        wide(&params.join(" ")),
    );
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    unsafe {
        if let Err(err) = ShellExecuteExW(&mut info) {
            if err.code() == ERROR_CANCELLED.to_hresult() {
                bail!("cancelled");
            }
            return Err(err.into());
        }
        WaitForSingleObject(info.hProcess, INFINITE);
        let mut code = 0u32;
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
    }
    let result = std::fs::read_to_string(&result_path).unwrap_or_default();
    let _ = std::fs::remove_file(&result_path);
    match result.trim() {
        "ok" => Ok(()),
        "" => Err(anyhow!("the elevated process didn't finish")),
        message => Err(anyhow!("{message}")),
    }
}
