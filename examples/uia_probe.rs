//! UIA 读取探针 —— 排查「为什么读不到 / 读出来格式不对」。
//!
//! 这一版的重点是**换行**：为什么同样的划词，VSCode / VS 里换行和缩进都正常，
//! 而 Chrome 网页、WorkBuddy 这类 Chromium 界面会把段落压成一行。
//!
//! 用法：
//!   cargo run --example uia_probe                      # 立即采样（焦点在哪就测哪）
//!   cargo run --example uia_probe -- --delay 8         # 倒数 8 秒，够切窗口 + 划词
//!   cargo run --example uia_probe -- --lines 20        # 逐行取文本时最多走 20 行
//!   cargo run --example uia_probe -- --delay 8 --lines 20
//!
//! ⚠️ **焦点问题（很重要）**：探针采样的是「当前焦点元素」。
//! 所以 **Alt+Tab 切到终端再跑是测不到目标程序的** —— 那一刻焦点已经在终端上了。
//! 要测别的程序，请用 `--delay`：先在终端启动，然后立刻切过去选中文字，等它采样。
//! （产品本体没这个矛盾：它靠全局快捷键触发，按键那一刻目标程序仍持有焦点。）
//!
//! **对照组做法（强烈推荐）**：拿同一个页面 / 同一段对话，先在 Chrome 里划词跑一次，
//! 再在 WorkBuddy 里划词跑一次，两份 report 直接对比 —— 差异点就是答案。
//!
//! 输出（全部在 %TEMP%，带 UTF-8 BOM，可直接用记事本打开）：
//!   text-snap-probe-report.txt   ← **全部控制台输出的副本，直接把这个文件发出来就行**
//!   text-snap-probe-A.txt        块间插换行
//!   text-snap-probe-B.txt        直接拼接
//!   text-snap-probe-ranges.txt   每一块单独一行（转义后可看不可见字符）
//!   text-snap-probe-lines.txt    按 Line 单元逐行取到的文本
//!   text-snap-probe-paras.txt    按 Paragraph 单元逐段取到的文本
//!
//! 纯控制台程序：不建窗口、不注册托盘、不占全局快捷键。

use std::io::Write as _;
use std::time::Duration;

use uiautomation::patterns::{UITextPattern, UITextRange};
use uiautomation::types::{ControlType, TextUnit};
use uiautomation::{UIAutomation, UIElement};
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};

/// 预览时每块最多显示多少字符，免得刷屏
const PREVIEW_CHARS: usize = 160;
/// 最多展示多少块
const MAX_SHOWN_RANGES: usize = 14;
/// 向上最多找几层祖先
const MAX_ANCESTORS: usize = 8;
/// 文档级普查只取前这么多字符（整流取回可能几 MB）
const DOC_PROBE_CHARS: i32 = 2000;
/// 单个「单元」文本超过这个长度就认为该单元没被正确支持，停止遍历
const UNIT_SANITY_LIMIT: usize = 4000;

// ───────────────────────────── 入口 ─────────────────────────────

