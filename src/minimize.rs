//! 编辑器窗口的「一键清场」：把 VS Code / Visual Studio 的窗口收起来，再切回来。
//!
//! 用途：**截图前把编辑器收起来。** 窗口最小化后不参与屏幕合成，
//! 截别的窗口时就不会被它干扰。
//!
//! ── 实现上的关键点（踩过才知道）────────────────────────────
//!
//! **绝对不能按进程名无脑最小化。**
//! VS Code 是 Electron 多进程应用 —— 实测本机同时跑着 12 个 `Code.exe`，
//! 其中**只有 1 个**拥有可见的顶层窗口，其余全是辅助进程：
//! 不可见的 `Chrome_WidgetWin_0`、`crashpad_SessionEndWatcher`、
//! `Base_PowerMessageWindow`、`MSCTFIME UI`、`IME`…… 它们的窗口都是隐藏的。
//!
//! 所以判断条件是三个**同时**成立：
//!   1. `IsWindowVisible` 为真
//!   2. 窗口有标题（`GetWindowTextW` 长度 > 0）
//!   3. 所属进程名在目标名单里
//!
//! 少任何一条都会去动不该动的窗口。
//!
//! ── 状态怎么判断 ────────────────────────────────────────────
//! 不留内部状态，直接看**当前所有目标窗口是不是都已经最小化**：
//!   · 只要还有哪怕一个没最小化 → 执行最小化
//!   · 全部都最小化了         → 执行还原
//! 这样即使你手动最小化/还原过其中一个，行为也依然符合直觉。

use windows_sys::core::BOOL;
use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM, TRUE};
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible, ShowWindow,
    SW_SHOWMINNOACTIVE, SW_SHOWNOACTIVATE,
};

/// 目标进程名，小写比较。
///
/// `Code.exe` = VS Code，`devenv.exe` = Visual Studio。
/// 需要加别的（Cursor、Windsurf、内部 IDE）时往这里加就行。
const TARGET_PROCESSES: [&str; 2] = ["code.exe", "devenv.exe"];

/// 切换动作的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// 已把编辑器最小化
    Minimized,
    /// 已把编辑器还原
    Restored,
    /// 一个目标窗口都没找到
    Nothing,
}

/// 切换的完整结果。
///
/// `failed` 专门用来暴露一种**最隐蔽**的失败：`ShowWindow` 对
/// **以管理员身份运行**的目标窗口会被 UIPI（用户界面特权隔离）拦下，
/// **不返回任何错误**，窗口就是没反应。只能靠执行后的实际状态复核出来。
#[derive(Debug, Clone, Copy)]
pub struct ToggleResult {
    pub action: Action,
    /// 参与本次切换的窗口数
    pub count: usize,
    /// 执行后仍未达成目标的窗口数（>0 几乎总是因为目标程序提权了）
    pub failed: usize,
}

/// 最小化 ⇄ 还原，一键切换。
pub fn toggle() -> ToggleResult {
    let handles = target_windows();
    if handles.is_empty() {
        return ToggleResult {
            action: Action::Nothing,
            count: 0,
            failed: 0,
        };
    }

    // 只要有一个还开着（没最小化），就认为当前是"显示中"，执行最小化
    let showing = handles
        .iter()
        .filter(|hwnd| unsafe { IsIconic(**hwnd) } == 0)
        .count();

    if showing > 0 {
        for hwnd in &handles {
            // **必须用 SW_SHOWMINNOACTIVE 而不是 SW_MINIMIZE。**
            // SW_MINIMIZE 的文档行为是"最小化并激活 z 序里的下一个窗口"——
            // 两个编辑器接连最小化时前台会易主一次，视觉上就是"闪一下"。
            // NOACTIVE 变体不动前台：当前正在用的窗口（比如要截图的浏览器）保持不动。
            unsafe {
                ShowWindow(*hwnd, SW_SHOWMINNOACTIVE);
            }
        }
        let failed = handles
            .iter()
            .filter(|hwnd| unsafe { IsIconic(**hwnd) } == 0)
            .count();
        ToggleResult {
            action: Action::Minimized,
            count: handles.len(),
            failed,
        }
    } else {
        for hwnd in &handles {
            restore_no_activate(*hwnd);
        }
        let failed = handles
            .iter()
            .filter(|hwnd| unsafe { IsIconic(**hwnd) } != 0)
            .count();
        ToggleResult {
            action: Action::Restored,
            count: handles.len(),
            failed,
        }
    }
}

/// 还原窗口到**它被最小化之前的状态**，且不抢前台。
///
/// 这里踩过两个坑，都是探针实测出来的，别再改回去：
///
/// **坑①：不能用 `SetWindowPlacement(SW_SHOWNOACTIVATE)`。**
/// 它确实不抢前台，但它会照 `rcNormalPosition` 摆放窗口 ——
/// 最大化状态直接丢掉，还原出来全是普通窗口。
///
/// **坑②：不能用 `SW_SHOWMAXIMIZED`。** 状态是对的，但它的文档原文就是
/// "Activates the window"，多个最大化窗口接连还原时会互相抢前台 ⇒ 闪一下。
///
/// **正解是 `ShowWindow(SW_SHOWNOACTIVATE)`**：
///   · 它走的是系统的"从最小化还原"流程，会尊重
///     `WPF_RESTORETOMAXIMIZED`（系统记的"最小化前是最大化"）⇒ 状态正确（实测 3/3）
///   · 文档语义是 "similar to SW_SHOWNORMAL, except that the window is **not activated**"
///     ⇒ 不抢前台
///
/// 一个调用同时满足两个要求，所以之前那套 Get/SetWindowPlacement 全都不要了。
fn restore_no_activate(hwnd: HWND) {
    unsafe {
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
}

/// 收集所有属于目标进程的、可见且有标题的顶层窗口。
pub fn target_windows() -> Vec<HWND> {
    let mut handles: Vec<HWND> = Vec::new();
    unsafe {
        // 返回值是被 EnumWindows 提前中断（回调返回 FALSE）才为 0，这里回调恒返回 TRUE
        let _ = EnumWindows(Some(collect_proc), &mut handles as *mut _ as LPARAM);
    }
    handles
}

unsafe extern "system" fn collect_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // edition 2024：unsafe fn 体内必须显式写 unsafe 块
    unsafe {
        let list = &mut *(lparam as *mut Vec<HWND>);

        // 条件 1：可见 —— 这一步就滤掉了绝大部分 Electron 辅助窗口
        if IsWindowVisible(hwnd) == 0 {
            return TRUE;
        }

        // 条件 2：有标题 —— 再滤掉隐藏的、无标题的容器窗口
        let mut title = [0u16; 256];
        let title_len = GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32);
        if title_len <= 0 {
            return TRUE;
        }

        // 条件 3：进程名命中
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid != 0 && process_matches(pid) {
            list.push(hwnd);
        }
    }
    TRUE
}

/// 该进程是不是我们要处理的目标。
fn process_matches(pid: u32) -> bool {
    unsafe {
        // 只需要读进程信息，用最小权限；权限不足时会返回空句柄，视为不匹配
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }

        let mut buf = [0u16; 512];
        let mut size = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut size);
        let _ = CloseHandle(handle);

        if ok == 0 {
            return false;
        }

        let path = String::from_utf16_lossy(&buf[..size as usize]);
        let name = path
            .rsplit(['\\', '/'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        TARGET_PROCESSES.contains(&name.as_str())
    }
}
