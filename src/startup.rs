//! Per-user startup registration shared by the launcher and Settings.

use std::path::Path;

use windows::{
    Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, VARIANT_FALSE},
        System::{Com::*, Registry::*, TaskScheduler::*, Variant::VARIANT},
    },
    core::{BSTR, Interface, Result, w},
};

const RUN_KEY: windows::core::PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");

struct Com;

impl Drop for Com {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

fn with_scheduler<T>(f: impl FnOnce(ITaskService, ITaskFolder) -> Result<T>) -> Result<T> {
    unsafe {
        let initialized = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let _com = if initialized == windows::Win32::Foundation::RPC_E_CHANGED_MODE {
            None
        } else {
            initialized.ok()?;
            Some(Com)
        };
        let service: ITaskService = CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER)?;
        let empty = VARIANT::default();
        service.Connect(&empty, &empty, &empty, &empty)?;
        let root = service.GetFolder(&BSTR::from(r"\"))?;
        f(service, root)
    }
}

pub fn enabled() -> bool {
    let scheduled =
        with_scheduler(|_, root| unsafe { Ok(root.GetTask(&BSTR::from(crate::channel::NAME))?.Enabled()?.as_bool()) })
            .unwrap_or(false);
    scheduled
        || unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                crate::channel::RUN_VALUE,
                RRF_RT_REG_SZ,
                None,
                None,
                None,
            )
            .is_ok()
        }
}

pub fn set(exe: Option<&Path>) -> Result<()> {
    with_scheduler(|service, root| unsafe {
        let name = BSTR::from(crate::channel::NAME);
        let Some(exe) = exe else {
            return match root.DeleteTask(&name, 0) {
                Err(error) if error.code() == ERROR_FILE_NOT_FOUND.to_hresult() => Ok(()),
                result => result,
            };
        };
        let user = BSTR::from(format!("{}\\{}", service.ConnectedDomain()?, service.ConnectedUser()?));
        let definition = service.NewTask(0)?;
        let principal = definition.Principal()?;
        principal.SetUserId(&user)?;
        principal.SetLogonType(TASK_LOGON_INTERACTIVE_TOKEN)?;
        principal.SetRunLevel(TASK_RUNLEVEL_LUA)?;

        let trigger: ILogonTrigger = definition.Triggers()?.Create(TASK_TRIGGER_LOGON)?.cast()?;
        trigger.SetUserId(&user)?;
        let action: IExecAction = definition.Actions()?.Create(TASK_ACTION_EXEC)?.cast()?;
        action.SetPath(&BSTR::from(exe.to_string_lossy().as_ref()))?;
        action.SetArguments(&BSTR::from("--startup"))?;

        let settings = definition.Settings()?;
        settings.SetDisallowStartIfOnBatteries(VARIANT_FALSE)?;
        settings.SetStopIfGoingOnBatteries(VARIANT_FALSE)?;
        settings.SetExecutionTimeLimit(&BSTR::from("PT0S"))?;
        let empty = VARIANT::default();
        root.RegisterTaskDefinition(
            &name,
            &definition,
            TASK_CREATE_OR_UPDATE.0,
            &VARIANT::from(user),
            &empty,
            TASK_LOGON_INTERACTIVE_TOKEN,
            &empty,
        )?;
        Ok(())
    })?;
    let status = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, crate::channel::RUN_VALUE) };
    if status == ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        status.ok()
    }
}