fn main() {
    let opts = Options::parse();
    let dir = std::env::temp_dir();
    let report_path = dir.join("text-snap-probe-report.txt");
    let mut report = Report::default();

    report.line(format!(
        "text-snap UIA 探针 · 本进程 PID = {}",
        std::process::id()
    ));
    report.line(format!(
        "参数：delay = {} 秒，逐行上限 = {} 行",
        opts.delay, opts.lines
    ));
    report.blank();

    if opts.delay > 0 {
        countdown(opts.delay);
    } else {
        report.line("（没给 --delay，已立即采样。若要测别的程序，请加 --delay 8）");
        report.blank();
    }

    let automation = match UIAutomation::new() {
        Ok(automation) => automation,
        Err(err) => {
            report.line(format!("初始化 UI Automation 失败：{err}"));
            finish(&report, &report_path);
            std::process::exit(1);
        }
    };

    let focused = match automation.get_focused_element() {
        Ok(element) => element,
        Err(err) => {
            report.line(format!("取焦点元素失败：{err}"));
            finish(&report, &report_path);
            std::process::exit(1);
        }
    };

    // ── ⓪ 环境扫描：不用划词也能测，直接把各应用 Document 的 provider 摆在一起 ──
    scan_documents(&mut report, &automation);

    // ── 祖先链：逐层描述，并记住第一个拿到非空选区的层 ──
    let walker = automation.get_raw_view_walker().ok();
    let mut current = focused;
    let mut hit: Option<Hit> = None;

    report.line("════════ 焦点元素 + 祖先链 ════════");
    for level in 0..MAX_ANCESTORS {
        let probe = describe(&mut report, level, &current);

        if hit.is_none() && probe.texts.iter().any(|t| !t.trim().is_empty()) {
            if let Some(pattern) = probe.pattern {
                hit = Some(Hit {
                    level,
                    label: probe.label,
                    pattern,
                    ranges: probe.ranges,
                    texts: probe.texts,
                });
            }
        }

        match walker.as_ref().and_then(|w| w.get_parent(&current).ok()) {
            Some(parent) => current = parent,
            None => {
                report.line("（已经到顶，没有更多祖先了）");
                report.blank();
                break;
            }
        }
    }

    match hit {
        Some(hit) => deep_probe(&mut report, &hit, &opts, &dir),
        None => {
            report.blank();
            report.line("⚠️ 整条祖先链上都没有读到非空的选中文本。");
            report.line("   常见原因：");
            report.line("   · 采样那一刻焦点不在选中文本上（--delay 期间是不是切走了？）");
            report.line("   · 该程序没暴露 UIA 文本模式（VS Code 需要 editor.accessibilitySupport）");
            report.line("   · 目标程序以管理员身份运行，权限高于本程序（UIPI 拦截）");
            report.line("   · 选中确实是空的");
        }
    }

    finish(&report, &report_path);
}

/// 采样前的倒数，给用户切窗口 + 划词的时间。
fn countdown(seconds: u64) {
    print!("倒数 {seconds} 秒：现在切到目标程序，选中一段带换行 / 缩进的文字");
    let _ = std::io::stdout().flush();
    for rest in (1..=seconds).rev() {
        print!(" {rest}");
        let _ = std::io::stdout().flush();
        std::thread::sleep(Duration::from_secs(1));
    }
    println!();
    println!();
}

// ────────────── ⓪ 环境扫描：各应用 Document 横向对比 ──────────────

