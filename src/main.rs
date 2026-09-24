// ═══════════════════════════════════════════════════════════════════════════
//  text-snap · MVP1
//  Windows 全局划词助手：托盘常驻 + 全局快捷键 + UIAutomation 读选中文本 + 弹窗展示
//
//  本期只做 MVP1：托盘 / 快捷键 / UIA 读取 / 弹窗展示 / 复制到剪贴板 / 保存 txt。
//  DeepSeek 对话属于 MVP2：打开 Cargo.toml 的 `ai` feature 后在此接入。
//
//  三条硬约束（贯穿全项目）：
//    · 不用 Windows 钩子
//    · 不读进程内存
//    · 不用剪贴板**读取**选中文本 —— 读取永远只走 UIA。
//      写剪贴板是刻意保留的功能（复制按钮 + 自动复制），不在此约束之内。
// ═══════════════════════════════════════════════════════════════════════════

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod minimize;
mod selection;
mod toast;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, RunEvent, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, Window, WindowEvent,
};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_dialog::{DialogExt, FilePath};
use tauri_plugin_global_shortcut::{
    Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState,
};
use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;

/// 弹窗的窗口标签，能力文件里按这个标签授权。
const WIN_LABEL: &str = "main";
/// 设置窗口的标签。按需创建，关掉即销毁。
const SETTINGS_LABEL: &str = "settings";
/// 唯一的推送事件：payload 是 `{ event: "ready" | "failed", payload: {...} }`。
/// 用单一事件而不是两个，是为了让前端在加载完成后能一次性取回"最后一次结果"，
/// 避免弹窗刚建好、页面还没加载完就被按键命中导致内容丢失。
const EVT_UPDATE: &str = "selection://update";
/// 设置保存成功后广播，让已经建好的弹窗刷新它显示的快捷键
const EVT_SETTINGS_APPLIED: &str = "settings://applied";
/// 弹窗刚展示后的这段时间不响应「失焦隐藏」，躲开 show() 过程中的焦点抖动
const FOCUS_GRACE_MS: u64 = 300;

struct AppState {
    config: Mutex<config::Config>,
    /// 往 UIA 工作线程投递一次「读取」请求
    reader_tx: Sender<()>,
    /// 弹窗最近一次展示的时间戳（毫秒）
    shown_at: AtomicU64,
    /// 最后一次读取结果，供前端补拉
    last: Mutex<Option<serde_json::Value>>,
}

// ───────────────────────────── 入口 ─────────────────────────────

