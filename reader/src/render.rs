//! 渲染：Page → 400×300 1bpp PNG（白底黑字，页眉章节名+页码）

use ab_glyph::{Font, FontRef, Glyph, ScaleFont};
use image::Luma;

use crate::layout::{CANVAS_H, CANVAS_W, MARGIN_TOP, MARGIN_X, Page, PageStyle};

/// 页脚：左边写当前批次位置，右边写操作提示（仅离线批量模式用）
#[derive(Debug, Clone, Copy, Default)]
pub struct Footer<'a> {
    pub left: &'a str,
    pub right: &'a str,
}

#[allow(dead_code)]
pub fn render_page(
    font: &FontRef<'static>,
    page: &Page,
    text: &[char],
    header: &str,
    page_label: &str,
    style: &PageStyle,
) -> Vec<u8> {
    render_page_with_footer(font, page, text, header, page_label, style, None)
}

pub fn render_page_with_footer(
    font: &FontRef<'static>,
    page: &Page,
    text: &[char],
    header: &str,
    page_label: &str,
    style: &PageStyle,
    footer: Option<Footer<'_>>,
) -> Vec<u8> {
    let img = render_page_image(font, page, text, header, page_label, style, footer);
    let mut png = Vec::new();
    image::DynamicImage::ImageLuma8(img)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .expect("PNG 编码失败");
    png
}

/// 灰度图 → 设备 1bpp 位图（行优先、MSB 先行、bit=1 黑，阈值 128）
fn pack_1bpp(img: &image::GrayImage) -> Vec<u8> {
    let w = img.width() as usize;
    let bytes_per_row = w.div_ceil(8);
    let mut out = vec![0u8; bytes_per_row * img.height() as usize];
    for (i, p) in img.pixels().enumerate() {
        if p[0] < 128 {
            let x = i % w;
            let y = i / w;
            out[y * bytes_per_row + x / 8] |= 0x80u8 >> (x % 8);
        }
    }
    out
}

/// 待机页（书名 + 操作说明），与书页同格式 1bpp
pub fn render_standby_raw1bpp(font: &FontRef<'static>, title: &str) -> Vec<u8> {
    let mut img = image::GrayImage::from_pixel(CANVAS_W, CANVAS_H, Luma([255u8]));

    let heading = format!("《{}》", title);
    let hs = 44.0f32;
    let hw = measure(font, &heading, hs);
    draw_text(&mut img, font, &heading, hs, (CANVAS_W as f32 - hw) / 2.0, 40.0);

    for x in 60..(CANVAS_W - 60) {
        img.put_pixel(x, 118, Luma([120u8]));
    }

    let size = 20.0f32;
    for (text, y) in [
        ("短按确认键：开始阅读", 152.0f32),
        ("按住确认键：说话", 190.0),
        ("上键 上一页    下键 下一页", 228.0),
    ] {
        let w = measure(font, text, size);
        draw_text(&mut img, font, text, size, (CANVAS_W as f32 - w) / 2.0, y);
    }

    pack_1bpp(&img)
}

/// 渲染成设备 WriteRaw1bpp 格式的裸位图：行优先、MSB 先行、bit=1 为黑。
/// 灰度抗锯齿按 50% 阈值二值化。
pub fn render_page_raw1bpp(
    font: &FontRef<'static>,
    page: &Page,
    text: &[char],
    header: &str,
    page_label: &str,
    style: &PageStyle,
) -> Vec<u8> {
    let img = render_page_image(font, page, text, header, page_label, style, None);
    pack_1bpp(&img)
}

fn render_page_image(
    font: &FontRef<'static>,
    page: &Page,
    text: &[char],
    header: &str,
    page_label: &str,
    style: &PageStyle,
    footer: Option<Footer<'_>>,
) -> image::GrayImage {
    let mut img = image::GrayImage::from_pixel(CANVAS_W, CANVAS_H, Luma([255u8]));

    // 页眉：章节名（左）+ 页码·全书进度（右），12px
    let header_size = 12.0f32;
    draw_text(&mut img, font, header, header_size, MARGIN_X, 7.0);
    let label_w = measure(font, page_label, header_size);
    draw_text(
        &mut img,
        font,
        page_label,
        header_size,
        CANVAS_W as f32 - MARGIN_X - label_w,
        7.0,
    );
    // 页眉分隔线
    for x in (MARGIN_X as u32)..(CANVAS_W - MARGIN_X as u32) {
        img.put_pixel(x, 23, Luma([170u8]));
    }

    // 正文
    let lh = style.font_size * style.line_gap;
    for (row, &(start, len)) in page.lines.iter().enumerate() {
        let y = MARGIN_TOP + row as f32 * lh;
        let line: String = text[start..start + len].iter().collect();
        draw_text(&mut img, font, &line, style.font_size, MARGIN_X, y);
    }

    // 页脚（11px，灰色），只在离线批量模式出现
    if let Some(f) = footer {
        let fs = 11.0f32;
        let y = CANVAS_H as f32 - 14.0;
        draw_text(&mut img, font, f.left, fs, MARGIN_X, y);
        let w = measure(font, f.right, fs);
        draw_text(
            &mut img,
            font,
            f.right,
            fs,
            CANVAS_W as f32 - MARGIN_X - w,
            y,
        );
    }

    img
}

