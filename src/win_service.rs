//!
//! Windows服务接口封装
//!
//! 实现WinService接口即可
//!

use futures::channel::oneshot;
use std::{
    env, error::Error, ffi::OsString, mem::transmute, process::Command, sync::{Arc, Mutex}, thread, time
};
use windows_service::{
    service::*, service_control_handler::{self, ServiceControlHandlerResult, ServiceStatusHandle}, service_dispatcher, service_manager::*
};

const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;

windows_service::define_windows_service!(ffi_service_main, service_main);

static mut SRV_REGISTERED: Option<&'static (dyn WinService + Sync)> = None;

/// 退出代码
pub mod exit_code {
    pub const OK: u16 = 0;
    pub const RESTART: u16 = 3010; //ERROR_SUCCESS_REBOOT_REQUIRED
}

/// 服务接口
pub trait WinService: Sync {
    /// 服务名称
    fn name(&self) -> &str;

    /// 服务描述
    fn description(&self) -> &str { self.name() }

    /// 初始化
    fn initialize(&self, from_scm: bool) -> Result<(), Box<dyn Error>>;
    /// 服务入口过程
    fn main(&self, stop_signer: Option<oneshot::Receiver<()>>) -> Result<u16, Box<dyn Error>>;

    /// 安装Windows服务
    fn install(&self, run_args: Vec<OsString>) -> Result<(), Box<dyn Error>> {
        let mgr = ServiceManager::local_computer(
            None::<&str>,
            ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE
        )?;
        let info = ServiceInfo {
            name: self.name().into(),
            display_name: self.description().into(),
            service_type: SERVICE_TYPE,
            #[cfg(debug_assertions)]
            start_type: ServiceStartType::OnDemand,
            #[cfg(not(debug_assertions))]
            start_type: ServiceStartType::AutoStart,
            error_control: ServiceErrorControl::Normal,
            executable_path: env::current_exe()?,
            launch_arguments: run_args,
            dependencies: vec![],
            account_name: None,
            account_password: None
        };
        let srv = mgr.create_service(&info, ServiceAccess::START | ServiceAccess::CHANGE_CONFIG)?;
        //开启故障自动重启
        let failure_actions = ServiceFailureActions {
            //[在此时间之后重置失败计数]
            reset_period: ServiceFailureResetPeriod::After(time::Duration::from_secs(86400)),
            reboot_msg: None,
            command: None,
            actions: Some(vec![
                //[第一次失败]
                ServiceAction {
                    action_type: ServiceActionType::Restart,
                    delay: time::Duration::from_secs(5)
                },
                //[第二次失败]
                ServiceAction {
                    action_type: ServiceActionType::Restart,
                    delay: time::Duration::from_secs(15)
                },
                //[后续失败]
                ServiceAction {
                    action_type: ServiceActionType::None,
                    delay: Default::default()
                },
            ])
        };
        srv.update_failure_actions(failure_actions)?;
        //[启用发生错误时]
        //退出代码不为0时也触发故障重启机制
        //srv.set_failure_actions_on_non_crash_failures(true)?;
        Ok(())
    }

    /// 卸载Windows服务
    fn uninstall(&self) -> Result<(), Box<dyn Error>> {
        let mgr = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
        let srv = mgr.open_service(
            self.name(),
            ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE
        )?;
        if srv.query_status()?.current_state != ServiceState::Stopped {
            srv.stop()?;
            while srv.query_status()?.current_state != ServiceState::Stopped {
                thread::sleep(time::Duration::from_secs(1));
            }
        }
        srv.delete()?;
        Ok(())
    }

    /// 运行Windows服务
    fn run(&self) -> Result<(), Box<dyn Error>>
    where
        Self: Sized
    {
        //挂载Windows服务
        unsafe {
            SRV_REGISTERED = Some(transmute(self as &dyn WinService));
        }
        if let Err(e) = service_dispatcher::start(self.name(), ffi_service_main) {
            if let windows_service::Error::Winapi(ref e) = e {
                match e.raw_os_error() {
                    //非SCM调用则直接运行服务
                    Some(code) if code == 1063 => return run_service(false),
                    _ => {}
                }
            }
            return Err(e.into());
        }
        Ok(())
    }

    /// 启动Windows服务
    fn start(&self) -> Result<(), Box<dyn Error>> {
        let mgr = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
        let srv = mgr.open_service(self.name(), ServiceAccess::QUERY_STATUS | ServiceAccess::START)?;
        if srv.query_status()?.current_state != ServiceState::Running {
            srv.start(&[""])?;
            while srv.query_status()?.current_state != ServiceState::Running {
                thread::sleep(time::Duration::from_secs(1));
            }
        }
        Ok(())
    }