fn main() {
    let result = tauri::Builder::default()
        // 单实例必须第一个注册，官方要求
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            log_line("检测到重复启动，唤起已有实例");
            if let Some(window) = app.get_webview_window(WIN_LABEL) {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        // ⚠️ 这里**故意不调 `.with_handler()`**。
        //
        // 插件文档写着 with_handler 的 handler 会 "be triggered for any and all shortcuts"，
        // 而分发逻辑里它是**无条件再调一次**，不是"没有专属 handler 时才兜底"：
        //
        //     if let Some(h) = &shortcut.handler { h(...) }   // 按键专属
        //     if let Some(h) = &handler         { h(...) }   // 全局：又一次
        //
        // 两条键都用 on_shortcut 各挂各的，全局 handler 留空，
        // 才不会出现"按最小化键结果也触发了一次读取"这种事。
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            hide_popup,
            copy_text,
            save_text,
            last_selection,
            ui_info,
            open_settings,
            apply_settings,
            pick_folder,
            open_dir,
            hide_toast
        ])
        .setup(setup)
        .on_window_event(on_window_event)
        .build(tauri::generate_context!());

    let app = match result {
        Ok(app) => app,
        Err(err) => {
            log_line(&format!("启动失败：{err}"));
            std::process::exit(1);
        }
    };

    app.run(|_app, event| {
        // 托盘常驻应用：弹窗关掉不等于退出程序。
        //
        // 但这里有个坑：ExitRequested 的 `code` 字段区分了两种来源 ——
        //   `None`     = 用户交互（最后一个窗口被关掉）
        //   `Some(_)`  = 程序主动调用 AppHandle::exit()，也就是托盘菜单的「退出」
        // 所以只能拦 None，无条件拦会把「退出」一起拦掉，表现就是点了退出毫无反应。
        if let RunEvent::ExitRequested { code, api, .. } = event {
            if code.is_none() {
                api.prevent_exit();
            }
        }
    });
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let handle = app.handle().clone();
    let cfg = config::load();

    log_line(&format!(
        "── 启动 ──  配置 {}  快捷键 {}",
        config::path().display(),
        cfg.hotkey
    ));

    // ① UIA 读取线程：先建好，托盘和快捷键都要往它投递
    let (reader_tx, reader_rx) = mpsc::channel::<()>();
    spawn_reader(handle.clone(), reader_rx);

    app.manage(AppState {
        config: Mutex::new(cfg.clone()),
        reader_tx,
        shown_at: AtomicU64::new(0),
        last: Mutex::new(None),
    });

    // ② 弹窗：提前建好并隐藏。不在按键时才创建，因为 WebView2 冷启动要几百毫秒，
    //    而「按一下立刻出现」是这个工具的核心体验。
    WebviewWindowBuilder::new(app, WIN_LABEL, WebviewUrl::App("index.html".into()))
        .title("text-snap")
        .inner_size(560.0, 440.0)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .visible(false)
        .focused(false)
        .shadow(true)
        .build()?;

    // ③ 系统托盘
    build_tray(&handle)?;

    // ④ 全局快捷键
    match register_shortcuts(&handle, &cfg) {
        Ok(()) => log_line(&format!(
            "全局快捷键已注册：读取 [{}]，最小化/还原 [{}]",
            cfg.hotkey,
            cfg.minimize_spec().unwrap_or("未设置")
        )),
        Err(err) => log_line(&format!("快捷键注册失败：{err}")),
    }

    // ⑤ 通知窗口 + 启动提示
    //
    // create 必须先于 show_startup：窗口先存在，后台的显示线程才有东西可显示。
    // （之前两步顺序是反的，靠线程里 sleep 600ms 隐式保证 —— 已改为显式顺序。）
    if let Err(err) = toast::create(&handle) {
        log_line(&format!(
            "创建通知窗口失败：{err}（本次运行不显示启动提示，其余功能不受影响）"
        ));
    } else if cfg.show_startup_toast {
        toast::show_startup(&handle, &cfg.hotkey);
    }

    Ok(())
}

/// 收起启动提示窗口。`toast.html` 的关闭按钮会调它。
#[tauri::command]
fn hide_toast(app: AppHandle) {
    if let Some(window) = app.get_webview_window(toast::TOAST_LABEL) {
        let _ = window.hide();
    }
}

// ─────────────────────── 全局快捷键 ───────────────────────

/// 往工作线程投递一次读取请求。
fn trigger_read(app: &AppHandle) {
    if let Some(state) = app.try_state::<AppState>() {
        if state.reader_tx.send(()).is_err() {
            log_line("UIA 工作线程已退出，读取请求被丢弃");
        }
    }
}

/// 最小化 ⇄ 还原编辑器，并写日志。托盘菜单和全局快捷键共用这一条路径。
fn toggle_editors() {
    let result = minimize::toggle();

    match result.action {
        minimize::Action::Nothing => {
            log_line("没有检测到 VS Code / Visual Studio 窗口");
            return;
        }
        _ => {}
    }

    let what = match result.action {
        minimize::Action::Minimized => "已最小化",
        minimize::Action::Restored => "已还原",
        _ => return,
    };

    // UIPI 拦截不会报错，只能靠复核发现 —— 这条日志是排障的关键线索
    let suffix = if result.failed > 0 {
        format!(
            "（{}/{} 个没成功：目标程序可能以管理员运行，本程序也需要管理员权限才能控制它）",
            result.failed, result.count
        )
    } else {
        String::new()
    };

    log_line(&format!(
        "{} {} 个编辑器窗口{}",
        what, result.count, suffix
    ));
}