fn draw_text(
    img: &mut image::GrayImage,
    font: &FontRef<'static>,
    s: &str,
    size: f32,
    x: f32,
    y: f32,
) {
    let scale = ab_glyph::PxScale::from(size);
    let scaled = font.as_scaled(size);
    let baseline = y + scaled.ascent();
    let mut cx = x;
    for c in s.chars() {
        let gid = font.glyph_id(c);
        let glyph = Glyph {
            id: gid,
            scale,
            position: ab_glyph::point(cx, baseline),
        };
        if let Some(og) = font.outline_glyph(glyph) {
            let bmin = og.px_bounds().min;
            og.draw(|px, py, coverage| {
                let ix = (bmin.x as i64 + px as i64).max(0);
                let iy = (bmin.y as i64 + py as i64).max(0);
                if (ix as u64) < img.width() as u64 && (iy as u64) < img.height() as u64 {
                    let v = (255.0 - coverage * 255.0) as u8;
                    img.put_pixel(ix as u32, iy as u32, Luma([v]));
                }
            });
            cx += scaled.h_advance(gid);
        } else {
            cx += size * 0.5; // 缺字占位
        }
    }
}

fn measure(font: &FontRef<'static>, s: &str, size: f32) -> f32 {
    let scaled = font.as_scaled(size);
    s.chars()
        .map(|c| {
            let adv = scaled.h_advance(font.glyph_id(c));
            if adv > 0.0 { adv } else { size * 0.5 }
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Paginator;

    fn load_font() -> FontRef<'static> {
        let bytes: &'static [u8] = Box::leak(
            std::fs::read("../assets/fonts/NotoSansSC-Regular.otf")
                .unwrap()
                .into_boxed_slice(),
        );
        FontRef::try_from_slice(bytes).unwrap()
    }

    #[test]
    fn raw1bpp_matches_device_format() {
        let f = load_font();
        let style = PageStyle::default();
        let text: Vec<char> = "天色渐晚，他推开了那扇木门。风从门缝里挤进来。"
            .chars()
            .collect();
        let pages = Paginator::new(&f, style.clone()).paginate(&text);
        let raw = render_page_raw1bpp(&f, &pages[0], &text, "第一章 起点", "1/2 · 10%", &style);
        // 设备 buffer 尺寸：400×300，每行 50 字节
        assert_eq!(raw.len(), 400 * 300 / 8);
        let dark: u32 = raw.iter().map(|b| b.count_ones()).sum();
        assert!(dark > 200, "应有足量黑位，实际 {dark}");
        assert!(dark < (400 * 300 / 2) as u32, "不应大面积黑，实际 {dark}");
    }

    #[test]
    fn standby_smoke() {
        let f = load_font();
        let raw = render_standby_raw1bpp(&f, "示例书");
        assert_eq!(raw.len(), 400 * 300 / 8);
        let dark: u32 = raw.iter().map(|b| b.count_ones()).sum();
        assert!(dark > 500, "待机页应有文字像素，实际 {dark}");
        std::fs::write("/tmp/note4_standby_sample.bin", &raw).unwrap();
    }

    #[test]
    fn render_smoke() {
        let f = load_font();
        let style = PageStyle::default();
        let text: Vec<char> = "天色渐晚，他推开了那扇木门。风从门缝里挤进来，带着深秋的凉意。"
            .chars()
            .collect();
        let pages = Paginator::new(&f, style.clone()).paginate(&text);
        let png = render_page(&f, &pages[0], &text, "第一章 起点", "1/3 · 12%", &style);
        assert!(png.len() > 500);
        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']));
        std::fs::write("/tmp/note4_page_sample.png", &png).unwrap();
        // 黑字覆盖率：不应全白
        let img = image::load_from_memory(&png).unwrap().to_luma8();
        let dark = img.pixels().filter(|p| p[0] < 128).count();
        assert!(dark > 200, "应有足量笔画像素，实际 {dark}");
    }
}
