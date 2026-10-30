//! NOTE4 墨水屏小说阅读器（主机侧渲染 + 自托管服务端推页）
//!
//! 流程：TXT → 切章 → 排版分页 → 每页渲染成 400×300 1bpp 位图
//!      → 上传自托管服务端（/api/display/*）→ 设备按键时由服务端下发显示。
//!
//! 用法：
//!   note4-reader <书.txt> --serve --sync http://<服务器>:12900
//!   note4-reader <书.txt> --serve --sync <url> --window 2   # 只装当前章起的 2 章（快速验证）
//!   note4-reader <书.txt> --dry-run                          # 只渲染首页到 /tmp 看效果
//!
//! 也可用环境变量 NOTE4_SERVER 代替 --sync 参数。

mod book;
mod layout;
mod progress;
mod render;
mod sync;

use ab_glyph::FontRef;
use anyhow::{Context, Result, bail};
use layout::{PageStyle, Paginator};

const DEFAULT_FONT: f32 = 28.0;

struct Reader {
    chapters: Vec<book::Chapter>,
    style: PageStyle,
    /// 当前章的排版结果（换章时重排）
    pages: Vec<layout::Page>,
    chapter: usize,
    page: usize,
    /// 每章字符数的前缀和（len = chapters.len()+1），用于全书进度百分比
    char_cum: Vec<usize>,
}

impl Reader {
    fn new(chapters: Vec<book::Chapter>, style: PageStyle, chapter: usize, page: usize) -> Self {
        let mut char_cum = Vec::with_capacity(chapters.len() + 1);
        char_cum.push(0);
        for c in &chapters {
            let last = *char_cum.last().unwrap();
            char_cum.push(last + c.text.len());
        }
        let mut r = Reader {
            chapters,
            style,
            pages: Vec::new(),
            chapter: 0,
            page: 0,
            char_cum,
        };
        r.goto_chapter(chapter, page);
        r
    }

    fn goto_chapter(&mut self, chapter: usize, page: usize) {
        self.chapter = chapter.min(self.chapters.len().saturating_sub(1));
        self.pages =
            Paginator::new(font(), self.style.clone()).paginate(&self.chapters[self.chapter].text);
        self.page = page.min(self.pages.len().saturating_sub(1));
    }

    fn title(&self) -> &str {
        &self.chapters[self.chapter].title
    }

    fn total_pages(&self) -> usize {
        self.pages.len()
    }

    /// 全书进度（0-100），按已读字符占比估算：章内按页首字符位置折算
    fn book_percent(&self) -> u32 {
        book_percent_at(&self.char_cum, self.chapter, &self.pages, self.page)
    }

    fn render(&self) -> Vec<u8> {
        self.render_with(None)
    }

    fn render_with(&self, footer: Option<render::Footer<'_>>) -> Vec<u8> {
        let page_label = format!(
            "{}/{} · {}%",
            self.page + 1,
            self.pages.len(),
            self.book_percent()
        );
        render::render_page_with_footer(
            font(),
            &self.pages[self.page],
            &self.chapters[self.chapter].text,
            self.title(),
            &page_label,
            &self.style,
            footer,
        )
    }

}

/// 全局字体（leak 成 'static，进程生命周期内有效）
static FONT: std::sync::OnceLock<FontRef<'static>> = std::sync::OnceLock::new();

fn font() -> &'static FontRef<'static> {
    FONT.get().expect("字体未加载")
}

