//! 开机自启：`HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 下的一个值。
//!
//! 与 `claude-traffic-light/src-tauri/src/autostart.rs` 对齐（那边是 Python 版
//! `install.py` 的 `cmd_autostart` 的 Rust 移植）。**唯一的差别是底层**：那边没有
//! windows-sys，只能手写 `extern "system"` 声明；本仓库本来就有 windows-sys，
//! 所以直接用它的 `Win32_System_Registry`。
//!
//! 这不只是少写几十行 —— 手写 `Reg*` 声明有个隐蔽的坑：`HKEY_CURRENT_USER`
//! 常量是 `0xFFFFFFFF80000001` 而不是 `0x80000001`，**必须符号扩展**，写错会得到
//! 莫名其妙的 `ERROR_INVALID_HANDLE`。windows-sys 生成的常量已经处理好了这个。
//!
//! 用 `HKCU` 而不是 `HKLM`：不需要管理员权限，也不会弹 UAC。
//! 代价是自启拉起的进程是**中完整性**（和手动双击 exe 一样），这一点和本程序
//! 一贯的 asInvoker 设计一致，详见 README「已知限制」。
//!
//! ## 为什么可以放心测注册表
//!
//! [`RunEntry`] 的键路径与值名都是字段，可以注入。所以单测用的是
//! `HKCU\Software\TextSnapTest\<pid>` 这种**自己的沙箱键**，跑完就删 ——
//! 绝不碰真实的那条 `Run` 记录。
//! 即便这样，那条测试仍然标了 `#[ignore]`：默认的 `cargo test` 不该写注册表。
//! 要跑它：`cargo test --bin text-snap -- --ignored`

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW,
    RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ,
};

#[cfg(test)]
use windows_sys::Win32::System::Registry::RegDeleteKeyW;

/// `Run` 键的路径。开机自启就写在这里。
pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// UTF-16 编码并补 NUL —— Win32 的宽字符 API 要的就是这个。
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 一条「开机启动」记录。
#[derive(Clone, Debug)]
pub struct RunEntry {
    pub key_path: String,
    pub name: String,
}

impl RunEntry {
    /// 本程序真实使用的那一条。
    pub fn app() -> Self {
        Self {
            key_path: RUN_KEY.to_string(),
            name: "text-snap".to_string(),
        }
    }

