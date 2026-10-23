# text-snap

Windows 托盘小工具：在任意窗口里选中文字，按一下快捷键，鼠标旁边就弹出一个小窗把内容显示出来。读取走 **Windows UI Automation 无障碍接口**，全程不碰剪贴板。可解决企业**加密软件限制**下，VSCode、Visual Studio 等编辑器内文本无法复制的问题。

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-Windows%2010%20%2F%2011-lightgrey.svg)](https://github.com/xz-code/text-snap)
[![Rust](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org/)
[![Tauri](https://img.shields.io/badge/tauri-2.x-24C8DB.svg)](https://tauri.app/)

## 这是什么？

- 定位：常驻托盘的划词速览小工具，面向在编辑器里频繁摘取代码片段的人。
- 解决的核心问题：不用 `Ctrl+C` 也能拿到选中文本 —— 不覆盖剪贴板原有内容、不抢占键盘，也不依赖剪贴板访问权限。

## 为什么存在？

- 背景 / 动机：大多数「抓取选中文字」的工具都是模拟 `Ctrl+C` 再读剪贴板。这条路的代价是覆盖剪贴板原内容、在剪贴板受限的环境（**企业 DLP 加密软件策略**、RDP / VDI、沙箱会话、安全桌面）直接失效、短暂抢占键盘，而且行为上和用户自己复制数据没有区别 —— 在会审计复制操作的环境里，这一点很关键。
- 另一条路：通过 **UI Automation** 向目标程序索取它的选中文本。这是 Windows 提供给读屏软件（NVDA、讲述人）的公开无障碍接口，程序自己把文本交出来，剪贴板全程不参与。不是绕安全机制，就是按它原本的设计在用。

## 如何安装和运行？

- 前置要求：
  - **编译**：Rust 工具链（验证于 `cargo` / `rustc` **1.98.1**，最低 1.85）；MSVC 链接器（Visual Studio Build Tools，勾选「使用 C++ 的桌面开发」）。
  - **运行**：WebView2 运行时（Win11 自带，Win10 装了 Edge 就有）；VC++ 2015–2022 可再发行组件 (x64)。
- 安装步骤：

  ```bash
  git clone https://github.com/xz-code/text-snap.git
  cd text-snap
  cargo run
  ```

  首次编译要拉 400+ 个 crate，大约 5～15 分钟。项目没有任何 Node.js 构建步骤，`cargo` 一条命令到底。
- 运行 / 启动：`cargo run` 启动后托盘区出现蓝色小图标。切到 VS Code（或 Visual Studio），选中一段文字，按 `Ctrl+Shift+A`，鼠标下方弹出小窗显示原文，内容同时已经进了剪贴板；`Esc` 收起后直接 `Ctrl+V`。

  ```bash
  cargo build --release
  ```

  产物是 `target\release\text-snap.exe`，**前端资源已内嵌**，拷这一个文件到任何 x64 Windows 机器就能跑。不要用 `cargo tauri build` —— 它会去下载 NSIS/WiX 打安装包，网络受限时基本拉不下来，而本项目只需要一个绿色免安装 exe。

## 当前状态

- 阶段：**开发中**。MVP1 已完成（托盘、全局快捷键、UIA 读取、无边框置顶弹窗、自动复制、保存 txt、设置窗口、单实例保护、编辑器一键最小化/还原、开机自启、静默模式）；MVP2 计划中（弹窗内接 AI 对话区，把原文发给大模型做总结 / 翻译 / 代码解释，结果可另存为 markdown，端点 `https://api.deepseek.com/v1/chat/completions`）。暂未打包、exe 未签名。
- 已知限制：
  - **目标程序权限更高时读不到。** UIA 的跨权限限制来自 UIPI，只拦「低完整性 → 高完整性」。用「以管理员身份运行」启动的 VS Code / Visual Studio 读不到，会明确报错，**不会**退回偷偷读剪贴板；同样地，管理员启动的编辑器也无法被「最小化/还原」控制。
  - **只保证 VS Code 和 Visual Studio。** 其余应用看运气：Chromium / Electron 系（Chrome、Edge）通常可以，Windows Terminal 这类基本不暴露 `TextPattern`。VS Code 若读不到，把它设置里的 `editor.accessibilitySupport` 改成 `on` 再试。
  - **从渲染后的网页 / 聊天窗口划词，拿不到换行和缩进。** 这是选区来源的固有属性 —— HTML 里的换行是排版效果、缩进是 CSS，不是文本里的 `\n` 和空格。从编辑器（VS Code / Visual Studio / 记事本）划词没有这个问题。
  - **没有剪贴板读取兜底。** 刻意为之：宁可读不到，也不留一条会破坏初衷的路。注意方向 —— **写**剪贴板是产品功能，**读**才是被排除的那条。
  - **写剪贴板也可能被拦。** 读取走 UIA 绕开了剪贴板管控，但把文本写回剪贴板同样经过系统剪贴板。策略若连写入也管，自动复制和复制按钮都会失败，程序会如实报错并记日志，不会静默假装成功；这种情况改用「保存到本地 txt」。
  - **全局快捷键可能和 IDE 冲突。** 本程序注册的是系统级热键，例如默认的 `Ctrl+Shift+D`（最小化/还原编辑器）会覆盖 VS Code「运行和调试」的同名快捷键，介意就在设置里换成别的组合。

## 核心技术

- 语言 / 框架 / 关键依赖：
  - **Rust**（edition 2021，MSVC target）+ **Tauri 2** —— 托盘常驻、无边框置顶窗口、单实例。
  - **`uiautomation` 0.25** —— 通过 `TextPattern` 读取选中文本；UIA 调用跑在**专用线程**上（几百毫秒级阻塞，不能放进快捷键回调或事件循环，且 UIA 客户端对象必须在同一线程创建和使用）。
  - **`windows-sys` 0.61** —— Win32 侧能力：`GetCursorPos` + `MonitorFromPoint` + `GetMonitorInfoW` 做跟随鼠标的边缘避让定位；`ShowWindow` 做编辑器最小化/还原；读写 `HKCU\...\Run` 实现开机自启（不申请提权、不弹 UAC）。
  - **Tauri 插件**：`global-shortcut`（全局热键）、`clipboard-manager`（写剪贴板）、`dialog`（选择目录）、`single-instance`（防多开）。
  - **前端**：纯静态 HTML + JS，开 `withGlobalTauri: true` 用 `window.__TAURI__` 直连后端 —— 没有 npm、vite、React，无任何 Node 侧构建步骤。
  - **MVP2 可选依赖**：`reqwest` + rustls 挂在 `ai` feature 后面（`cargo run --features ai`），默认不编译。