    /// 停止Windows服务
    fn stop(&self) -> Result<(), Box<dyn Error>> {
        let mgr = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
        let srv = mgr.open_service(self.name(), ServiceAccess::QUERY_STATUS | ServiceAccess::STOP)?;
        if srv.query_status()?.current_state != ServiceState::Stopped {
            srv.stop()?;
            while srv.query_status()?.current_state != ServiceState::Stopped {
                thread::sleep(time::Duration::from_secs(1));
            }
        }
        Ok(())
    }
}

/// Windows服务入口(运行时独立的线程中)
fn service_main(_: Vec<OsString>) {
    //为了使日志输出的线程名称不为"unnamed",创建一个"srv-main"名称的线程来运行
    if let Ok(thread) =
        thread::Builder::new().name("srv-main".to_owned()).spawn(|| run_service(true).unwrap())
    {
        thread.join().unwrap();
    }
}

/// 启动服务
fn run_service(from_scm: bool) -> Result<(), Box<dyn Error>> {
    let srv = unsafe { SRV_REGISTERED.take().ok_or("服务实例未注册")? };
    if let Err(e) = srv.initialize(from_scm) {
        if log_enabled!(log::Level::Error) {
            error!("initial error: {:?}", e);
        } else {
            eprintln!("initial error: {:?}", e);
        }
        return Err(e);
    }

    info!("run");

    let rv = if from_scm {
        scm_serve(srv)
    } else {
        srv.main(None)
    };

    match rv {
        Ok(exit_code) => {
            if exit_code == exit_code::RESTART {
                //非SCM拖管时重启程序
                if !from_scm {
                    //使用PING延迟3S重新启动程序
                    let cmd =
                        format!("/c ping 127.0.0.1 > nul && {} --run", env::current_exe().unwrap().display());
                    if let Err(e) = Command::new("cmd.exe")
                        //使用相同的当前目录
                        .current_dir(env::current_dir().unwrap())
                        .arg(cmd)
                        .spawn()
                    {
                        warn!("restart failed, error: {}", e);
                    } else {
                        info!("restarting");
                    }
                }
            } else {
                info!("stop");
            }
        },
        Err(e) => {
            error!("abnormal terminated, error: {:?}", e);
        }
    }

    Ok(())
}

/// Windows服务开始(由SCM调用)
fn scm_serve(srv: &dyn WinService) -> Result<u16, Box<dyn Error>> {
    //服务状态汇报的句柄
    let status = Arc::new(Mutex::new(None));

    //创建停止信号的通道
    let (tx, rx) = oneshot::channel::<()>();
    let mut tx = Some(tx);

    //注册SCM控制事件
    let status_clone = status.clone();
    let event_handler = move |event| -> ServiceControlHandlerResult {
        match event {
            //停止服务
            ServiceControl::Stop => {
                if let Some(tx) = tx.take() {
                    let _ = tx.send(());
                }
                report_service_status(&status_clone, ServiceState::StopPending, None);
                ServiceControlHandlerResult::NoError
            },
            //心跳检测
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented
        }
    };
    *status.lock().unwrap() = Some(service_control_handler::register(srv.name(), event_handler)?);

    //报告状态
    report_service_status(&status, ServiceState::Running, None);

    //运行
    let rv = srv.main(Some(rx));

    let exit_code = match rv.as_ref() {
        Ok(exit_code) => {
            if exit_code == &exit_code::RESTART {
                //*在服务状态设置为停止之前才能创建进程
                //使用PING延迟3S重新启动服务
                let cmd = format!("/c ping 127.0.0.1 > nul && sc start {} > nul", srv.name());
                if let Err(e) = Command::new("cmd.exe").arg(cmd).spawn() {
                    warn!("restart failed, error: {}", e);
                } else {
                    info!("restarting");
                }
                //正常状态退出
                exit_code::OK
            } else {
                *exit_code
            }
        },
        _ => 0xffff
    };

    //报告状态
    report_service_status(&status, ServiceState::Stopped, Some(exit_code));

    rv
}

/// 向SCM报告服务的状态
fn report_service_status(
    status_hdl: &Arc<Mutex<Option<ServiceStatusHandle>>>,
    state: ServiceState,
    exit_code: Option<u16>
) {
    let accept_ctl = if state == ServiceState::Running {
        ServiceControlAccept::STOP
    } else {
        ServiceControlAccept::empty()
    };
    status_hdl
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: state,
            controls_accepted: accept_ctl,
            exit_code: ServiceExitCode::Win32(exit_code.unwrap_or(0) as u32),
            checkpoint: 0,
            wait_hint: time::Duration::default(),
            process_id: None
        })
        .unwrap();
}