/// 按配置注册全部全局快捷键。**任何一步失败都返回错误，由调用方负责回滚。**
///
/// 两条快捷键**都用 `on_shortcut()` 各挂各的 handler**，不用 `with_handler` 的全局 handler ——
/// 原因见 `main()` 里注册插件那段的注释：全局 handler 对**所有**快捷键都会再触发一次，
/// 叠在一起会导致「按最小化键顺带触发一次读取」。
///
/// 这样还有一个好处：不需要比较 `Shortcut` 的相等性去判断"刚按的是哪个键"。
/// 那个比较不可靠 —— 注册时构造的对象和回调里拿到的对象未必能直接比。
fn register_shortcuts(app: &AppHandle, cfg: &config::Config) -> Result<(), String> {
    let gs = app.global_shortcut();
    gs.unregister_all().ok();

    let read = parse_shortcut(&cfg.hotkey)
        .map_err(|err| format!("读取快捷键「{}」无效：{err}", cfg.hotkey))?;
    gs.on_shortcut(read, |app, _shortcut, event| {
        if event.state() == ShortcutState::Pressed {
            trigger_read(app);
        }
    })
    .map_err(|err| format!("「{}」注册失败，可能已被别的程序占用：{err}", cfg.hotkey))?;

    if let Some(spec) = cfg.minimize_spec() {
        let shortcut = parse_shortcut(spec)
            .map_err(|err| format!("最小化/还原快捷键「{spec}」无效：{err}"))?;
        gs.on_shortcut(shortcut, |_app, _shortcut, event| {
            if event.state() == ShortcutState::Pressed {
                toggle_editors();
            }
        })
        .map_err(|err| format!("「{spec}」注册失败，可能已被别的程序占用：{err}"))?;
    }

    Ok(())
}

/// 把 `Ctrl+Shift+A` 这样的写法解析成快捷键对象。
fn parse_shortcut(spec: &str) -> anyhow::Result<Shortcut> {
    let mut modifiers = Modifiers::empty();
    let mut key: Option<Code> = None;

    for raw in spec.split('+') {
        let part = raw.trim();
        if part.is_empty() {
            continue;
        }
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => modifiers |= Modifiers::CONTROL,
            "shift" => modifiers |= Modifiers::SHIFT,
            "alt" => modifiers |= Modifiers::ALT,
            "win" | "super" | "meta" | "cmd" => modifiers |= Modifiers::SUPER,
            other => {
                if key.is_some() {
                    anyhow::bail!("快捷键里出现了多个主键：{spec}");
                }
                key = Some(parse_code(other)?);
            }
        }
    }

    let key = key.ok_or_else(|| anyhow::anyhow!("快捷键缺少主键，例如 Ctrl+Shift+A"))?;
    if modifiers.is_empty() {
        anyhow::bail!("快捷键至少要有一个修饰键（Ctrl / Shift / Alt / Win），否则会抢走正常输入");
    }
    Ok(Shortcut::new(Some(modifiers), key))
}