fn load_font() -> Result<&'static FontRef<'static>> {
    if let Some(f) = FONT.get() {
        return Ok(f);
    }
    let path = [
        "assets/fonts/NotoSansSC-Regular.otf",
        "../assets/fonts/NotoSansSC-Regular.otf",
    ]
    .iter()
    .find(|p| std::path::Path::new(p).exists())
    .copied()
    .context("找不到字体 assets/fonts/NotoSansSC-Regular.otf")?;
    let bytes: &'static [u8] = Box::leak(
        std::fs::read(path)
            .with_context(|| format!("读取字体 {path}"))?
            .into_boxed_slice(),
    );
    let font = FontRef::try_from_slice(bytes)
        .map_err(|e: ab_glyph::InvalidFont| anyhow::anyhow!("字体解析失败: {e}"))?;
    Ok(FONT.get_or_init(|| font))
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let serve_mode = args.iter().any(|a| a == "--serve");
    let book_path = args.iter().find(|a| !a.starts_with("--")).cloned();
    let font_size = flag_value(&args, "--font")
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_FONT);
    let jump_page: Option<usize> = flag_value(&args, "--page").and_then(|v| v.parse().ok());
    let jump_chapter: Option<usize> = flag_value(&args, "--chapter").and_then(|v| v.parse().ok());
    let window: Option<usize> = flag_value(&args, "--window").and_then(|v| v.parse().ok());
    // 服务端地址：--sync <url> 或环境变量 NOTE4_SERVER（必须带 http 前缀）
    let server_url = flag_value(&args, "--sync")
        .filter(|v| v.starts_with("http"))
        .or_else(|| std::env::var("NOTE4_SERVER").ok().filter(|v| v.starts_with("http")));
    let sync_client = server_url.map(sync::Sync::new);

    let Some(book_path) = book_path else {
        bail!(
            "用法: note4-reader <书.txt> --serve --sync http://<服务器>:12900 [--font 28] [--chapter N] [--page N] [--window N] [--dry-run]"
        );
    };

    // 进度用绝对路径做 key，避免从不同目录启动时认不出同一本书
    let progress_key = std::fs::canonicalize(&book_path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| book_path.clone());

    let book = book::load_txt(std::path::Path::new(&book_path)).with_context(|| "载入书籍")?;
    load_font()?;
    let style = PageStyle {
        font_size,
        ..Default::default()
    };

    // 进度恢复：优先级 显式 --chapter/--page > 服务端（设备按键真值）> 本地文件
    let saved = progress::load()
        .iter()
        .find(|p| p.book == progress_key)
        .cloned();
    let server_state = match &sync_client {
        Some(s) => match s.pull(&progress_key).await {
            Ok(state) => {
                if let Some((ch, pg)) = state {
                    eprintln!("已从服务端恢复进度: 第{}章 第{}页", ch + 1, pg + 1);
                    Some((ch, pg))
                } else {
                    None
                }
            }
            Err(e) => {
                eprintln!("进度同步拉取失败（继续用本地）: {e:#}");
                None
            }
        },
        None => None,
    };
    // --chapter N 按书里的「第N章」计；若首章是「简介」前导，索引正好等于 N
    let preamble = book
        .chapters
        .first()
        .map(|c| c.title == "简介")
        .unwrap_or(false);
    let chapter = jump_chapter
        .map(|n| if preamble { n } else { n.saturating_sub(1) })
        .or(server_state.as_ref().map(|s| s.0))
        .or(saved.as_ref().map(|p| p.chapter))
        .unwrap_or(0)
        .min(book.chapters.len().saturating_sub(1));
    let page = jump_page
        .map(|p| p.saturating_sub(1))
        .or(server_state.as_ref().map(|s| s.1))
        .or(saved.as_ref().map(|p| p.page))
        .unwrap_or(0);

    let reader = Reader::new(book.chapters, style, chapter, page);
    let mode_name = if serve_mode {
        "服务端(设备按键翻页)"
    } else if dry_run {
        "dry-run"
    } else {
        "未指定模式"
    };
    let sync_note = if sync_client.is_some() { " + 进度同步" } else { "" };
    eprintln!(
        "《{}》{} 章 | 「{}」{} 页 | 字号 {} | 模式 {}{}",
        book.title,
        reader.chapters.len(),
        reader.title(),
        reader.total_pages(),
        font_size,
        mode_name,
        sync_note
    );

    if dry_run {
        std::fs::write("/tmp/note4_first_page.png", reader.render())?;
        eprintln!("已渲染 /tmp/note4_first_page.png");
        return Ok(());
    }

    // 服务端模式：整书渲染上传，设备按键翻页由服务端经 WS 推页显示
    if serve_mode {
        let Some(s) = &sync_client else {
            bail!("--serve 需要服务端地址：--sync http://<服务器>:12900 或环境变量 NOTE4_SERVER");
        };
        run_serve(&reader, s, &progress_key, &book.title, window).await?;
        return Ok(());
    }

    bail!("未指定模式：用 --serve（上传到自托管服务端）或 --dry-run（只渲染预览）")
}

