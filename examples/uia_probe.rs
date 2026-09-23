//! UIA 读取探针 —— 排查"为什么读不到 / 读出来格式不对"。
//!
//! 用法：
//!   1. 在 VS Code / Visual Studio 里选中一段**多行、带缩进**的代码
//!   2. 切到终端（用 Alt+Tab，不要点鼠标），执行：`cargo run --example uia_probe`
//!
//! 重点看两样东西：
//!   · **选区块数** —— UIA 把这次选中拆成了几块。1 块说明是一段连续文本；
//!     很多块说明是按节点拆碎的，那种情况下"块之间补换行"就会把格式搞乱。
//!   · **两种拼法对比** —— 同一份数据分别按"块间插换行"和"直接拼接"输出，
//!     哪个和原代码一样就是对的，一眼可判。
//!
//! 同时会把两份结果原始写到：
//!   %TEMP%\text-snap-probe-A.txt  （块间插换行，当前实现用的拼法）
//!   %TEMP%\text-snap-probe-B.txt  （直接拼接）
//!   %TEMP%\text-snap-probe-ranges.txt （每一块单独一行）
//!
//! 纯控制台程序：不建窗口、不注册托盘、不占全局快捷键。

use uiautomation::core::UIElement;
use uiautomation::patterns::UITextPattern;
use uiautomation::UIAutomation;

/// 预览时每块最多显示多少字符，免得刷屏
const PREVIEW_CHARS: usize = 160;
/// 最多展示多少块
const MAX_SHOWN_RANGES: usize = 14;

fn main() {
    let automation = match UIAutomation::new() {
        Ok(automation) => automation,
        Err(err) => {
            eprintln!("初始化 UI Automation 失败：{err}");
            std::process::exit(1);
        }
    };

    let focused = match automation.get_focused_element() {
        Ok(element) => element,
        Err(err) => {
            eprintln!("取焦点元素失败：{err}");
            std::process::exit(1);
        }
    };

    println!("本进程 PID = {}", std::process::id());
    println!("探针改版：会同时打印两种拼接方式，方便判断哪种和原文一致");
    println!();

    let walker = automation.get_raw_view_walker().ok();
    let mut current = focused;

    for level in 0..8 {
        describe(level, &current);
        match walker.as_ref().and_then(|w| w.get_parent(&current).ok()) {
            Some(parent) => current = parent,
            None => {
                println!("（已经到顶，没有更多祖先了）");
                break;
            }
        }
    }
}

fn describe(level: usize, element: &UIElement) {
    let pid = element.get_process_id().unwrap_or(0);
    let control = element
        .get_control_type()
        .map(|c| format!("{c:?}"))
        .unwrap_or_else(|_| "?".into());
    let class = element.get_classname().unwrap_or_default();
    let name = element.get_name().unwrap_or_default();

    println!("── 第 {level} 层 ───────────────────────────────");
    println!(
        "  PID {pid}{}  控件 {control}  类名 {class}",
        if pid == std::process::id() { "（本程序自己）" } else { "" }
    );
    if !name.is_empty() {
        println!("  名称 {}", truncate(&name, 60));
    }

    let Ok(pattern) = element.get_pattern::<UITextPattern>() else {
        println!("  TextPattern  没有");
        println!();
        return;
    };

    let Ok(ranges) = pattern.get_selection() else {
        println!("  TextPattern  有，但 get_selection 失败");
        println!();
        return;
    };

    println!("  选区块数 = {}", ranges.len());

    let mut texts: Vec<String> = Vec::new();
    for range in &ranges {
        if let Ok(text) = range.get_text(-1) {
            texts.push(text);
        }
    }

    if texts.iter().all(|t| t.trim().is_empty()) {
        println!("  （选中是空的 —— 多半没选东西）");
        println!();
        return;
    }

    // ── 每一块单独看 ────────────────────────────────────────
    println!("  ── 每一块单独看（·=空格 →=Tab ⏎=换行 ␍=回车）──");
    for (index, text) in texts.iter().enumerate().take(MAX_SHOWN_RANGES) {
        println!(
            "   [{index:>2}] {} 字符  {}",
            text.chars().count(),
            quote(&visualize(&truncate(text, PREVIEW_CHARS)))
        );
    }
    if texts.len() > MAX_SHOWN_RANGES {
        println!("   …（还有 {} 块没显示）", texts.len() - MAX_SHOWN_RANGES);
    }

    // ── 两种拼法对比 ────────────────────────────────────────
    let joined_with_break = join_with_break(&texts);
    let joined_direct = texts.concat();

    println!();
    println!("  ── 拼法 A：块之间插换行（当前实现）── {} 字符", joined_with_break.chars().count());
    print_block(&joined_with_break);

    println!();
    println!("  ── 拼法 B：直接拼接 ── {} 字符", joined_direct.chars().count());
    print_block(&joined_direct);

    let stats_a = indent_stats(&joined_with_break);
    let stats_b = indent_stats(&joined_direct);
    println!();
    println!("  ── 缩进统计（行数 / 第二行起首的行首空格数）──");
    println!("   拼法 A  {stats_a}");
    println!("   拼法 B  {stats_b}");

    // 落到文件，方便直接拿去比对
    let dir = std::env::temp_dir();
    let _ = std::fs::write(dir.join("text-snap-probe-A.txt"), &joined_with_break);
    let _ = std::fs::write(dir.join("text-snap-probe-B.txt"), &joined_direct);
    let ranges_dump: String = texts
        .iter()
        .enumerate()
        .map(|(index, text)| format!("[{index}] {}\n", escape(text)))
        .collect();
    let _ = std::fs::write(dir.join("text-snap-probe-ranges.txt"), ranges_dump);

    println!();
    println!("  已写出：");
    println!("    {}", dir.join("text-snap-probe-A.txt").display());
    println!("    {}", dir.join("text-snap-probe-B.txt").display());
    println!("    {}", dir.join("text-snap-probe-ranges.txt").display());
    println!();
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
            _ => out.push(ch),
        }
    }
    out
}

fn print_block(text: &str) {
    let shown: Vec<&str> = text.lines().take(10).collect();
    if shown.is_empty() {
        println!("   （空）");
        return;
    }
    for line in &shown {
        println!("   │ {}", truncate(line, 90));
    }
    let total = text.lines().count();
    if total > shown.len() {
        println!("   │ …（还有 {} 行）", total - shown.len());
    }
}

/// 行数，以及第二行起的行首空格数，用来判断缩进有没有丢
fn indent_stats(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return "空".to_string();
    }
    let indent = |line: &str| line.chars().take_while(|c| *c == ' ' || *c == '\t').count();
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
