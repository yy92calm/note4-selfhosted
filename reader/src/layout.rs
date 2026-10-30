//! 排版分页：字符流 → 页面（400×300，1bpp 黑白）
//!
//! 规则：
//! - 行宽 = 画布宽 - 左右边距；行高 = 字号 × 行距系数
//! - 折行：逐字累计宽度，超宽回退
//! - 避头：折行点后若是"不可行首"标点，把它留在本行（允许轻微超宽）
//! - 段首缩进两格；页眉显示章节名/页码

use ab_glyph::{Font, FontRef, ScaleFont};

pub const CANVAS_W: u32 = 400;
pub const CANVAS_H: u32 = 300;
pub const MARGIN_X: f32 = 16.0;
pub const MARGIN_TOP: f32 = 28.0; // 页眉之下开始正文
pub const MARGIN_BOTTOM: f32 = 12.0;

/// 避头字符（不可出现在行首）
const NO_LINE_START: &str = "，。！？；：、」』）】〉》…—·,,!?;:]}>";

#[derive(Debug, Clone)]
pub struct PageStyle {
    pub font_size: f32,
    pub line_gap: f32, // 行距系数
}

impl Default for PageStyle {
    fn default() -> Self {
        Self {
            font_size: 20.0,
            line_gap: 1.45,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Page {
    /// (行首字符下标, 行字符数)
    pub lines: Vec<(usize, usize)>,
}

pub struct Paginator<'a> {
    font: &'a FontRef<'static>,
    pub style: PageStyle,
}

impl<'a> Paginator<'a> {
    pub fn new(font: &'a FontRef<'static>, style: PageStyle) -> Self {
        Self { font, style }
    }

    fn char_width(&self, c: char) -> f32 {
        let scaled = self.font.as_scaled(self.style.font_size);
        let adv = scaled.h_advance(self.font.glyph_id(c));
        if adv > 0.0 {
            adv
        } else {
            // 缺字按半角宽
            self.style.font_size * 0.5
        }
    }

    fn line_height(&self) -> f32 {
        self.style.font_size * self.style.line_gap
    }

    pub fn lines_per_page(&self) -> usize {
        let usable = CANVAS_H as f32 - MARGIN_TOP - MARGIN_BOTTOM;
        ((usable / self.line_height()).floor() as usize).max(1)
    }

    fn max_line_width(&self) -> f32 {
        CANVAS_W as f32 - MARGIN_X * 2.0
    }

    /// 对整章排版。indent = 段首缩进（章内自然段以 \n 分隔，已在 book.rs 去除，这里按参数统一缩进首行以外的段首）。
    /// 返回所有页。
    pub fn paginate(&self, text: &[char]) -> Vec<Page> {
        let lpp = self.lines_per_page();
        let maxw = self.max_line_width();
        let mut lines: Vec<(usize, usize)> = Vec::new();

        let mut i = 0usize; // 当前消费到的位置
        while i < text.len() {
            let line_start = i;
            let mut w = 0f32;
            let mut j = i;
            while j < text.len() {
                let c = text[j];
                if c == '\n' {
                    break; // 段落硬换行
                }
                let cw = self.char_width(c);
                if w + cw > maxw {
                    break;
                }
                w += cw;
                j += 1;
            }
            let mut line_end = j; // 不含
            // 避头：下一行行首若是"不可行首"标点 → 本行收编它（可轻微超宽）
            while line_end < text.len()
                && text[line_end] != '\n'
                && NO_LINE_START.contains(text[line_end])
                && line_end - line_start > 1
            {
                line_end += 1;
            }
            // 整行一个字都放不下（字号过大）→ 硬切一个字防止死循环
            if line_end == line_start {
                line_end = line_start + 1;
            }
            lines.push((line_start, line_end - line_start));
            // 跳过段落换行符
            i = if line_end < text.len() && text[line_end] == '\n' {
                line_end + 1
            } else {
                line_end
            };
        }

        let mut pages: Vec<Page> = Vec::new();
        for chunk in lines.chunks(lpp) {
            pages.push(Page {
                lines: chunk.to_vec(),
            });
        }
        if pages.is_empty() {
            pages.push(Page { lines: vec![] });
        }
        pages
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load_font() -> FontRef<'static> {
        let bytes: &'static [u8] = Box::leak(
            std::fs::read("../assets/fonts/NotoSansSC-Regular.otf")
                .unwrap()
                .into_boxed_slice(),
        );
        FontRef::try_from_slice(bytes).unwrap()
    }

    #[test]
    fn paginate_basic() {
        let f = load_font();
        let p = Paginator::new(&f, PageStyle::default());
        let text: Vec<char> = "天色渐晚，他推开了那扇木门。风从门缝里挤进来。"
            .repeat(20)
            .chars()
            .collect();
        let pages = p.paginate(&text);
        assert!(pages.len() > 1, "长文本应分多页，实际 {}", pages.len());
        let lpp = p.lines_per_page();
        for pg in &pages {
            assert!(pg.lines.len() <= lpp);
        }
        // 拼回去应无损
        let rebuilt: String = pages
            .iter()
            .flat_map(|pg| pg.lines.iter())
            .flat_map(|&(s, n)| text[s..s + n].iter().copied())
            .collect();
        assert_eq!(rebuilt, text.into_iter().collect::<String>());
    }

    #[test]
    fn pages_fit_canvas() {
        let f = load_font();
        for (size, min_lines) in [(16.0, 10usize), (20.0, 8), (24.0, 7)] {
            let p = Paginator::new(
                &f,
                PageStyle {
                    font_size: size,
                    line_gap: 1.45,
                },
            );
            let text: Vec<char> = "测".repeat(3000).chars().collect();
            let pages = p.paginate(&text);
            assert!(!pages.is_empty());
            assert!(p.lines_per_page() >= min_lines, "字号{size}行数过少");
        }
    }
}