/// 按键名 → 物理键位码。
fn parse_code(name: &str) -> anyhow::Result<Code> {
    let upper = name.to_ascii_uppercase();
    let code = match upper.as_str() {
        "A" => Code::KeyA,
        "B" => Code::KeyB,
        "C" => Code::KeyC,
        "D" => Code::KeyD,
        "E" => Code::KeyE,
        "F" => Code::KeyF,
        "G" => Code::KeyG,
        "H" => Code::KeyH,
        "I" => Code::KeyI,
        "J" => Code::KeyJ,
        "K" => Code::KeyK,
        "L" => Code::KeyL,
        "M" => Code::KeyM,
        "N" => Code::KeyN,
        "O" => Code::KeyO,
        "P" => Code::KeyP,
        "Q" => Code::KeyQ,
        "R" => Code::KeyR,
        "S" => Code::KeyS,
        "T" => Code::KeyT,
        "U" => Code::KeyU,
        "V" => Code::KeyV,
        "W" => Code::KeyW,
        "X" => Code::KeyX,
        "Y" => Code::KeyY,
        "Z" => Code::KeyZ,
        "0" => Code::Digit0,
        "1" => Code::Digit1,
        "2" => Code::Digit2,
        "3" => Code::Digit3,
        "4" => Code::Digit4,
        "5" => Code::Digit5,
        "6" => Code::Digit6,
        "7" => Code::Digit7,
        "8" => Code::Digit8,
        "9" => Code::Digit9,
        "F1" => Code::F1,
        "F2" => Code::F2,
        "F3" => Code::F3,
        "F4" => Code::F4,
        "F5" => Code::F5,
        "F6" => Code::F6,
        "F7" => Code::F7,
        "F8" => Code::F8,
        "F9" => Code::F9,
        "F10" => Code::F10,
        "F11" => Code::F11,
        "F12" => Code::F12,
        "SPACE" => Code::Space,
        "ENTER" | "RETURN" => Code::Enter,
        "TAB" => Code::Tab,
        "ESC" | "ESCAPE" => Code::Escape,
        "MINUS" | "-" => Code::Minus,
        "EQUAL" | "=" => Code::Equal,
        "COMMA" | "," => Code::Comma,
        "PERIOD" | "." => Code::Period,
        "SLASH" | "/" => Code::Slash,
        "SEMICOLON" | ";" => Code::Semicolon,
        "QUOTE" | "'" => Code::Quote,
        "BACKQUOTE" | "`" => Code::Backquote,
        "BRACKETLEFT" | "[" => Code::BracketLeft,
        "BRACKETRIGHT" | "]" => Code::BracketRight,
        other => anyhow::bail!("认不出的按键：{other}"),
    };
    Ok(code)
}

// ─────────────────────────── 托盘 ───────────────────────────

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let pick = MenuItem::with_id(app, "pick", "读取当前选中文本", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "设置…", true, None::<&str>)?;
    let open_dir = MenuItem::with_id(app, "open_dir", "打开保存目录", true, None::<&str>)?;
    let clear = MenuItem::with_id(
        app,
        "clear_editors",
        "最小化/还原编辑器（截图前清场）",
        true,
        None::<&str>,
    )?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[&pick, &settings, &open_dir, &separator, &clear, &quit],
    )?;

    let mut builder = TrayIconBuilder::with_id("text-snap-tray")
        .menu(&menu)
        // 左键单击直接读取，右键才出菜单 —— 托盘工具的常规手感
        .show_menu_on_left_click(false)
        .tooltip("text-snap · 划词助手")
        .on_menu_event(on_menu)
        .on_tray_icon_event(on_tray_icon);

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }

    builder.build(app)?;
    Ok(())
}

fn on_tray_icon(tray: &TrayIcon, event: TrayIconEvent) {
    if let TrayIconEvent::Click {
        button: MouseButton::Left,
        button_state: MouseButtonState::Up,
        ..
    } = event
    {
        trigger_read(tray.app_handle());
    }
}

fn on_menu(app: &AppHandle, event: MenuEvent) {
    match event.id().as_ref() {
        "pick" => trigger_read(app),

        "settings" => {
            if let Err(err) = show_settings(app) {
                log_line(&format!("打开设置窗口失败：{err}"));
            }
        }

        "open_dir" => {
            let cfg = current_config(app);
            let document_dir = app.path().document_dir().ok();
            let dir = config::resolve_save_dir(&cfg, document_dir.as_deref());
            let _ = std::fs::create_dir_all(&dir);
            if let Err(err) = spawn_explorer(&dir) {
                log_line(&format!("打开目录 {} 失败：{err}", dir.display()));
            }
        }

        // 切换式：有窗口开着就收起来，全收起来了就还原
        "clear_editors" => toggle_editors(),

        "quit" => app.exit(0),
        _ => {}
    }
}