    /// 读当前值；没开则返回 `None`。
    pub fn current(&self) -> Option<String> {
        unsafe {
            let mut key: HKEY = std::ptr::null_mut();
            let rc = RegOpenKeyExW(
                HKEY_CURRENT_USER,
                wide(&self.key_path).as_ptr(),
                0,
                KEY_QUERY_VALUE,
                &mut key,
            );
            if rc != ERROR_SUCCESS {
                return None;
            }

            // 先问长度，再按长度取数据（两步是 Win32 的标准用法）
            let mut ty: u32 = 0;
            let mut len: u32 = 0;
            let rc = RegQueryValueExW(
                key,
                wide(&self.name).as_ptr(),
                std::ptr::null_mut(),
                &mut ty,
                std::ptr::null_mut(),
                &mut len,
            );
            if rc != ERROR_SUCCESS || len == 0 {
                RegCloseKey(key);
                return None;
            }

            let mut buf = vec![0u8; len as usize];
            let rc = RegQueryValueExW(
                key,
                wide(&self.name).as_ptr(),
                std::ptr::null_mut(),
                &mut ty,
                buf.as_mut_ptr(),
                &mut len,
            );
            RegCloseKey(key);
            if rc != ERROR_SUCCESS {
                return None;
            }

            // REG_SZ 是 UTF-16，末尾带一个 NUL
            let u16s: Vec<u16> = buf
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            let end = u16s.iter().position(|&c| c == 0).unwrap_or(u16s.len());
            Some(String::from_utf16_lossy(&u16s[..end]))
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.current().is_some()
    }

    /// 开启（或改写成新的命令）。
    ///
    /// 用 `RegCreateKeyExW` 而不是 `RegOpenKeyExW`：`Run` 键在极少数干净系统上
    /// 可能不存在，而"键不存在"不该被当成"开不了自启"。
    pub fn enable(&self, command: &str) -> Result<(), String> {
        unsafe {
            let mut key: HKEY = std::ptr::null_mut();
            let mut disp: u32 = 0;
            let rc = RegCreateKeyExW(
                HKEY_CURRENT_USER,
                wide(&self.key_path).as_ptr(),
                0,
                std::ptr::null(),
                0,
                KEY_SET_VALUE,
                std::ptr::null(),
                &mut key,
                &mut disp,
            );
            if rc != ERROR_SUCCESS {
                return Err(format!("打不开注册表键（错误码 {rc}）"));
            }

            let data = wide(command);
            let rc = RegSetValueExW(
                key,
                wide(&self.name).as_ptr(),
                0,
                REG_SZ,
                data.as_ptr() as *const u8,
                (data.len() * 2) as u32,
            );
            RegCloseKey(key);
            if rc != ERROR_SUCCESS {
                return Err(format!("写注册表值失败（错误码 {rc}）"));
            }
            Ok(())
        }
    }

    /// 关闭。本来就没开也算成功（幂等）。
    pub fn disable(&self) -> Result<(), String> {
        unsafe {
            let mut key: HKEY = std::ptr::null_mut();
            let rc = RegOpenKeyExW(
                HKEY_CURRENT_USER,
                wide(&self.key_path).as_ptr(),
                0,
                KEY_SET_VALUE,
                &mut key,
            );
            if rc == ERROR_FILE_NOT_FOUND {
                return Ok(()); // 键都没有，自然没开
            }
            if rc != ERROR_SUCCESS {
                return Err(format!("打不开注册表键（错误码 {rc}）"));
            }

            let rc = RegDeleteValueW(key, wide(&self.name).as_ptr());
            RegCloseKey(key);
            if rc != ERROR_SUCCESS && rc != ERROR_FILE_NOT_FOUND {
                return Err(format!("删注册表值失败（错误码 {rc}）"));
            }
            Ok(())
        }
    }

    /// 删掉整个键。**只给单测清理沙箱键用**，程序本身从不删 `Run` 键。
    #[cfg(test)]
    pub fn remove_key_tree(&self) -> Result<(), String> {
        unsafe {
            let rc = RegDeleteKeyW(HKEY_CURRENT_USER, wide(&self.key_path).as_ptr());
            if rc == ERROR_SUCCESS || rc == ERROR_FILE_NOT_FOUND {
                Ok(())
            } else {
                Err(format!("删注册表键失败（错误码 {rc}）"))
            }
        }
    }
}

/// 开机要跑的命令：exe 的全路径，**带引号**。
///
/// 引号不能省 —— 路径里有空格（比如 `C:\Users\Me\App Files\...`）时，
/// 不带引号会被开机启动器按空格切成两段，然后启动失败。
///
/// release 版是 GUI 子系统，不需要额外参数，也不用像 Python 版那样特意挑
/// `pythonw.exe` 来躲黑框。
pub fn autostart_command(exe: &std::path::Path) -> String {
    format!("\"{}\"", exe.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox(tag: &str) -> RunEntry {
        RunEntry {
            key_path: format!("{}\\{}-{tag}", SANDBOX_ROOT, std::process::id()),
            name: "TestEntry".to_string(),
        }
    }

    /// 沙箱键的父容器。`RegDeleteKeyW` 只能删空键，所以叶子删完后再试着删它 ——
    /// 不清的话 `HKCU\Software` 下会永久留一个空壳（每条用例都会留一个）。
    const SANDBOX_ROOT: &str = "Software\\TextSnapTest";

    fn drop_sandbox_root() {
        let root = RunEntry {
            key_path: SANDBOX_ROOT.to_string(),
            name: String::new(),
        };
        // 还有别的用例在跑（键非空）时会返回 ERROR_KEY_HAS_CHILDREN，忽略即可
        let _ = root.remove_key_tree();
    }

    #[test]
    fn command_is_a_quoted_exe_path() {
        // 路径里有空格时必须带引号，否则开机时会按空格切开
        let cmd = autostart_command(std::path::Path::new(
            r"C:\Users\Mr.Zhang\App Files\text-snap.exe",
        ));
        assert_eq!(cmd, r#""C:\Users\Mr.Zhang\App Files\text-snap.exe""#);
        assert_eq!(
            autostart_command(std::path::Path::new(r"D:\x\text-snap.exe")),
            r#""D:\x\text-snap.exe""#
        );
    }

    #[test]
    fn app_entry_points_at_the_run_key() {
        let e = RunEntry::app();
        assert_eq!(e.name, "text-snap");
        assert_eq!(e.key_path, RUN_KEY);
    }

    /// 真的读写一次注册表，但只在**自己的沙箱键**下。
    #[test]
    #[ignore = "会写注册表（沙箱键）。跑法：cargo test --bin text-snap -- --ignored"]
    fn registry_round_trip_in_a_sandbox_key() {
        let e = sandbox("roundtrip");
        // 先清干净，避免上次失败残留
        let _ = e.remove_key_tree();

        assert!(!e.is_enabled(), "新建的沙箱键应当是空的");
        assert_eq!(e.current(), None);

        e.enable(r#""C:\x\app.exe""#).expect("写入应当成功");
        assert!(e.is_enabled());
        assert_eq!(e.current().as_deref(), Some(r#""C:\x\app.exe""#));

        // 覆盖写（改命令）应当生效，而不是留下两条
        e.enable(r#""D:\y\app.exe""#).expect("改写应当成功");
        assert_eq!(e.current().as_deref(), Some(r#""D:\y\app.exe""#));

        e.disable().expect("删除应当成功");
        assert!(!e.is_enabled());
        // 幂等：再删一次也当成功
        e.disable().expect("重复删除应当幂等");

        e.remove_key_tree().expect("清理沙箱键");
        drop_sandbox_root();
    }
}