/// 服务端模式：把整书（或 --window 起始的 N 章）渲染成 1bpp 位图上传服务端。
/// 之后设备 ↑/↓ 键 → page_turn → 服务端折算进度并经 WS 推送对应页位图。
async fn run_serve(
    reader: &Reader,
    sync: &sync::Sync,
    key: &str,
    title: &str,
    window: Option<usize>,
) -> Result<()> {
    use base64::Engine as _;

    eprintln!("排版全书（{} 章）……", reader.chapters.len());
    let paginator = Paginator::new(font(), reader.style.clone());
    let counts: Vec<usize> = reader
        .chapters
        .iter()
        .map(|c| paginator.paginate(&c.text).len())
        .collect();

    let status = sync.display_status(key).await?;
    if status.chapters != counts {
        eprintln!(
            "服务端缓存与当前排版不一致（缓存 {} 章 / 本地 {} 章），清空重传",
            status.chapters.len(),
            counts.len()
        );
        sync.display_clear(key).await?;
    }
    sync.display_book(key, &counts).await?;

    // 从当前章开始顺序上传；整书模式绕一圈覆盖全书
    let total_chapters = reader.chapters.len();
    let plan: Vec<usize> = match window {
        Some(n) => (reader.chapter..total_chapters).take(n).collect(),
        None => (0..total_chapters).map(|i| (reader.chapter + i) % total_chapters).collect(),
    };
    let total_pages: usize = counts.iter().sum();

    // 待机页（书名+操作说明）：设备开机/关机时显示，与书页同格式
    let standby = render::render_standby_raw1bpp(font(), title);
    sync.display_standby(key, &base64::engine::general_purpose::STANDARD.encode(&standby))
        .await?;
    eprintln!("待机页已上传（书名：{title}）");

    if status.chapters == counts && status.cached_pages == total_pages {
        eprintln!("服务端页缓存已是最新（{total_pages} 页），跳过重传");
        progress::save(&progress::Progress {
            book: key.to_string(),
            chapter: reader.chapter,
            page: reader.page,
        })?;
        if let Err(e) = sync.push(key, reader.chapter, reader.page).await {
            eprintln!("进度上送失败（已存本地）: {e:#}");
        }
        eprintln!("就绪。设备可直接按键阅读");
        return Ok(());
    }

    eprintln!(
        "需要上传 {} 章 / {} 页（按当前阅读位置起）",
        plan.len(),
        plan.iter().map(|&ci| counts[ci]).sum::<usize>()
    );

    const PAGES_PER_POST: usize = 20;
    for (idx, &ci) in plan.iter().enumerate() {
        let pages = paginator.paginate(&reader.chapters[ci].text);
        for (chunk, pages_chunk) in pages.chunks(PAGES_PER_POST).enumerate() {
            let mut batch: Vec<(usize, String)> = Vec::with_capacity(pages_chunk.len());
            for (pi, page) in pages_chunk.iter().enumerate() {
                let page_no = chunk * PAGES_PER_POST + pi;
                let label = format!(
                    "{}/{} · {}%",
                    page_no + 1,
                    pages.len(),
                    book_percent_at(&reader.char_cum, ci, &pages, page_no)
                );
                let raw = render::render_page_raw1bpp(
                    font(),
                    page,
                    &reader.chapters[ci].text,
                    &reader.chapters[ci].title,
                    &label,
                    &reader.style,
                );
                batch.push((page_no, base64::engine::general_purpose::STANDARD.encode(&raw)));
            }
            sync.display_pages(key, ci, &batch).await.with_context(|| {
                format!("上传第 {} 章「{}」失败", ci + 1, reader.chapters[ci].title)
            })?;
        }
        eprintln!(
            "  [{}/{}]「{}」{} 页 ✓",
            idx + 1,
            plan.len(),
            reader.chapters[ci].title,
            pages.len()
        );
    }

    progress::save(&progress::Progress {
        book: key.to_string(),
        chapter: reader.chapter,
        page: reader.page,
    })?;
    if let Err(e) = sync.push(key, reader.chapter, reader.page).await {
        eprintln!("进度上送失败（已存本地）: {e:#}");
    }
    eprintln!(
        "整书就绪。现在直接在设备上按 ↓ 键翻页阅读（服务端会推页显示并记住进度）"
    );
    Ok(())
}

/// 任意章节页的全书进度（与 Reader::book_percent 同一算法，服务端模式渲染任意章时用）
fn book_percent_at(char_cum: &[usize], chapter: usize, pages: &[layout::Page], page: usize) -> u32 {
    let total = char_cum.last().copied().unwrap_or(0);
    if total == 0 {
        return 0;
    }
    let before = char_cum[chapter];
    let within = pages
        .get(page)
        .and_then(|p| p.lines.first().map(|(s, _)| *s))
        .unwrap_or(0);
    (((before + within) as f64 / total as f64) * 100.0)
        .round()
        .clamp(0.0, 100.0) as u32
}

fn flag_value(args: &[String], name: &str) -> Option<String> {
    args.windows(2).find(|w| w[0] == name).map(|w| w[1].clone())
}