fn current_config(app: &AppHandle) -> config::Config {
    app.try_state::<AppState>()
        .map(|state| state.config.lock().unwrap().clone())
        .unwrap_or_default()
}

fn spawn_explorer(path: &std::path::Path) -> std::io::Result<()> {
    std::process::Command::new("explorer").arg(path).spawn().map(|_| ())
}

/// 打开设置窗口。已经开着就只做唤起，不重复创建。
fn show_settings(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(SETTINGS_LABEL) {
        let _ = window.show();
        let _ = window.set_focus();
        return Ok(());
    }

    WebviewWindowBuilder::new(app, SETTINGS_LABEL, WebviewUrl::App("settings.html".into()))
        .title("text-snap 设置")
        .inner_size(540.0, 400.0)
        .min_inner_size(480.0, 360.0)
        .resizable(true)
        .center()
        .build()?;

    Ok(())
}

// ──────────────────────── 弹窗行为 ────────────────────────

/// 失焦即隐藏。刚展示的一小段时间内忽略，避免 show() 过程中的焦点抖动把窗口立刻收掉。
///
/// **只对划词弹窗生效**。设置窗口是常规窗口，跟着一起收掉就没法用了 ——
/// 这个 label 判断不能省。
fn on_window_event(window: &Window, event: &WindowEvent) {
    if window.label() != WIN_LABEL {
        return;
    }
    if let WindowEvent::Focused(false) = event {
        let recently_shown = window
            .try_state::<AppState>()
            .map(|state| now_ms().saturating_sub(state.shown_at.load(Ordering::Relaxed)) < FOCUS_GRACE_MS)
            .unwrap_or(false);
        if !recently_shown && window.is_visible().unwrap_or(false) {
            let _ = window.hide();
        }
    }
}

/// 把弹窗摆到鼠标附近，并做屏幕边缘避让。
fn place_near_cursor(window: &WebviewWindow) {
    let (cursor_x, cursor_y) = cursor_position();
    let (width, height) = match window.outer_size() {
        Ok(size) if size.width > 0 && size.height > 0 => (size.width as i32, size.height as i32),
        _ => (560, 440),
    };

    const GAP: i32 = 8;
    const OFFSET_Y: i32 = 18;

    let mut x = cursor_x - width / 2;
    let mut y = cursor_y + OFFSET_Y;

    if let Some((left, top, w, h)) = monitor_work_area(cursor_x, cursor_y) {
        if x < left + GAP {
            x = left + GAP;
        }
        if x + width > left + w - GAP {
            x = left + w - width - GAP;
        }
        // 光标下面放不下就翻到上面去
        if y + height > top + h - GAP {
            y = cursor_y - height - GAP;
        }
        if y < top + GAP {
            y = top + GAP;
        }
    }

    let _ = window.set_position(PhysicalPosition::new(x, y));
}

fn cursor_position() -> (i32, i32) {
    let mut point = POINT { x: 0, y: 0 };
    unsafe {
        GetCursorPos(&mut point);
    }
    (point.x, point.y)
}