/// 扫一遍系统里所有 `Document` 级元素，把它们的无障碍 provider 摆在一起。
///
/// 这一段**不需要划词**，目的只有一个：看清不同的 Chromium 应用究竟走哪条 provider。
/// 走**原生 UIA provider** 和走 **MSAA / IAccessible2 桥**，对块级边界的表示可能不同，
/// 而这正是「Chrome 网页正常、WorkBuddy 不正常」最值得怀疑的地方。
///
/// 之所以按 `ControlType::Document` 扫、而不是按窗口类扫：Chromium 系应用的窗口类
/// 并不统一（Chrome 浏览器和 CEF / Electron 应用用的就不是同一个），按 Document
/// 扫能把它们一网打尽。
fn scan_documents(report: &mut Report, automation: &UIAutomation) {
    report.line("════════ ⓪ 环境扫描：系统里所有 Document 级元素 ════════");
    report.line("（不需要划词。看谁走原生 UIA provider、谁走 MSAA 桥）");
    report.blank();

    let matcher = automation
        .create_matcher()
        .control_type(ControlType::Document)
        .depth(8)
        .timeout(8000);

    let elements = match matcher.find_all() {
        Ok(elements) if !elements.is_empty() => elements,
        Ok(_) => {
            report.line("一个 Document 都没找到。");
            report.blank();
            return;
        }
        Err(err) => {
            report.line(format!("扫描失败：{err}"));
            report.blank();
            return;
        }
    };

    report.line(format!("找到 {} 个 Document：", elements.len()));
    report.blank();

    for (index, element) in elements.iter().take(8).enumerate() {
        let pid = element.get_process_id().unwrap_or(0);
        let process = exe_name(pid);
        let class = element.get_classname().unwrap_or_default();
        let framework = element.get_framework_id().unwrap_or_default();
        let provider = element.get_provider_description().unwrap_or_default();
        let name = element.get_name().unwrap_or_default();

        let text_pattern = match element.get_pattern::<UITextPattern>() {
            Ok(pattern) => {
                let support = pattern
                    .get_supported_text_selection()
                    .map(|value| format!("{value:?}"))
                    .unwrap_or_else(|_| "?".to_string());
                let document_census = pattern
                    .get_document_range()
                    .ok()
                    .and_then(|range| range.get_text(DOC_PROBE_CHARS).ok())
                    .map(|text| census(&text).render())
                    .unwrap_or_else(|| "（取不到文档文本）".to_string());
                format!("有（支持的选区类型 {support}）\n     文档前 {DOC_PROBE_CHARS} 字普查：{document_census}")
            }
            Err(_) => "没有".to_string(),
        };

        report.line(format!("── 候选 {index} ──"));
        report.line(format!(
            "  进程 {process}（PID {pid}）   框架 {framework}   类名 {class}"
        ));
        report.line(format!("  文档名 {}", quote(&truncate(&name, 40))));
        report.line(format!("  TextPattern：{text_pattern}"));
        report.line(format!("  provider {provider}"));
        report.blank();
    }
    if elements.len() > 8 {
        report.line(format!("（还有 {} 个没显示）", elements.len() - 8));
        report.blank();
    }

    report.line("怎么读这一段：");
    report.line("  provider 里出现 `Microsoft: Chromium` 之类 ⇒ 该应用走**原生 UIA provider**");
    report.line("  provider 里出现 `MSAA Proxy (IAccessible2)`  ⇒ 只能拿到**桥转发的文本**");
    report.line("  「文档前 2000 字普查」里换行 LF 的多寡，直接反映该 provider 怎么表示块级边界。");
    report.blank();
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

// ─────────────────────── 深挖：换行去哪了 ───────────────────────

struct Hit {
    level: usize,
    label: String,
    pattern: UITextPattern,
    ranges: Vec<UITextRange>,
    texts: Vec<String>,
}

fn deep_probe(report: &mut Report, hit: &Hit, opts: &Options, dir: &std::path::Path) {
    let joined_break = join_with_break(&hit.texts);
    let joined_direct = hit.texts.concat();

    report.line("════════ 深挖：换行到底丢在哪儿 ════════");
    report.line(format!("采样对象：第 {} 层 · {}", hit.level, hit.label));
    report.blank();

    // ① 字符普查 —— 决定性指标
    report.line("── ① 字符普查（决定性指标）──");
    report.line(format!("   拼法 A（块间插换行）{}", census(&joined_break).render()));
    report.line(format!("   拼法 B（直接拼接）  {}", census(&joined_direct).render()));
    if hit.texts.len() > 1 {
        for (index, text) in hit.texts.iter().enumerate().take(8) {
            report.line(format!("     块 [{index}] {}", census(text).render()));
        }
    }
    report.blank();

    // ② 非换行空白出现在哪 —— 看块级边界被换成了什么
    report.line("── ② 非换行空白的位置采样（块级边界被换成了什么）──");
    let samples = boundary_samples(&joined_direct, 4);
    if samples.is_empty() {
        report.line("   一个都没找到 —— 文本里连空格都很少（边界被直接抹掉了？）");
    } else {
        for sample in &samples {
            report.line(format!("   {sample}"));
        }
    }
    report.blank();

    // ③ 两种拼法的实际内容
    report.line("── ③ 拼法 A：块之间插换行（selection.rs 当前实现）──");
    print_block(report, &joined_break);
    report.blank();
    report.line("── ④ 拼法 B：直接拼接 ──");
    print_block(report, &joined_direct);
    report.blank();

    // ⑤ 缩进统计
    report.line("── ⑤ 缩进统计（行数 / 行首空格数 / Tab 数）──");
    report.line(format!("   拼法 A  {}", indent_stats(&joined_break)));
    report.line(format!("   拼法 B  {}", indent_stats(&joined_direct)));
    report.blank();

    // ⑥ 文档级普查 —— 判断 provider 的文本模型
    report.line("── ⑥ 文档级文本普查（判断 provider 用什么表示块级边界）──");
    match hit.pattern.get_document_range() {
        Ok(document) => match document.get_text(DOC_PROBE_CHARS) {
            Ok(text) => {
                report.line(format!(
                    "   取文档前 {DOC_PROBE_CHARS} 字：{}",
                    census(&text).render()
                ));
                let document_samples = boundary_samples(&text, 3);
                for sample in &document_samples {
                    report.line(format!("   {sample}"));
                }
            }
            Err(err) => report.line(format!("   document.get_text 失败：{err}")),
        },
        Err(err) => report.line(format!("   get_document_range 失败：{err}")),
    }
    report.blank();

    // ⑦⑧ 换个「单元」试试能不能把行 / 段边界要回来
    let mut lines: Vec<String> = Vec::new();
    let mut paragraphs: Vec<String> = Vec::new();

    if let Some(first) = hit.ranges.first() {
        report.line("── ⑦ 按 Line 单元逐行取（看能否要回行边界）──");
        let walk = walk_units(first, TextUnit::Line, opts.lines);
        if let Some(note) = &walk.note {
            report.line(format!("   ⚠️ {note}"));
        }
        if walk.units.is_empty() {
            report.line("   一行都没取到");
        } else {
            for (index, text) in walk.units.iter().enumerate() {
                report.line(format!(
                    "   [{index:>2}] {:>4} 字  {}",
                    text.chars().count(),
                    quote(&truncate(&visualize(text), 110))
                ));
            }
        }
        lines = walk.units;
        report.blank();

        report.line("── ⑧ 按 Paragraph 单元逐段取（看能否要回段边界）──");
        let walk = walk_units(first, TextUnit::Paragraph, opts.lines);
        if let Some(note) = &walk.note {
            report.line(format!("   ⚠️ {note}"));
        }
        if walk.units.is_empty() {
            report.line("   一段都没取到");
        } else {
            for (index, text) in walk.units.iter().enumerate() {
                report.line(format!(
                    "   [{index:>2}] {:>4} 字  {}",
                    text.chars().count(),
                    quote(&truncate(&visualize(text), 110))
                ));
            }
        }
        paragraphs = walk.units;
        report.blank();
    } else {
        report.line("── ⑦⑧ 没有可用的选区，跳过逐行 / 逐段遍历 ──");
        report.blank();
    }

    // 落盘
    let ranges_dump: String = hit
        .texts
        .iter()
        .enumerate()
        .map(|(index, text)| format!("[{index}] {}\n", escape(text)))
        .collect();
    let lines_dump = lines
        .iter()
        .enumerate()
        .map(|(index, text)| format!("[{index}] {}\n", escape(text)))
        .collect::<String>();
    let paragraphs_dump = paragraphs
        .iter()
        .enumerate()
        .map(|(index, text)| format!("[{index}] {}\n", escape(text)))
        .collect::<String>();

    let a_path = dir.join("text-snap-probe-A.txt");
    let b_path = dir.join("text-snap-probe-B.txt");
    let ranges_path = dir.join("text-snap-probe-ranges.txt");
    let lines_path = dir.join("text-snap-probe-lines.txt");
    let paragraphs_path = dir.join("text-snap-probe-paras.txt");

    write_text(&a_path, &joined_break);
    write_text(&b_path, &joined_direct);
    write_text(&ranges_path, &ranges_dump);
    write_text(&lines_path, &lines_dump);
    write_text(&paragraphs_path, &paragraphs_dump);

    report.line("── 附：已写出 ──");
    for path in [
        &a_path,
        &b_path,
        &ranges_path,
        &lines_path,
        &paragraphs_path,
    ] {
        report.line(format!("   {}", path.display()));
    }
    report.blank();
    report.line("怎么读这份报告：");
    report.line("  ① 里「换行 LF」= 0 而「半角空格」不为 0 ⇒ 块级边界被 provider 换成了空格，");
    report.line("     那种情况下 selection.rs 里「块之间补换行」永远不会生效。");
    report.line("  ⑥ 文档级普查如果同样 0 换行 ⇒ 是这个 provider 的文本模型如此，不是选区的问题。");
    report.line("  ⑦⑧ 如果 Line / Paragraph 能取回多行多段 ⇒ 还有救，可以按单元重建换行。");
}

/// 遍历结果 + 一句说明
struct UnitWalk {
    units: Vec<String>,
    note: Option<String>,
}

/// 把 range 收缩到「所在单元」，然后逐个单元往前走。
fn walk_units(start: &UITextRange, unit: TextUnit, max_units: usize) -> UnitWalk {
    let mut units = Vec::new();
    let mut note = None;
    let cursor = start.clone();

    if let Err(err) = cursor.expand_to_enclosing_unit(unit) {
        return UnitWalk {
            units,
            note: Some(format!("expand_to_enclosing_unit 失败：{err}")),
        };
    }

    for _ in 0..max_units {
        let text = cursor.get_text(-1).unwrap_or_default();
        if text.trim().is_empty() {
            break;
        }
        if text.chars().count() > UNIT_SANITY_LIMIT {
            note = Some(format!(
                "单元过大（{} 字），该单元多半没被正确支持，已停止",
                text.chars().count()
            ));
            break;
        }
        units.push(text);

        let moved = cursor.move_text(unit, 1).unwrap_or(0);
        if moved == 0 {
            break;
        }
        if let Err(err) = cursor.expand_to_enclosing_unit(unit) {
            note = Some(format!("前进后 expand 失败：{err}"));
            break;
        }
    }

    UnitWalk { units, note }
}

// ─────────────────────────── 字符普查 ───────────────────────────

#[derive(Default)]
struct Census {
    chars: usize,
    lf: usize,
    cr: usize,
    tab: usize,
    space: usize,
    nbsp: usize,
    ideographic: usize,
    other: usize,
}

impl Census {
    fn render(&self) -> String {
        format!(
            "共 {} 字 | 换行 LF {} / 回车 CR {} | Tab {} | 半角空格 {} | NBSP {} | 全角空格 {} | 其它空白 {}",
            self.chars, self.lf, self.cr, self.tab, self.space, self.nbsp, self.ideographic, self.other
        )
    }
}

fn census(text: &str) -> Census {
    let mut result = Census::default();
    for ch in text.chars() {
        result.chars += 1;
        match ch {
            '\n' => result.lf += 1,
            '\r' => result.cr += 1,
            '\t' => result.tab += 1,
            ' ' => result.space += 1,
            '\u{00A0}' => result.nbsp += 1,
            '\u{3000}' => result.ideographic += 1,
            other => {
                if other.is_whitespace() {
                    result.other += 1;
                }
            }
        }
    }
    result
}

/// 找出「非换行的空白串」，连同前后文一起展示 —— 一眼看出块级边界变成了什么。
fn boundary_samples(text: &str, limit: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut index = 0;

    while index < chars.len() && out.len() < limit {
        if chars[index].is_whitespace() && chars[index] != '\n' {
            let start = index;
            while index < chars.len() && chars[index].is_whitespace() && chars[index] != '\n' {
                index += 1;
            }
            let run: String = chars[start..index].iter().collect();
            let before: String = chars[start.saturating_sub(14)..start].iter().collect();
            let after: String = chars[index..(index + 14).min(chars.len())].iter().collect();
            out.push(format!(
                "…{before} ⟦{}⟧ {after}…   （这处空白 {} 个字符）",
                ws_glyphs(&run),
                run.chars().count()
            ));
        } else {
            index += 1;
        }
    }
    out
}

/// 把空白字符画成看得见的符号
fn ws_glyphs(run: &str) -> String {
    run.chars()
        .map(|ch| match ch {
            ' ' => '·',
            '\t' => '→',
            '\u{00A0}' => '◇',
            '\u{3000}' => '□',
            '\n' => '⏎',
            '\r' => '␍',
            _ => '?',
        })
        .collect()
}

// ──────────────────────── 逐层描述 ────────────────────────

struct LevelProbe {
    label: String,
    pattern: Option<UITextPattern>,
    ranges: Vec<UITextRange>,
    texts: Vec<String>,
}

fn describe(report: &mut Report, level: usize, element: &UIElement) -> LevelProbe {
    let pid = element.get_process_id().unwrap_or(0);
    let control = element
        .get_control_type()
        .map(|c| format!("{c:?}"))
        .unwrap_or_else(|_| "?".into());
    let class = element.get_classname().unwrap_or_default();
    let framework = element.get_framework_id().unwrap_or_default();
    let automation_id = element.get_automation_id().unwrap_or_default();
    let provider = element.get_provider_description().unwrap_or_default();
    let name = element.get_name().unwrap_or_default();
    let label = format!("{control} / {class}");

    report.line(format!("── 第 {level} 层 ───────────────────────────────"));
    report.line(format!(
        "  PID {pid}{}",
        if pid == std::process::id() {
            "（本程序自己！焦点没在目标程序上）"
        } else {
            ""
        }
    ));
    report.line(format!("  控件 {control}   类名 {class}"));
    if !framework.is_empty() {
        report.line(format!("  框架 {framework}"));
    }
    if !automation_id.is_empty() {
        report.line(format!("  AutomationId {automation_id}"));
    }
    if !provider.is_empty() {
        report.line(format!("  provider {provider}"));
    }
    if !name.is_empty() {
        report.line(format!("  名称 {}", quote(&truncate(&name, 50))));
    }

    let Ok(pattern) = element.get_pattern::<UITextPattern>() else {
        report.line("  TextPattern：没有");
        report.blank();
        return LevelProbe {
            label,
            pattern: None,
            ranges: Vec::new(),
            texts: Vec::new(),
        };
    };

    let support = pattern
        .get_supported_text_selection()
        .map(|value| format!("{value:?}"))
        .unwrap_or_else(|_| "?".to_string());
    report.line(format!("  TextPattern：有   支持的选区类型 {support}"));

    let ranges = pattern.get_selection().unwrap_or_default();
    report.line(format!("  选区块数 = {}", ranges.len()));

    let texts: Vec<String> = ranges
        .iter()
        .map(|range| range.get_text(-1).unwrap_or_default())
        .collect();

    if texts.iter().all(|text| text.trim().is_empty()) {
        report.line("  （选中是空的 —— 可能没选东西，或焦点不在文本上）");
        report.blank();
        return LevelProbe {
            label,
            pattern: Some(pattern),
            ranges,
            texts,
        };
    }

    report.line("  ── 每一块单独看（·=空格 →=Tab ◇=NBSP □=全角空格 ␍=回车 ⏎=换行）──");
    for (index, text) in texts.iter().enumerate().take(MAX_SHOWN_RANGES) {
        report.line(format!(
            "   [{index:>2}] {:>5} 字  {}",
            text.chars().count(),
            quote(&visualize(&truncate(text, PREVIEW_CHARS)))
        ));
    }
    if texts.len() > MAX_SHOWN_RANGES {
        report.line(format!(
            "   …（还有 {} 块没显示）",
            texts.len() - MAX_SHOWN_RANGES
        ));
    }
    report.blank();

    LevelProbe {
        label,
        pattern: Some(pattern),
        ranges,
        texts,
    }
}

// ──────────────────────── 小工具 ────────────────────────

/// 既打印到控制台，也攒进报告（最后一次性写文件）
#[derive(Default)]
struct Report {
    buf: String,
}

impl Report {
    fn line(&mut self, text: impl AsRef<str>) {
        let text = text.as_ref();
        println!("{text}");
        self.buf.push_str(text);
        self.buf.push('\n');
    }

    fn blank(&mut self) {
        self.line("");
    }
}

fn finish(report: &Report, path: &std::path::Path) {
    report_write(path, &report.buf);
    println!();
    println!("════════════════════════════════════════");
    println!("完整报告已写出（直接把这个文件发出来就行）：");
    println!("  {}", path.display());
}

fn report_write(path: &std::path::Path, text: &str) {
    write_text(path, text);
}

/// 一律带 UTF-8 BOM 落盘，免得记事本里中文变乱码
fn write_text(path: &std::path::Path, text: &str) {
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(text.as_bytes());
    let _ = std::fs::write(path, bytes);
}

/// 当前实现用的拼法：块之间补一个换行
fn join_with_break(texts: &[String]) -> String {
    let mut buffer = String::new();
    for text in texts {
        if text.is_empty() {
            continue;
        }
        if !buffer.is_empty() && !buffer.ends_with('\n') {
            buffer.push('\n');
        }
        buffer.push_str(text);
    }
    buffer
}

/// 把空白字符换成看得见的符号
fn visualize(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        match ch {
            ' ' => out.push('·'),
            '\t' => out.push('→'),
            '\n' => {
                out.push('⏎');
                out.push('\n');
            }
            '\r' => out.push('␍'),
            '\u{00A0}' => out.push('◇'),
            '\u{3000}' => out.push('□'),
            _ => out.push(ch),
        }
    }
    out
}

