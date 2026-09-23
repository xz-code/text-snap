//! 通过 Windows UI Automation（无障碍 API）读取当前焦点元素里的**选中文本**。
//!
//! 这里划三条硬约束，整个项目都不得违反：
//!   1. 不用 Windows 钩子（`SetWindowsHookEx` / 低级键盘钩子）—— 一个都不调。
//!   2. 不读进程内存（`ReadProcessMemory` 之类）—— 只走 UIA 客户端接口。
//!   3. 不碰剪贴板**读取** —— 不调 `OpenClipboard` / `GetClipboardData`，也不模拟 Ctrl+C。
//!      （写剪贴板是本项目刻意的产品功能，见 `main.rs` 的 `write_clipboard`；方向只出不进。）
//!
//! 读取策略：拿焦点元素，逐级向上找暴露了 `TextPattern` 的祖先，取其 selection。
//! 之所以要向上找，是因为在 VS Code 这类 Chromium 应用里，焦点元素常常是一个
//! 嵌套的输入节点，真正带文本的是它的祖先。

use serde::Serialize;
use uiautomation::patterns::UITextPattern;
use uiautomation::UIAutomation;
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};

/// 读取成功的结果，连同诊断信息一起给前端。
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub text: String,
    /// 焦点元素所在进程的可执行文件名，例如 `Code.exe`
    pub process: String,
    /// 焦点元素的 UIA 控件类型
    pub control: String,
    /// 焦点元素的窗口类名
    pub class: String,
}

/// 读取失败时返回的诊断信息 —— 前端要拿它告诉用户"为什么读不到"。
#[derive(Debug, Clone, Serialize)]
pub struct Failure {
    pub message: String,
    pub process: String,
    pub control: String,
    pub class: String,
}

/// 向上最多找几层祖先。
const MAX_ANCESTORS: usize = 6;

/// 常驻在一个专用线程里的 UIA 客户端。
///
/// `UIAutomation::new()` 会为当前线程 `CoInitializeEx(COINIT_MULTITHREADED)`，
/// 所以这个对象必须在同一个线程里创建和使用 —— 绝不能跨线程传递。
pub struct Reader {
    automation: Option<UIAutomation>,
}

impl Reader {
    pub fn new() -> Self {
        Self { automation: None }
    }

    /// 读取当前焦点元素中的选中文本。
    pub fn read(&mut self) -> Result<Snapshot, Failure> {
        let automation = match self.automation() {
            Ok(a) => a,
            Err(message) => {
                return Err(Failure {
                    message,
                    process: String::new(),
                    control: String::new(),
                    class: String::new(),
                })
            }
        };

        let element = match automation.get_focused_element() {
            Ok(e) => e,
            Err(e) => {
                return Err(Failure {
                    message: format!("取不到当前焦点元素：{e}"),
                    process: String::new(),
                    control: String::new(),
                    class: String::new(),
                })
            }
        };

        let pid = element.get_process_id().unwrap_or(0);
        if pid == std::process::id() {
            return Err(Failure {
                message: "焦点还在 text-snap 自己的窗口上。请先回到要划词的程序，再按一次快捷键。"
                    .to_string(),
                process: exe_name(pid),
                control: String::new(),
                class: String::new(),
            });
        }

        let process = exe_name(pid);
        let control = element
            .get_control_type()
            .map(|c| format!("{c:?}"))
            .unwrap_or_default();
        let class = element.get_classname().unwrap_or_default();

        // 逐级向上找带 TextPattern 的元素。
        let walker = automation.get_raw_view_walker().ok();
        let mut current = element;
        let mut saw_text_pattern = false;

        for _ in 0..MAX_ANCESTORS {
            match probe(&current) {
                Probe::Text(text) => {
                    return Ok(Snapshot {
                        text,
                        process,
                        control,
                        class,
                    })
                }
                // 有文本模式但选中是空的 —— 记下来，继续往上试：
                // 焦点元素是嵌套节点时，真正的选中可能挂在祖先上。
                Probe::Empty => saw_text_pattern = true,
                Probe::Missing => {}
            }
            match walker.as_ref().and_then(|w| w.get_parent(&current).ok()) {
                Some(parent) => current = parent,
                None => break,
            }
        }

        Err(Failure {
            message: explain(&control, &class, saw_text_pattern),
            process,
            control,
            class,
        })
    }