/// 光标所在屏幕的工作区（已避开任务栏）：(left, top, width, height)
fn monitor_work_area(x: i32, y: i32) -> Option<(i32, i32, i32, i32)> {
    unsafe {
        let monitor = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
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

/// 内容先存下来、推给前端，再显示窗口 —— 避免看到上一轮的残影，也顺带解决补拉。
///
/// `clipboard_text` 有值时会**无条件**在弹窗出现前写进剪贴板，用户关掉弹窗就能直接 Ctrl+V。
/// 写失败不影响弹窗展示，只记日志 + 界面上不显示「已复制」徽标。
fn show_popup_with<T: serde::Serialize>(
    app: &AppHandle,
    kind: &str,
    payload: T,
    clipboard_text: Option<String>,
) {
    let Some(window) = app.get_webview_window(WIN_LABEL) else {
        log_line("找不到弹窗窗口");
        return;
    };

    let mut auto_copied = false;
    if let Some(text) = clipboard_text {
        match write_clipboard(app, &text) {
            Ok(()) => auto_copied = true,
            Err(err) => log_line(&format!("自动复制失败：{err}")),
        }
    }

    let frame = serde_json::json!({
        "event": kind,
        "payload": payload,
        "auto_copied": auto_copied,
    });
    if let Some(state) = app.try_state::<AppState>() {
        if let Ok(mut slot) = state.last.lock() {
            *slot = Some(frame.clone());
        }
    }

    if let Err(err) = window.emit(EVT_UPDATE, frame) {
        log_line(&format!("向前端推送内容失败：{err}"));
    }

    place_near_cursor(&window);
    let _ = window.show();

    if let Some(state) = app.try_state::<AppState>() {
        state.shown_at.store(now_ms(), Ordering::Relaxed);
    }
    let _ = window.set_focus();
}

// ───────────────────── UIA 读取工作线程 ─────────────────────

/// UIA 调用会阻塞几十到几百毫秒，绝不能放在快捷键回调或 Tauri 事件循环线程上。
/// 这里开一条专用线程：UIA 客户端只创建一次反复用，按需读取。
fn spawn_reader(app: AppHandle, rx: Receiver<()>) {
    let spawned = std::thread::Builder::new()
        .name("text-snap-uia".to_string())
        .spawn(move || {
            // 必须在工作线程里创建：UIAutomation::new() 会在这个线程上初始化 COM(MTA)
            let mut reader = selection::Reader::new();

            while rx.recv().is_ok() {
                let Some(window) = app.get_webview_window(WIN_LABEL) else {
                    continue;
                };

                // 弹窗已经开着 → 这一次按键理解为「收起」
                if window.is_visible().unwrap_or(false) {
                    let _ = window.hide();
                    continue;
                }

                match reader.read() {
                    Ok(snapshot) => {
                        log_line(&format!(
                            "读取成功：{} / {} 字符",
                            snapshot.process,
                            snapshot.text.chars().count()
                        ));
                        let clipboard_text = snapshot.text.clone();
                        show_popup_with(&app, "ready", snapshot, Some(clipboard_text));
                    }
                    Err(failure) => {
                        log_line(&format!("读取失败：{}", failure.message));
                        show_popup_with(&app, "failed", failure, None);
                    }
                }
            }
        });

    if let Err(err) = spawned {
        log_line(&format!("创建 UIA 工作线程失败：{err}"));
    }
}

// ───────────────────────── 前端命令 ─────────────────────────

#[tauri::command]
fn hide_popup(window: WebviewWindow) {
    let _ = window.hide();
}

/// 把文本写进系统剪贴板。
///
/// 注意方向：**只写不读**。「不用剪贴板读取选中文本」那条约束依然成立，
/// 写作能力是刻意加的产品功能，不是读取的退路。
#[tauri::command]
fn copy_text(app: AppHandle, content: String) -> Result<(), String> {
    write_clipboard(&app, &content)
}

fn write_clipboard(app: &AppHandle, content: &str) -> Result<(), String> {
    if content.is_empty() {
        return Err("内容为空".to_string());
    }
    app.clipboard()
        .write_text(content)
        .map_err(|err| format!("写剪贴板失败：{err}"))
}

/// 前端加载完成后补拉最后一次结果，避免和「窗口刚建好」撞车丢内容。
#[tauri::command]
fn last_selection(state: tauri::State<'_, AppState>) -> Option<serde_json::Value> {
    state.last.lock().ok().and_then(|slot| slot.clone())
}

#[derive(serde::Serialize, Clone)]
struct UiInfo {
    hotkey: String,
    /// 最小化/还原编辑器的快捷键。空串 = 未设置（不注册）
    minimize_hotkey: String,
    /// 启动时是否发系统通知
    show_startup_toast: bool,
    /// 实际生效的保存目录（绝对路径）
    save_dir: String,
    /// 配置文件里写的原始值，空串 = 用默认目录。设置页的输入框显示这个，
    /// 这样"没自定义过"这个状态不会被悄悄改成显式路径。
    save_dir_raw: String,
    /// 是否用了自定义目录（false = 走默认的「文档\text-snap」）
    save_dir_custom: bool,
    /// 目录当前是否存在。不存在不算错，首次保存时会自动创建。
    save_dir_exists: bool,
}

impl UiInfo {
    fn build(app: &AppHandle, cfg: &config::Config) -> Self {
        let document_dir = app.path().document_dir().ok();
        let dir = config::resolve_save_dir(cfg, document_dir.as_deref());
        let raw = cfg.save_dir.clone().unwrap_or_default();
        Self {
            hotkey: cfg.hotkey.clone(),
            minimize_hotkey: cfg.minimize_spec().unwrap_or_default().to_string(),
            show_startup_toast: cfg.show_startup_toast,
            save_dir: dir.to_string_lossy().into_owned(),
            save_dir_raw: raw.clone(),
            save_dir_custom: !raw.trim().is_empty(),
            save_dir_exists: dir.is_dir(),
        }
    }
}

#[tauri::command]
fn ui_info(app: AppHandle, state: tauri::State<'_, AppState>) -> UiInfo {
    let cfg = current_config(&app);
    let _ = state; // 状态已经在 current_config 里取了，这里只是为了保持签名一致
    UiInfo::build(&app, &cfg)
}

#[tauri::command]
fn open_settings(app: AppHandle) -> Result<(), String> {
    show_settings(&app).map_err(|err| format!("打开设置窗口失败：{err}"))
}

/// 保存设置：校验 → 换快捷键 → 落盘。
///
/// 快捷键切换用的是「先全摘、再注册、失败回滚」的顺序：
/// 先注册新的再注销旧的看起来更优雅，但注册同一个键会被拒，
/// 判断"新旧是否同一个键"又要依赖 Shortcut 的相等性，不如这样稳。
#[tauri::command]
fn apply_settings(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    hotkey: String,
    minimize_hotkey: String,
    save_dir: String,
    show_startup_toast: bool,
) -> Result<UiInfo, String> {
    let mut cfg = current_config(&app);
    let previous = cfg.clone();

    cfg.hotkey = hotkey.trim().to_string();
    let minimize = minimize_hotkey.trim();
    cfg.minimize_hotkey = if minimize.is_empty() {
        None
    } else {
        Some(minimize.to_string())
    };
    cfg.show_startup_toast = show_startup_toast;
    let dir = save_dir.trim();
    cfg.save_dir = if dir.is_empty() {
        None
    } else {
        Some(dir.to_string())
    };

    // 校验：两条快捷键不能是同一个 —— 不拦的话，第二条注册会失败，
    // 而报错只说"已被别的程序占用"，用户根本想不到是和自己冲突。
    if let Some(min_spec) = cfg.minimize_spec() {
        if min_spec.eq_ignore_ascii_case(&cfg.hotkey) {
            return Err(
                "「读取选中文本」和「最小化/还原编辑器」不能使用同一个快捷键".to_string(),
            );
        }
    }

    // 先换快捷键：任何一步失败都整体回滚，别让用户两头落空
    if let Err(err) = register_shortcuts(&app, &cfg) {
        let _ = register_shortcuts(&app, &previous);
        return Err(err);
    }

    if let Err(err) = config::save(&cfg) {
        // 落盘失败也回滚快捷键，保持「配置文件 = 实际行为」
        let _ = register_shortcuts(&app, &previous);
        return Err(format!("写配置文件失败：{err}"));
    }

    if let Ok(mut guard) = state.config.lock() {
        *guard = cfg.clone();
    }
    log_line(&format!(
        "设置已更新：读取 [{}]，最小化/还原 [{}]，保存目录 [{}]，启动提示 [{}]",
        cfg.hotkey,
        cfg.minimize_spec().unwrap_or("<未设置>"),
        cfg.save_dir.as_deref().unwrap_or("<默认>"),
        if cfg.show_startup_toast { "开" } else { "关" }
    ));

    let info = UiInfo::build(&app, &cfg);
    let _ = app.emit(EVT_SETTINGS_APPLIED, info.clone());
    Ok(info)
}

/// 弹系统文件夹选择框。必须在非主线程跑 —— 阻塞版在主线程会死锁事件循环，
/// 所以这个命令是 `async`。
#[tauri::command]
async fn pick_folder(app: AppHandle) -> Option<String> {
    let picked = app
        .dialog()
        .file()
        .set_title("选择保存目录")
        .blocking_pick_folder();

    match picked {
        Some(FilePath::Path(path)) => Some(path.to_string_lossy().into_owned()),
        Some(FilePath::Url(url)) => url
            .to_file_path()
            .ok()
            .map(|path| path.to_string_lossy().into_owned()),
        None => None,
    }
}

/// 打开指定目录（设置页里用来当场验证路径对不对）。
#[tauri::command]
fn open_dir(path: String) -> Result<(), String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("路径为空".to_string());
    }
    let dir = std::path::PathBuf::from(trimmed);
    std::fs::create_dir_all(&dir).map_err(|err| format!("创建目录失败：{err}"))?;
    spawn_explorer(&dir).map_err(|err| format!("打开目录失败：{err}"))
}