/// 给字符串加引号并把换行等转义，用于单行展示
fn quote(text: &str) -> String {
    format!("「{}」", escape(text))
}

fn escape(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        match ch {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ' ' => out.push('·'),
            '\u{00A0}' => out.push_str("\\u00a0"),
            '\u{3000}' => out.push_str("\\u3000"),
            _ => out.push(ch),
        }
    }
    out
}

fn print_block(report: &mut Report, text: &str) {
    let shown: Vec<&str> = text.lines().take(10).collect();
    if shown.is_empty() {
        report.line("   （空）");
        return;
    }
    for line in &shown {
        report.line(format!("   │ {}", truncate(line, 90)));
    }
    let total = text.lines().count();
    if total > shown.len() {
        report.line(format!("   │ …（还有 {} 行）", total - shown.len()));
    }
}

/// 行数，以及第二行起的行首空格数，用来判断缩进有没有丢
fn indent_stats(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return "空".to_string();
    }
    let indent = |line: &str| {
        line.chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .count()
    };
    let first_indent = indent(lines[0]);
    let leading: Vec<String> = lines
        .iter()
        .skip(1)
        .take(6)
        .map(|line| indent(line).to_string())
        .collect();
    let tab_count = text.chars().filter(|c| *c == '\t').count();
    format!(
        "行数 {}, 首行缩进 {}, 后续行缩进 [{}], Tab 数 {}",
        lines.len(),
        first_indent,
        leading.join(", "),
        tab_count
    )
}