    fn automation(&mut self) -> Result<&UIAutomation, String> {
        if self.automation.is_none() {
            let instance =
                UIAutomation::new().map_err(|e| format!("初始化 UI Automation 客户端失败：{e}"))?;
            self.automation = Some(instance);
        }
        match self.automation.as_ref() {
            Some(a) => Ok(a),
            None => Err("UI Automation 客户端不可用".to_string()),
        }
    }
}

/// 单个元素上的探测结果。
///
/// 必须把「有文本模式但没选中」和「压根没有文本模式」分开 ——
/// 这两种情况给用户的提示完全不同，混成一句话会把人带偏。
enum Probe {
    /// 拿到了选中文本
    Text(String),
    /// 有 `TextPattern`，但选中是空的（多半是用户忘了选东西）
    Empty,
    /// 没有 `TextPattern`
    Missing,
}

/// 探测某个元素上的选中文本。
fn probe(element: &uiautomation::UIElement) -> Probe {
    let Ok(pattern) = element.get_pattern::<UITextPattern>() else {
        return Probe::Missing;
    };
    let Ok(ranges) = pattern.get_selection() else {
        // 有文本模式，只是取不出选区 —— 当作"空选中"处理，提示更贴近实际
        return Probe::Empty;
    };

    let mut buffer = String::new();
    for range in &ranges {
        let Ok(part) = range.get_text(-1) else {
            continue;
        };
        if part.is_empty() {
            continue;
        }
        if !buffer.is_empty() && !buffer.ends_with('\n') {
            buffer.push('\n');
        }
        buffer.push_str(&part);
    }

    if buffer.trim().is_empty() {
        Probe::Empty
    } else {
        Probe::Text(buffer)
    }
}

/// 把失败原因翻译成用户看得懂的话。三种情况的处理建议完全不同，不能糊成一句。
fn explain(control: &str, class: &str, saw_text_pattern: bool) -> String {
    let who = format!("控件类型 {control}，类名 {class}");

    if is_shell_chrome(class) {
        return format!(
            "焦点落在任务栏/托盘区域上了（{who}），那里没有可读的文本。\n\
             用托盘图标触发读取时很容易这样。\n\
             回到要划词的程序、重新选中一段文字，再按一次快捷键。"
        );
    }

    if saw_text_pattern {
        return format!(
            "焦点元素支持 UI Automation 文本模式，但当前没有选中任何文本（{who}）。\n\
             先选中一段文字，再按快捷键。"
        );
    }

    format!(
        "这个控件没有暴露 UI Automation 文本模式（TextPattern），读不到选中文本（{who}）。\n\
         常见原因：\n\
         · 该程序未开启无障碍支持 —— VS Code 需要 editor.accessibilitySupport 生效\n\
         · 目标程序以管理员身份运行，权限高于本程序（UIPI 拦截）\n\
         · 焦点不在可编辑的文本区域上"
    )
}

/// 焦点是不是掉到了任务栏 / 托盘这类 shell 皮肤上。
fn is_shell_chrome(class: &str) -> bool {
    const MARKERS: [&str; 4] = [
        "SystemTray",
        "Shell_TrayWnd",
        "TrayNotifyWnd",
        "TaskListThumbnailWnd",
    ];
    MARKERS.iter().any(|marker| class.contains(marker))
}

/// 由进程 ID 取可执行文件名（只要文件名，不要全路径）。
fn exe_name(pid: u32) -> String {
    if pid == 0 {
        return String::new();
    }
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return String::new();
        }
        let mut buffer = [0u16; 512];
        let mut length = buffer.len() as u32;
        // 第二个参数 PROCESS_NAME_WIN32 == 0，直接写字面量免得再引一个 feature。
        let ok = QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut length);
        windows_sys::Win32::Foundation::CloseHandle(handle);
        if ok == 0 {
            return String::new();
        }
        let full = String::from_utf16_lossy(&buffer[..length as usize]);
        full.rsplit(['\\', '/']).next().unwrap_or(&full).to_string()
    }
}
