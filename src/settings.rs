#![allow(dead_code)]
use config::{Config, ConfigError, File};
use std::sync::{Arc, RwLock};

/// 静态数据
pub mod data {
    /// 配置参数
    pub const CONFIG_STR: &'static str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "\\log.toml"));
}

/// 默认参数
pub mod default {
    /// 配置文件相对路径
    pub const CONFIG_PATH: &'static str = "config.toml";
    /// 日志配置文件相对路径
    pub const LOG_CONFIG_PATH: &'static str = "log.toml";
    /// HTTP监听地址
    pub const HTTP_LISTEN: &'static str = "127.0.0.1:1086";
    /// HTTP日志格式
    pub const HTTP_LOG_FORMAT: &'static str = "src: %a req: \"%r\", %{Content-Type}i resp: %s, %bbytes, %{Content-Encoding}o agent: \"%{User-Agent}i\" elapsed: %Dms";
}

#[derive(Clone)]
pub struct SharedConfig {
    /// 配置参数
    cfg: Arc<RwLock<Config>>
}

impl SharedConfig {
    pub fn load() -> Result<SharedConfig, ConfigError> {
        let mut cfg = Config::new();
        //配置默认参数
        cfg.set_default("http.listen", default::HTTP_LISTEN)?;
        cfg.set_default("http.log_format", default::HTTP_LOG_FORMAT)?;
        //加载配置文件合并参数
        cfg.merge(File::with_name(default::CONFIG_PATH).required(false))?;

        let cfg = Arc::new(RwLock::new(cfg));

        Ok(SharedConfig {
            cfg
        })
    }

    pub fn config_string(&self, key: &str) -> String {
        self.cfg.read().unwrap().get_str(key).expect(&format!("config key: {}, type: string", key))
    }
    pub fn config_bool(&self, key: &str) -> bool {
        self.cfg.read().unwrap().get_bool(key).expect(&format!("config key: {}, type: bool", key))
    }
    pub fn config_int(&self, key: &str) -> i64 {
        self.cfg.read().unwrap().get_int(key).expect(&format!("config key: {}, type: int", key))
    }
    pub fn config_float(&self, key: &str) -> f64 {
        self.cfg.read().unwrap().get_float(key).expect(&format!("config key: {}, type: float", key))
    }
    pub fn config<'de, T: serde::Deserialize<'de>>(&self, key: &str) -> Result<T, ConfigError> {
        self.cfg.read().unwrap().get(key)
    }
}
