//! 启动提示：一个**自绘**的仿 Windows 通知小窗。
//!
//! 为什么不用系统通知 API：
//! Windows 要求应用有带 AppUserModelID 的开始菜单快捷方式才能发 WinRT 通知。
//! 未打包的应用（直接跑 exe / `cargo run`）没有 AUMID，`tauri-winrt-notification`
//! 会回退到 PowerShell 的 AUMID —— 通知弹得出来，但"来源"显示成 Windows PowerShell，
//! 改不了（插件在 `target\debug` / `target\release` 路径下还**故意**跳过设置 AppID）。
//!
//! 自绘就没这些限制：图标、名字、文案、位置、停留时长全部由我们决定，
//! 而且不依赖任何系统注册机制。
//!
//! 生命周期：
//!   启动时 `create()` 建好并隐藏（避免显示时的冷启动延迟）
//!   → `show_startup()` 延迟 600ms 显示（给 webview 留加载时间）
//!   → 前端自己播滑入动画，4.4s 后播淡出
//!   → 后端 5s 时硬隐藏兜底
//!
//! 不抢焦点：`focusable(false)` + 从不调 `set_focus`，
//! 截图场景下弹提示不能把焦点从浏览器抢走。

use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTOPRIMARY,
};

use crate::log_line;

pub const TOAST_LABEL: &str = "toast";

/// 推送文案的事件名
const EVT_SHOW: &str = "toast-show";
/// 窗口逻辑尺寸
const WIDTH: f64 = 360.0;
const HEIGHT: f64 = 116.0;
/// 停留时长。
/// 注意：**淡出动画的起点（4.4s）写在 `toast.html` 里**，必须比这个值早，
/// 后端只负责到点硬隐藏兜底。
const VISIBLE_MS: u64 = 5000;
/// 距屏幕边缘的留白
const MARGIN: i32 = 18;

/// 启动时创建通知窗口（隐藏）。提前建好，显示时没有冷启动延迟。
pub fn create(app: &AppHandle) -> tauri::Result<()> {
    WebviewWindowBuilder::new(app, TOAST_LABEL, WebviewUrl::App("toast.html".into()))
        .title("text-snap 通知")
        .decorations(false) // 无边框：自己画圆角和排版
        .always_on_top(true) // 置顶，不被别的窗口盖住
        .skip_taskbar(true) // 不进任务栏，也不在 Alt+Tab 里出现
        .focusable(false) // 不抢焦点 —— 截图时弹它不能把焦点从浏览器拿走
        .focused(false) // 创建时也不获取焦点
        .resizable(false)
        .shadow(false) // 阴影交给 CSS 边框和圆角，系统阴影在无边框窗口上会带一圈黑边
        .inner_size(WIDTH, HEIGHT)
        .visible(false) // 先藏起来，定位好再显示
        .build()?;
    Ok(())
}

/// 显示启动提示（在独立线程里延迟执行，给 webview 留加载时间）。
pub fn show_startup(app: &AppHandle, hotkey: &str) {
    let handle = app.clone();
    // 线程要活过本函数，字符串必须拥有所有权（&str 活不过 spawn）
    let hotkey = hotkey.to_string();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(600));
        show(&handle, &hotkey);
    });
}

/// 显示通知。`hotkey` 传配置里"读取选中文本"的快捷键，用于文案提示。
pub fn show(app: &AppHandle, hotkey: &str) {
    let Some(window) = app.get_webview_window(TOAST_LABEL) else {
        log_line("找不到通知窗口");
        return;
    };

    // 先推文案再显示 —— 和划词弹窗同一个顺序，避免看到上一轮的残影
    if let Err(err) = window.emit(
        EVT_SHOW,
        serde_json::json!({
            "app_name": "text-snap 划词助手",
            "title": "已在后台运行",
            "body": format!("按 {hotkey} 读取选中文本，右键托盘图标打开菜单。"),
        }),
    ) {
        log_line(&format!("向通知窗口推送文案失败：{err}"));
    }

    position_bottom_right(&window);
    let _ = window.show();
    log_line("已弹出启动提示（自绘通知窗口）");

    // 后端硬隐藏兜底：就算前端动画没跑完，到点也一定收起来
    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(VISIBLE_MS));
        if let Some(window) = handle.get_webview_window(TOAST_LABEL) {
            let _ = window.hide();
        }
    });
}

/// 贴到主显示器工作区的右下角（已避开任务栏）。
///
/// 用**物理像素**定位：窗口的 `outer_size()` 也是物理像素，两边一致就不会偏。
fn position_bottom_right(window: &WebviewWindow) {
    let Some((left, top, width, height)) = primary_work_area() else {
        return;
    };
    let Ok(size) = window.outer_size() else {
        return;
    };

    let x = left + width - size.width as i32 - MARGIN;
    let y = top + height - size.height as i32 - MARGIN;
    let _ = window.set_position(PhysicalPosition::new(x, y));
}

/// 主显示器的工作区（已避开任务栏）：(left, top, width, height)。
///
/// 用 (0,0) + `MONITOR_DEFAULTTOPRIMARY` 拿主屏 —— 启动提示固定贴主屏右下角，
/// 不跟随鼠标（那是划词弹窗的需求）。
fn primary_work_area() -> Option<(i32, i32, i32, i32)> {
    unsafe {
        let monitor = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        if monitor.is_null() {
            return None;
        }
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(monitor, &mut info) == 0 {
            return None;
        }
        let work = info.rcWork;
        Some((
            work.left,
            work.top,
            work.right - work.left,
            work.bottom - work.top,
        ))
    }
}