/// 保存到本地文件。**替代剪贴板复制** —— 全程不碰剪贴板。
#[tauri::command]
fn save_text(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    content: String,
    kind: String,
) -> Result<String, String> {
    if content.trim().is_empty() {
        return Err("内容为空，没什么可保存的".to_string());
    }

    let cfg = state
        .config
        .lock()
        .map_err(|err| format!("配置读取失败：{err}"))?
        .clone();
    let document_dir = app.path().document_dir().ok();
    let dir = config::resolve_save_dir(&cfg, document_dir.as_deref());

    std::fs::create_dir_all(&dir).map_err(|err| format!("创建目录失败：{err}"))?;

    let path = dir.join(build_filename(&content, &kind, &dir));
    let bytes = if kind == "md" {
        content.into_bytes()
    } else {
        // .txt 带 UTF-8 BOM，免得某些编辑器里中文显示成乱码
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(content.as_bytes());
        bytes
    };

    std::fs::write(&path, bytes).map_err(|err| format!("写入失败：{err}"))?;
    log_line(&format!("已保存：{}", path.display()));
    Ok(path.to_string_lossy().into_owned())
}

/// 文件名规则：`20260922_211005_<正文首行前 20 字>.txt`，重名自动加序号。
fn build_filename(content: &str, kind: &str, dir: &std::path::Path) -> String {
    let snippet: String = content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_control() && !r#"\/:*?"<>|"#.contains(*c))
        .take(20)
        .collect();
    let snippet = snippet.trim();

    let stamp = timestamp();
    let base = if snippet.is_empty() {
        format!("{stamp}_snapshot")
    } else {
        format!("{stamp}_{snippet}")
    };
    let ext = if kind == "md" { "md" } else { "txt" };

    let mut candidate = format!("{base}.{ext}");
    let mut index = 2;
    while dir.join(&candidate).exists() {
        candidate = format!("{base}_{index}.{ext}");
        index += 1;
    }
    candidate
}

/// 本地时间戳，形如 `20260922_211005`。用 Win32 取，省掉一个日期库依赖。
fn timestamp() -> String {
    let mut st = unsafe { std::mem::zeroed::<windows_sys::Win32::Foundation::SYSTEMTIME>() };
    unsafe {
        GetLocalTime(&mut st);
    }
    format!(
        "{:04}{:02}{:02}_{:02}{:02}{:02}",
        st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond
    )
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 托盘程序没有控制台，日志落到 `%APPDATA%\text-snap\text-snap.log`。
fn log_line(message: &str) {
    let _ = std::fs::create_dir_all(config::dir());
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(config::dir().join("text-snap.log"))
    {
        use std::io::Write;
        let _ = writeln!(file, "[{}] {message}", timestamp());
    }
}
