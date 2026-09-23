# text-snap

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-Windows%2010%20%2F%2011-lightgrey.svg)](https://github.com/xz-code/text-snap)
[![Rust](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org/)
[![Tauri](https://img.shields.io/badge/tauri-2.x-24C8DB.svg)](https://tauri.app/)

Windows 托盘小工具。选中任意窗口里的文字，按一下快捷键，就在鼠标旁边弹出一小块窗口把内容显示出来。
读取走的是 **Windows UI Automation 无障碍接口**，**读取路径完全不碰剪贴板**。

---

## 为什么不用 Ctrl+C

大多数「抓取选中文字」的工具都是模拟 `Ctrl+C` 然后读剪贴板。能用，但代价不小：

- 会**覆盖你剪贴板里原本的内容**。
- 在**剪贴板访问受限的地方直接失效** —— 企业 DLP 策略、部分 RDP / VDI 环境、沙箱会话、
  安全桌面。
- 会**短暂抢占键盘**，可能和你正在用的程序打架。
- 行为上**和你自己复制数据没有区别** —— 在会审计复制操作的环境里，这一点很重要。

text-snap 换了一条路：通过 **UI Automation** 向目标程序索取它的选中文本。这是 Windows 给
读屏软件（NVDA、讲述人）提供的**公开无障碍接口**，程序自己把文本交出来，剪贴板全程不参与。

这不是在绕什么安全机制 —— 这就是 Windows 为无障碍软件准备的机制，按它本来的用法在用。

---

## 功能

- **不用剪贴板读取选中文本** —— 走 UI Automation 的 `TextPattern`，并逐级向上查找正确的元素。
- **全局快捷键**（默认 `Ctrl+Shift+A`），可在设置窗口里随时改。
- **无边框置顶弹窗** —— 在鼠标旁出现，失焦自动收起，再按一次快捷键也能收起。
- **自动复制到剪贴板** —— 读到文本的那一刻就写进去，按完快捷键直接 `Esc` + `Ctrl+V`。
- **一键保存为本地 txt** —— 固定目录、自动命名。
- **复制按钮** —— 想再复制一次的时候用。
- **设置窗口** —— 快捷键用「按键录制」方式改，保存目录可以浏览选择。
- **单实例保护** —— 重复启动不会再生成第二个托盘图标，而是唤起已经在跑的那个。
- **全程没有 Node.js** —— 前端是纯静态 HTML + JS，靠 `withGlobalTauri` 直连后端，
  整个项目只用 `cargo` 就能构建。

---

## 环境要求

**编译**

- Rust 工具链（验证于 `cargo` / `rustc` **1.98.1**，最低 1.85）
- MSVC 链接器 —— Visual Studio Build Tools，勾选「使用 C++ 的桌面开发」

**运行**

| 依赖 | 说明 |
|---|---|
| **WebView2 运行时** | 界面靠它渲染。Win11 自带；Win10 装了 Edge 就有 |
| **VC++ 2015–2022 可再发行组件 (x64)** | 装了 Visual Studio 的机器都有 |

---

## 快速开始

```bash
git clone https://github.com/xz-code/text-snap.git
cd text-snap
cargo run
```

首次编译要拉 400+ 个 crate，5～15 分钟。启动后：

1. 托盘区出现一个蓝色小图标
2. 切到 VS Code（或 Visual Studio），**选中一段文字**
3. 按 `Ctrl+Shift+A`
4. 鼠标下方弹出小窗显示原文，**内容同时已经进了剪贴板**
5. `Esc` 收起，然后在任何地方 `Ctrl+V`

> 弹窗已经开着时再按一次快捷键 = 收起（开关式）。

**发布版**

```bash
cargo build --release
```

产物是 `target\release\text-snap.exe`。**前端资源已内嵌**，拷这一个文件到任何 x64 Windows
机器就能跑，不需要带 `ui\` 目录。

> 不要用 `cargo tauri build`。它会去下载 NSIS/WiX 打安装包，网络受限时基本拉不下来，
> 而这个工具只需要一个绿色免安装 exe。

---

## 使用

**全局快捷键**：默认 `Ctrl+Shift+A`

**托盘菜单**（右键图标）

| 菜单项 | 作用 |
|---|---|
| 读取当前选中文本 | 等同于按快捷键。注意：点托盘会让焦点落到任务栏上，可能读不到；用快捷键更可靠 |
| **设置…** | 打开设置窗口（快捷键 / 保存目录） |
| 打开保存目录 | 在资源管理器里打开保存目录 |
| 退出 | 退出程序 |

**左键单击托盘图标**也能读取当前选中文本。

### 弹窗

两个按钮：

| 按钮 | 作用 |
|---|---|
| **复制到剪贴板** | 把当前显示的文本写进剪贴板，并给出成功/失败提示 |
| **保存到本地 txt** | 写入保存目录，完整路径显示在按钮右侧 |

**自动复制**：弹窗出现的同时文本就已经写进剪贴板了，成功时卡片右上角会亮起「已复制」小徽标。
这是固定行为，没有开关。

所以最顺手的用法是：**选中 → `Ctrl+Shift+A` → `Esc` → `Ctrl+V`**。

弹窗里按 `Esc` 收起。

### 设置窗口

托盘右键 →「设置…」，或者程序已经在跑时再双击一次 exe。

**全局快捷键** —— 点一下那个框，直接按你想用的组合键，界面实时显示。`Esc` 取消录制。
至少要有一个修饰键；只按修饰键时会显示 `Ctrl+Shift+…` 等你按主键。支持 `A`–`Z`、`0`–`9`、
`F1`–`F12`、`Space`、`Enter`、`Tab` 以及常见符号键；小键盘和媒体键暂不支持。

**保存目录** —— 可以直接输入/粘贴，也可以点「浏览…」选文件夹。「打开」用来当场验证路径对不对，
「恢复默认」清空回「文档\text-snap」。

> 留空 = 用默认目录。目录不存在不算错，首次保存时会自动创建。

改完点「保存并应用」即时生效，不用重启。如果新快捷键被别的程序占了，会报错并**自动回滚**到
原来那个，不会出现「改坏了没快捷键可用」的情况。

---

## 配置

**平时不用手改** —— 设置窗口里都能改。这一节是给需要直接编辑文件或排查问题时看的。

`%APPDATA%\text-snap\config.json`，首次运行自动生成：

```json
{
  "hotkey": "Ctrl+Shift+A",
  "save_dir": null
}
```

| 字段 | 说明 |
|---|---|
| `hotkey` | 全局快捷键。写法：`Ctrl+Shift+A`、`Alt+Q`、`Ctrl+Alt+F12`、`Ctrl+Shift+Space` |
| `save_dir` | 保存目录。`null` 或留空 = 用「文档\text-snap」 |

手改完之后，打开设置窗口点一下「保存并应用」即可重新读取生效。

支持的按键名：`A`–`Z`、`0`–`9`、`F1`–`F12`、`Space`、`Enter`、`Tab`、`Esc`、
`Minus`/`-`、`Equal`/`=`、`Comma`/`,`、`Period`/`.`、`Slash`/`/`、`Semicolon`/`;`、
`Quote`/`'`、`Backquote`/`` ` ``、`BracketLeft`/`[`、`BracketRight`/`]`。
修饰键：`Ctrl`/`Control`、`Shift`、`Alt`、`Win`/`Super`/`Meta`。

> 快捷键至少要带一个修饰键，否则会抢走正常输入。

---

## 保存规则

点「保存到本地 txt」后，落到：

```
<文档>\text-snap\20260922_211005_<正文首行前 20 字>.txt
```

- 文件名里的正文来自**首行前 20 个字符**，会剔除 `\ / : * ? " < > |` 和控制字符
- 重名自动加序号：`..._2.txt`、`..._3.txt`
- `.txt` 写 **UTF-8 BOM**，避免某些编辑器里中文显示成乱码
- `.md`（规划中的功能用）写无 BOM 的 UTF-8

---

## 项目结构

```
text-snap/
├─ Cargo.toml                    依赖 + 可选的 ai feature
├─ build.rs                      tauri-build 入口
├─ tauri.conf.json               Tauri 配置（withGlobalTauri / frontendDist）
├─ capabilities/default.json     两个窗口的权限集
├─ icons/                        应用与托盘图标（ico + png）
├─ src/
│  ├─ main.rs                    托盘 / 快捷键 / 弹窗 / 命令
│  ├─ config.rs                  配置读写
│  └─ selection.rs               UIA 读取选中文本
├─ examples/
│  └─ uia_probe.rs               控制台诊断探针
└─ ui/
   ├─ index.html                 划词弹窗
   └─ settings.html              设置窗口
```

**没有 npm、vite、React。** 开了 `withGlobalTauri: true`，`ui/*.html` 直接用
`window.__TAURI__` 调后端。整个项目没有任何 Node 侧构建步骤。

---

## 工作原理

```
按下 Ctrl+Shift+A
   │
   ├─ tauri-plugin-global-shortcut 截获按键
   │        ↓ 往工作线程投递一次请求（mpsc channel）
   │
   ├─ UIA 工作线程（专用线程，持有常驻的 UIAutomation 客户端）
   │        ├─ get_focused_element()
   │        ├─ 取 process_id / control_type / classname 做诊断信息
   │        └─ 逐级向上找暴露了 TextPattern 的祖先（最多 6 层）
   │             └─ get_selection() → 拼出选中文本
   │
   └─ 读到文本后
        ├─ 写进剪贴板（失败只记日志，绝不影响展示）
        ├─ 结果存进内存 → 推给前端（带 auto_copied 标记）
        └─ 定位到鼠标附近 → show + set_focus
```

几个值得知道的决策：

- **UIA 走专用线程。** UIA 调用会阻塞几十到几百毫秒，绝不能放在快捷键回调或 Tauri 事件
  循环里，否则整个 UI 会冻住。而且 `UIAutomation::new()` 内部会
  `CoInitializeEx(COINIT_MULTITHREADED)`，对象必须在**同一个线程**创建和使用，不能跨线程传。

- **向上扫祖先。** 在 VS Code 这类 Chromium 应用里，焦点元素常常是一个嵌套输入节点，
  真正的文本挂在祖先上，所以逐层试探，取第一个非空结果。

- **弹窗提前建好并隐藏。** WebView2 冷启动要几百毫秒，按需创建会毁掉「按下即现」的手感。
  改成 setup 阶段用 `visible(false)` 建好，按键时只做定位和显示。

- **先存内容再推送。** 前端可能还没加载完就被推送命中，所以用单一事件 + 一个
  `last_selection` 命令，让前端启动时补拉最后一次结果。

- **失焦即隐藏**，但展示后 300ms 内忽略，躲开 `show()` 过程中的焦点抖动。这条**只对划词弹窗
  生效**（按窗口 label 判断）—— 设置窗口是常规窗口，跟着一起收掉就没法用了。

- **两个窗口，两种性格。** `main` 是划词弹窗：无边框、置顶、不出现在任务栏、失焦即隐藏、
  启动时就预先创建。`settings` 是常规窗口：有边框、可调大小、按需创建、关掉即销毁。

- **定位用 Win32** —— `GetCursorPos` + `MonitorFromPoint` + `GetMonitorInfoW`，跟随鼠标、
  按屏幕工作区做边缘避让、下方放不下自动翻到上方。

---

## 排障

**所有诊断都写在这一个文件里：**

```
%APPDATA%\text-snap\text-snap.log
```

托盘程序没有控制台，启动日志、快捷键注册结果、每次读取的成败，全在里面。

**先跑诊断探针：**

```bash
cargo run --example uia_probe
```

在目标程序里选中一段文字，用 `Alt+Tab` 切到终端（**不要点鼠标**，会丢选中），然后执行。
它会逐层打印：

- 焦点元素属于哪个进程、什么控件类型、什么类名
- 哪一层暴露了 `TextPattern`
- 选中的实际内容是什么
- UIA 这次返回了**几个选区块**，以及两种拼接方式的并排对比

同时会往 `%TEMP%` 落三份文件（`text-snap-probe-A.txt`、`-B.txt`、`-ranges.txt`）。

| 症状 | 原因 |
|---|---|
| 双击完全没反应，连日志都没生成 | 进程没起来：WebView2 缺失，或被安全软件拦了 |
| 日志里 `快捷键 [xxx] 注册失败` | 该组合被别的软件占了，换一个 |
| 弹窗提示「当前没有选中任何文本」 | 真的没选东西。先选中一段文字再按快捷键 |
| 弹窗提示「没有暴露 UI Automation 文本模式」 | 目标程序不提供文本接口，或它权限更高 |
| 弹窗提示「焦点落在任务栏/托盘区域上了」 | 用托盘图标触发时容易这样。回到目标程序再用快捷键 |
| 设置里改快捷键提示「已被别的程序占用」 | 换个组合。原快捷键会自动回滚 |
| 托盘出现两个图标 / 快捷键突然失效 | 缺单实例保护时会出现。重启，或 `taskkill /F /IM text-snap.exe` |
| 点了「退出」没反应 | 已修复的 bug，详见「已知限制」 |
| 弹窗没显示「已复制」徽标 | 写剪贴板失败，日志里有 `自动复制失败：...`。多半是剪贴板被别的程序占着，或 DLP 拦了写入 |

---

## 已知限制

- **目标程序权限更高时读不到。** UIA 的跨权限限制来自 UIPI，只拦「低完整性 → 高完整性」。
  如果你用「以管理员身份运行」启动了 VS Code / Visual Studio，本程序（中完整性）就读不到它的
  窗口，会返回明确错误，**不会**退回去偷偷读剪贴板。

- **只保证 VS Code 和 Visual Studio。** 这两个是主要目标。其余应用看运气：
  Chromium/Electron 系（Chrome、Edge）通常可以；Windows Terminal 这类基本不暴露 `TextPattern`。

- **VS Code 需要无障碍支持生效。** 它有个 `editor.accessibilitySupport` 设置
  （`on` / `off` / `auto`，默认 `auto`）。读不到就改成 `on` 再试。

- **从「渲染后的网页 / 聊天窗口」划词，拿不到换行和缩进。** 这是选区来源的固有属性，不是解析
  问题：HTML 里的换行是排版效果，**不是文本里的 `\n` 字符**；网页里的缩进也是 CSS，不是空格。
  所以从无障碍接口拿到的 DOM 文本天然就连成一片。从**编辑器**（VS Code / Visual Studio /
  记事本）里划词没有这个问题 —— 那里的换行和空格是真实字符。

  判断方法很简单：同一个程序里选中**代码**，换行缩进都对；选中**渲染后的文章**，段落之间的
  换行可能就没了。这不是 text-snap 独有的行为，任何走纯文本通道的工具都一样。

- **弹窗里超长行会被软折行。** 弹窗宽度固定，`white-space: pre-wrap` 会把超过宽度的行自动折行，
  折出来的续行没有缩进、也没有折行标记。看**代码**时版面会显得「被重排过」—— 但底下的数据
  （剪贴板内容、保存的文件）是逐字节忠实的。

- **没有剪贴板读取兜底。** 刻意的设计选择 —— 宁可读不到，也不留一条会破坏初衷的路。
  注意区分方向：**写**剪贴板是产品功能，**读**才是被排除的那条。

- **写剪贴板也可能被拦。** 读取走 UIA 绕开了剪贴板管控，但把文本写回剪贴板时，同样会经过系统
  剪贴板。如果你的策略连写入也管，自动复制和复制按钮都会失败 —— 程序会如实报错并记日志，
  不会静默假装成功。这种情况请改用「保存到本地 txt」。

---

## 路线图

**MVP1（已完成）** —— 托盘、全局快捷键、UIA 读取、弹窗、自动复制、复制按钮、保存 txt、
设置窗口、单实例保护。

**MVP2（计划中）** —— 弹窗里加 AI 对话区：把原文发给大模型做总结 / 翻译 / 代码解释，
结果可另存为 markdown。

```bash
cargo run --features ai
```

依赖已经挂在 `ai` feature 后面（`reqwest` + rustls），默认不编译。开工前还有两个待定：

1. **API Key 存哪** —— 明文配置文件 / Windows 凭据管理器 / DPAPI 加密
2. **要不要流式输出** —— 非流式几十行就够，流式体验好但代码量明显增加

端点：`https://api.deepseek.com/v1/chat/completions`

---

## 部署到其他机器

**只拷 exe**：`target\release\text-snap.exe`，单文件可跑，不需要带任何附属文件。

**要从源码编**，拷这个清单过去（**别拷 `target\`**，那是几个 G）：

```
Cargo.toml  Cargo.lock  build.rs  tauri.conf.json
capabilities/  icons/  src/  ui/  examples/
```

`gen/` 是编译时生成的，不用拷。

**开机自启**：`Win+R` → `shell:startup` → 把 exe 的快捷方式丢进去。不需要管理员权限。

**走代理拉不到 crates.io**，在 `%USERPROFILE%\.cargo\config.toml` 里配国内镜像：

```toml
[source.crates-io]
replace-with = "rsproxy-sparse"

[source.rsproxy-sparse]
registry = "sparse+https://rsproxy.cn/index/"

[registries.rsproxy]
index = "sparse+https://rsproxy.cn/index/"

[net]
git-fetch-with-cli = true
```

**想彻底摆脱 VC++ 运行库依赖**，只为 MSVC target 接受静态 CRT：

```toml
[target.x86_64-pc-windows-msvc]
rustflags = ["-C", "target-feature=+crt-static"]
```

> 必须写在 `[target.x86_64-pc-windows-msvc]` 段里。写成环境变量 `RUSTFLAGS` 会连带污染
> proc-macro 和 build script，可能导致编译失败。

### 在受管控的机器上部署前

1. **exe 未签名。** SmartScreen、AppLocker、WDAC 都可能拦下或删除它，杀毒软件也可能误报。
   建议先加白名单。
2. **「全局快捷键 + 读取其他窗口内容」是 EDR 的典型告警画像。** MVP1 没有任何外网调用，
   相对安静；MVP2 接上 DeepSeek 后会多一条出站 HTTPS，特征会更明显。
3. **先确认你有权这么做。** 如果机器由雇主管控，安装一个会读取窗口内容、并注册全局快捷键的
   工具之前，请先确认符合公司政策。

---

## 许可

MIT —— 见 [LICENSE](LICENSE)。
