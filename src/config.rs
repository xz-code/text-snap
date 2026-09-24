//! 配置读写：只有全局快捷键和保存目录两项。
//!
//! 配置文件位置：`%APPDATA%\text-snap\config.json`，首次运行自动生成。
//! 手改之后在托盘菜单点「重新加载快捷键配置」即可生效，不用重启。

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// 读取选中文本的全局快捷键。写法：`Ctrl+Shift+A`、`Alt+Q`、`Ctrl+Alt+F12`
    pub hotkey: String,
    /// 「最小化/还原编辑器」的全局快捷键。**空 = 不注册**（只用托盘菜单）
    pub minimize_hotkey: Option<String>,
    /// 保存目录。留空表示用「文档\text-snap」
    pub save_dir: Option<String>,
    /// 启动时发一条系统通知，提示程序已在后台运行
    pub show_startup_toast: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hotkey: "Ctrl+Shift+A".to_string(),
            // 注意：Ctrl+Shift+D 是 VS Code 默认的「运行和调试」。
            // 本程序注册的是**全局**热键，会把它抢走 —— 用户可以到设置里改成别的。
            minimize_hotkey: Some("Ctrl+Shift+D".to_string()),
            save_dir: None,
            show_startup_toast: true,
        }
    }
}

impl Config {
    /// 去掉空白后的「最小化/还原」快捷键；为空表示不注册。
    pub fn minimize_spec(&self) -> Option<&str> {
        self.minimize_hotkey
            .as_deref()
            .map(str::trim)
            .filter(|spec| !spec.is_empty())
    }
}

/// 配置目录：`%APPDATA%\text-snap`
pub fn dir() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("text-snap")
}

pub fn path() -> PathBuf {
    dir().join("config.json")
}

/// 读配置。文件不存在或解析不了就回落到默认值，并顺手写一份出来，方便用户手改。
pub fn load() -> Config {
    match fs::read_to_string(path()) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
        Err(_) => {
            let cfg = Config::default();
            let _ = save(&cfg);
            cfg
        }
    }
}

pub fn save(cfg: &Config) -> anyhow::Result<()> {
    fs::create_dir_all(dir())?;
    fs::write(path(), serde_json::to_string_pretty(cfg)?)?;
    Ok(())
}

/// 算出真正要写入的目录。配置里没写就用「文档\text-snap」。
pub fn resolve_save_dir(cfg: &Config, document_dir: Option<&Path>) -> PathBuf {
    if let Some(custom) = cfg.save_dir.as_deref() {
        let custom = custom.trim();
        if !custom.is_empty() {
            return PathBuf::from(custom);
        }
    }
    document_dir
        .map(|p| p.join("text-snap"))
        .unwrap_or_else(|| dir().join("saved"))
}
