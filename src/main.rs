#![allow(non_snake_case)]
#![feature(backtrace)]

#[macro_use]
extern crate log;
#[macro_use]
extern crate clap;
use clap::{Arg, ArgGroup, SubCommand};
use futures::channel::oneshot;
use std::error::Error;

type AnyError = Box<dyn std::error::Error>;

mod settings;
mod svn;
mod worktile;
mod endpoint;
mod win_service;

use win_service::WinService;

const CLAP_TEMPLATE: &'static str = r"
{about} [版本 {version}]
{author}

使用方法:
{usage}

参数说明:
{flags}";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    //启动参数解析
    let clap = app_from_crate!()
        .template(CLAP_TEMPLATE)
        .help_message("打印此帮助信息")
        .version_message("打印版本信息")
        .subcommand(
            SubCommand::with_name("service")
                .version(crate_version!())
                .author(crate_authors!())
                .about(crate_description!())
                .template(CLAP_TEMPLATE)
                .help_message("打印此帮助信息")
                .version_message("打印版本信息")
                .arg(Arg::with_name("install").long("install").help("安装为Windows服务").display_order(1))
                .arg(Arg::with_name("uninstall").long("uninstall").help("从Windows服务卸载").display_order(2))
                .arg(Arg::with_name("start").long("start").help("开始Windows服务").display_order(3))
                .arg(Arg::with_name("stop").long("stop").help("停止Windows服务").display_order(4))
                .arg(Arg::with_name("run").long("run").help("直接运行服务").display_order(5))
                .group(ArgGroup::with_name("action").args(&["install", "uninstall", "run", "start", "stop"]))
        )
        .subcommand(
            SubCommand::with_name("commit")
                .version(crate_version!())
                .author(crate_authors!())
                .about(crate_description!())
                .template(CLAP_TEMPLATE)
                .help_message("打印此帮助信息")
                .version_message("打印版本信息")
                .arg(
                    Arg::with_name("repo_path")
                        .short("p")
                        .long("repo_path")
                        .help("仓库位置")
                        .takes_value(true)
                        .required(true)
                        .display_order(1)
                )
                .arg(
                    Arg::with_name("repo_name")
                        .short("n")
                        .long("repo_name")
                        .help("仓库名称")
                        .takes_value(true)
                        .required(true)
                        .display_order(2)
                )
                .arg(
                    Arg::with_name("revision")
                        .short("r")
                        .long("revision")
                        .help("版本号")
                        .takes_value(true)
                        .required(true)
                        .display_order(3)
                )
        )
        .get_matches();
    //[Service]命令
    if let Some(ref matches) = clap.subcommand_matches("service") {
        if matches.is_present("install") {
            Service.install(vec!["service".into(), "--run".into()])
        } else if matches.is_present("uninstall") {
            Service.uninstall()
        } else if matches.is_present("start") {
            Service.start()
        } else if matches.is_present("stop") {
            Service.stop()
        } else if matches.is_present("run") {
            Service.run()
        } else {
            unimplemented!()
        }
    }
    //[Commit]命令
    else if let Some(ref matches) = clap.subcommand_matches("commit") {
        match (matches.value_of("repo_path"), matches.value_of("repo_name"), matches.value_of("revision")) {
            (Some(repo_path), Some(repo_name), Some(rev)) => {
                endpoint::request_commit(repo_path, repo_name, rev)
            },
            _ => panic!("[commit]缺少参数")
        }
    } else {
        println!("{}", clap.usage());
        Ok(())
    }
}

struct Service;

impl WinService for Service {
    /// 服务名称
    fn name(&self) -> &str { crate_name!() }
    /// 服务描述
    fn description(&self) -> &str { crate_description!() }
    /// 初始化
    fn initialize(&self, from_scm: bool) -> Result<(), Box<dyn Error>> {
        use std::{backtrace::Backtrace, env, panic};

        //设置工作目录
        let exe_path = env::current_exe()?;
        if from_scm || !cfg!(debug_assertions) {
            env::set_current_dir(exe_path.as_path().parent().unwrap())
                .map_err(|e| format!("设置当前目录失败, {}", e))?;
        }
        //初始化日志配置
        let log_de = log4rs::file::Deserializers::default();
        log4rs::init_file(settings::default::LOG_CONFIG_PATH, log_de.clone())
            .or_else(|e| {
                init_default_log(log_de).map(move |_| {
                    let mut err_info = String::new();
                    if let log4rs::Error::Log4rs(e) = e {
                        match e.downcast_ref::<std::io::Error>() {
                            //忽略文件不存在的错误 (ERROR_FILE_NOT_FOUND)
                            Some(e) if e.raw_os_error() == Some(2) => {},
                            _ => err_info = e.to_string()
                        }
                    }
                    if !err_info.is_empty() {
                        info!("use default log config, cause: {}", err_info);
                    } else {
                        info!("use default log config");
                    }
                })
            })
            .map_err(|e| format!("加载日志配置文件失败, {}", e))?;
        //捕获全局Panic事件打印错误日志
        panic::set_hook(Box::new(|info| {
            error!("{} backtrace:\r\n{}", info, Backtrace::force_capture().to_string().replace("\n", "\r\n"))
        }));
        Ok(())
    }
    /// 服务入口过程
    fn main(&self, stop_signer: Option<oneshot::Receiver<()>>) -> Result<u16, Box<dyn Error>> {
        let rv = endpoint::http_serve(stop_signer);
        log::logger().flush();
        rv
    }
}

/// 初始化默认的日志配置
fn init_default_log(de: log4rs::file::Deserializers) -> Result<(), Box<dyn Error>> {
    use log4rs::{config::Config, file::RawConfig, Logger};

    let config: RawConfig = toml::from_str(settings::data::CONFIG_STR)?;
    let (appenders, _) = config.appenders_lossy(&de);
    let (config, _) =
        Config::builder().appenders(appenders).loggers(config.loggers()).build_lossy(config.root());
    let logger = Box::new(Logger::new(config));
    log::set_max_level(log::LevelFilter::Trace);
    log::set_boxed_logger(logger)?;

    Ok(())
}
