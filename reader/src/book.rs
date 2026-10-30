//! 书籍解析：TXT → 章节列表
//!
//! v0 只处理 UTF-8 TXT（兼容 BOM、CRLF/CR），按章节正则切分。
//! 章节切不到时整本作为一章。

use anyhow::{Context, Result, bail};

#[derive(Debug, Clone)]
pub struct Chapter {
    pub title: String,
    /// 该章的字符流（已去掉换行，供排版器重新折行）
    pub text: Vec<char>,
}

#[derive(Debug)]
pub struct Book {
    pub title: String,
    pub chapters: Vec<Chapter>,
}

/// 卷标题：第一卷 / 第二卷（只作分隔，不单独成章）
fn is_volume_heading(line: &str) -> bool {
    let t = line.trim();
    let n = t.chars().count();
    if n == 0 || n > 12 {
        return false;
    }
    let num = "一二三四五六七八九十百千万零〇两0-9";
    let mut chars = t.chars().peekable();
    if let Some('第') = chars.peek().copied() {
        chars.next();
        let mut digits = 0usize;
        while let Some(&c) = chars.peek() {
            if num.contains(c) {
                digits += 1;
                chars.next();
            } else {
                break;
            }
        }
        if digits >= 1 {
            return matches!(chars.peek(), Some(&'卷'));
        }
    }
    false
}

/// 常见中文章节标题模式：第x章/节/卷、Chapter N、纯数字行等
fn is_chapter_heading(line: &str) -> bool {
    let t = line.trim();
    let n = t.chars().count();
    if n == 0 || n > 30 {
        return false;
    }
    let num = "一二三四五六七八九十百千万零〇两0-9";
    // 第X章/节/卷/回/篇（可带副标题：第二章 转折）
    let mut chars = t.chars().peekable();
    if let Some('第') = chars.peek().copied() {
        chars.next();
        let mut digits = 0usize;
        while let Some(&c) = chars.peek() {
            if num.contains(c) {
                digits += 1;
                chars.next();
            } else {
                break;
            }
        }
        if digits >= 1 {
            if let Some(&c) = chars.peek() {
                if ['章', '节', '卷', '回', '篇', '部', '集'].contains(&c) {
                    return true;
                }
            }
        }
    }
    // Chapter N / CHAPTER N
    let lower = t.to_lowercase();
    if lower.starts_with("chapter ") {
        return true;
    }
    // 序章/楔子/前言/后记/番外（后可跟空格+副标题）
    for prefix in [
        "序章", "序言", "楔子", "前言", "后记", "尾声", "番外", "终章",
    ] {
        let pl = prefix.chars().count();
        if t.starts_with(prefix) {
            let rest: Vec<char> = t.chars().skip(pl).collect();
            if rest.is_empty() || rest[0] == ' ' || rest[0] == '　' {
                return true;
            }
        }
    }
    false
}

pub fn load_txt(path: &std::path::Path) -> Result<Book> {
    let raw = std::fs::read(path).with_context(|| format!("读取 {}", path.display()))?;
    let mut text = String::from_utf8_lossy(&raw).into_owned();
    if text.starts_with('\u{feff}') {
        text.remove(0);
    }
    // 统一换行为 \n
    let text = text.replace("\r\n", "\n").replace('\r', "\n");

    let title = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("未命名")
        .to_string();

    let mut chapters: Vec<Chapter> = Vec::new();
    let mut cur_title = String::from("简介");
    let mut buf: Vec<Vec<char>> = Vec::new();

    // 段首缩进两个全角空格，空行丢弃；空章节（如「第一卷」标题后直接跟「第一章」）不单独成章
    let flush = |title: &str, buf: &mut Vec<Vec<char>>, out: &mut Vec<Chapter>| {
        let mut text: Vec<char> = Vec::new();
        for (k, para) in buf.iter().enumerate() {
            if k > 0 {
                text.push('\n');
            }
            text.push('\u{3000}');
            text.push('\u{3000}');
            text.extend(para.iter().copied());
        }
        if !text.is_empty() {
            out.push(Chapter {
                title: title.to_string(),
                text,
            });
        }
        buf.clear();
    };

    for line in text.split('\n') {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if is_volume_heading(t) {
            // 卷标题只作为分隔，不单独成章
            flush(&cur_title, &mut buf, &mut chapters);
            cur_title = String::from("简介");
            continue;
        }
        if is_chapter_heading(t) {
            flush(&cur_title, &mut buf, &mut chapters);
            cur_title = t.to_string();
            continue;
        }
        buf.push(t.chars().collect());
    }
    flush(&cur_title, &mut buf, &mut chapters);

    if chapters.is_empty() {
        bail!("书籍为空: {}", path.display());
    }
    // 无章节标记的书：整本当一章，用书名作标题
    if chapters.len() == 1 {
        chapters[0].title = title.clone();
    }
    Ok(Book { title, chapters })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chapter_split() {
        let dir = std::env::temp_dir().join("note4_reader_test.txt");
        std::fs::write(
            &dir,
            "第一卷 风起\n第一章 起点\n天色渐晚。\n他推开门。\n第二章 转折\n风起了。",
        )
        .unwrap();
        let b = load_txt(&dir).unwrap();
        assert_eq!(b.chapters.len(), 2);
        assert_eq!(b.chapters[0].title, "第一章 起点");
        assert_eq!(
            b.chapters[1].text,
            vec!['\u{3000}', '\u{3000}', '风', '起', '了', '。']
        );
    }
}