fn truncate(text: &str, max_chars: usize) -> String {
    let mut out: String = text.chars().take(max_chars).collect();
    if text.chars().count() > max_chars {
        out.push('…');
    }
    out
}

// ──────────────────────── 命令行参数 ────────────────────────

struct Options {
    delay: u64,
    lines: usize,
}

impl Options {
    fn parse() -> Self {
        let mut options = Options { delay: 0, lines: 12 };
        let mut args = std::env::args().skip(1);

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--delay" => {
                    options.delay = args.next().and_then(|v| v.parse().ok()).unwrap_or(0);
                }
                "--lines" => {
                    options.lines = args.next().and_then(|v| v.parse().ok()).unwrap_or(12).max(1);
                }
                "--help" | "-h" => {
                    print_help();
                    std::process::exit(0);
                }
                other => {
                    println!("认不出的参数：{other}（用 --help 看用法）");
                    std::process::exit(2);
                }
            }
        }
        options
    }
}

fn print_help() {
    println!(
        "\
text-snap UIA 探针

用法：
  cargo run --example uia_probe                    立即采样（焦点在哪就测哪）
  cargo run --example uia_probe -- --delay 8       倒数 8 秒，够你切窗口 + 划词
  cargo run --example uia_probe -- --lines 20      逐行取文本时最多走 20 行

要测别的程序必须用 --delay —— Alt+Tab 切到终端再跑，
采样那一刻焦点已经在终端上，测不到目标程序。

报告落在 %TEMP%\\text-snap-probe-report.txt"
    );
}
