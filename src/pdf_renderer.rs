use anyhow::{Context, Result};
use skia_safe::pdf;
use skia_safe::textlayout::{
    FontCollection, Paragraph, ParagraphBuilder, ParagraphStyle, TextAlign, TextStyle,
};
use skia_safe::{Canvas, Color, FontMgr, FontStyle, Paint, Point};
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use crate::lrc::LyricLine;

const A4_WIDTH: f32 = 595.28;
const A4_HEIGHT: f32 = 841.89;
const PAGE_MARGIN: f32 = 24.0;
const COLUMN_GUTTER: f32 = 22.0;
const FOOTER_HEIGHT: f32 = 14.0;
const CONTENT_TOP: f32 = 48.0;
const BLOCK_GAP: f32 = 11.0;
const TRANSLATION_GAP: f32 = 3.0;
const BACKGROUND_GAP: f32 = 5.0;
const BACKGROUND_TRANSLATION_GAP: f32 = 2.0;
const MAIN_FONT_SIZE: f32 = 11.5;
const TRANSLATION_FONT_SIZE: f32 = 8.5;
const BACKGROUND_FONT_SIZE: f32 = 9.5;

const FONT_FAMILIES: &[&str] = &[
    "PingFang SC",
    "Microsoft YaHei",
    "Noto Sans CJK SC",
    "sans-serif",
];

#[derive(Clone, Debug)]
pub struct PdfOptions {
    pub title: Option<String>,
    pub render_translation: bool,
    pub render_background_vocal: bool,
    pub excluded_lines: Vec<usize>,
    pub speed_printer: bool,
}

impl Default for PdfOptions {
    fn default() -> Self {
        Self {
            title: None,
            render_translation: true,
            render_background_vocal: true,
            excluded_lines: Vec::new(),
            speed_printer: false,
        }
    }
}

struct LineBlock {
    main: Paragraph,
    translation: Option<Paragraph>,
    background: Option<Paragraph>,
    background_translation: Option<Paragraph>,
}

impl LineBlock {
    fn height(&self) -> f32 {
        let mut height = self.main.height();
        if let Some(paragraph) = &self.translation {
            height += TRANSLATION_GAP + paragraph.height();
        }
        if let Some(paragraph) = &self.background {
            height += BACKGROUND_GAP + paragraph.height();
        }
        if let Some(paragraph) = &self.background_translation {
            height += BACKGROUND_TRANSLATION_GAP + paragraph.height();
        }
        height
    }

    fn paint(&self, canvas: &Canvas, x: f32, mut y: f32) {
        self.main.paint(canvas, Point::new(x, y));
        y += self.main.height();
        if let Some(paragraph) = &self.translation {
            y += TRANSLATION_GAP;
            paragraph.paint(canvas, Point::new(x, y));
            y += paragraph.height();
        }
        if let Some(paragraph) = &self.background {
            y += BACKGROUND_GAP;
            paragraph.paint(canvas, Point::new(x, y));
            y += paragraph.height();
        }
        if let Some(paragraph) = &self.background_translation {
            y += BACKGROUND_TRANSLATION_GAP;
            paragraph.paint(canvas, Point::new(x, y));
        }
    }
}

#[derive(Default)]
struct PageLayout {
    columns: [Vec<usize>; 2],
}
pub fn render_lyrics_pdf(
    output: &Path,
    lines: &[LyricLine],
    options: &PdfOptions,
) -> Result<usize> {
    let title = options
        .title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or("歌词");
    let column_width = (A4_WIDTH - PAGE_MARGIN * 2.0 - COLUMN_GUTTER) / 2.0;
    let collection = font_collection();
    let blocks = build_line_blocks(lines, options, column_width, &collection);
    let layouts = paginate(
        &blocks.iter().map(LineBlock::height).collect::<Vec<_>>(),
        CONTENT_TOP,
    );

    let file = File::create(output).with_context(|| format!("create PDF {}", output.display()))?;
    let mut writer = BufWriter::new(file);
    let metadata = pdf::Metadata {
        title: title.to_owned(),
        subject: "Printable lyrics".to_owned(),
        creator: "GoosyRenderer".to_owned(),
        producer: "GoosyRenderer with Skia/PDF".to_owned(),
        lang: "zh-CN".to_owned(),
        ..Default::default()
    };
    let mut document = pdf::new_document(&mut writer, Some(&metadata));
    for (page_index, layout) in layouts.iter().enumerate() {
        let mut page = document.begin_page((A4_WIDTH, A4_HEIGHT), None);
        draw_page(
            page.canvas(),
            page_index,
            title,
            column_width,
            layout,
            &blocks,
            &collection,
            options.speed_printer,
        );
        document = page.end_page();
    }
    document.close();
    writer
        .flush()
        .with_context(|| format!("flush PDF {}", output.display()))?;
    Ok(layouts.len())
}

fn font_collection() -> FontCollection {
    let mut collection = FontCollection::new();
    collection.set_default_font_manager(FontMgr::new(), None);
    collection
}

fn text_style(font_size: f32, font_style: FontStyle, color: Color, height: f32) -> TextStyle {
    let mut style = TextStyle::new();
    style
        .set_font_families(FONT_FAMILIES)
        .set_font_size(font_size)
        .set_font_style(font_style)
        .set_color(color)
        .set_height(height)
        .set_height_override(true);
    style
}

fn build_paragraph(
    text: &str,
    style: &TextStyle,
    align: TextAlign,
    width: f32,
    collection: &FontCollection,
    max_lines: Option<usize>,
) -> Paragraph {
    let mut paragraph_style = ParagraphStyle::new();
    paragraph_style.set_text_align(align).set_text_style(style);
    if let Some(max_lines) = max_lines {
        paragraph_style.set_max_lines(max_lines).set_ellipsis("…");
    }
    let mut builder = ParagraphBuilder::new(&paragraph_style, collection.clone());
    builder.push_style(style).add_text(text);
    let mut paragraph = builder.build();
    paragraph.layout(width);
    paragraph
}

fn build_line_blocks(
    lines: &[LyricLine],
    options: &PdfOptions,
    column_width: f32,
    collection: &FontCollection,
) -> Vec<LineBlock> {
    let excluded = options
        .excluded_lines
        .iter()
        .copied()
        .collect::<HashSet<_>>();
    let (main_color, translation_color, background_color) = if options.speed_printer {
        (Color::BLACK, Color::BLACK, Color::BLACK)
    } else {
        (
            Color::from_rgb(28, 28, 32),
            Color::from_rgb(105, 105, 112),
            Color::from_rgb(76, 76, 84),
        )
    };
    let main_style = text_style(MAIN_FONT_SIZE, FontStyle::bold(), main_color, 1.28);
    let translation_style = text_style(
        TRANSLATION_FONT_SIZE,
        FontStyle::normal(),
        translation_color,
        1.32,
    );
    let background_style = text_style(
        BACKGROUND_FONT_SIZE,
        FontStyle::bold(),
        background_color,
        1.28,
    );

    lines
        .iter()
        .enumerate()
        .filter(|(index, _)| !excluded.contains(index))
        .filter(|(_, line)| !line.text.trim().is_empty())
        .map(|(_, line)| {
            let align = if line.is_duet {
                TextAlign::Right
            } else {
                TextAlign::Left
            };
            let translation = options
                .render_translation
                .then(|| line.translation.as_deref())
                .flatten()
                .filter(|text| !text.trim().is_empty())
                .map(|text| {
                    build_paragraph(
                        text.trim(),
                        &translation_style,
                        align,
                        column_width,
                        collection,
                        None,
                    )
                });
            let background = options
                .render_background_vocal
                .then(|| line.background_vocal.as_ref())
                .flatten();
            let background_paragraph = background.map(|background| {
                build_paragraph(
                    &format!("伴唱 · {}", background.text.trim()),
                    &background_style,
                    align,
                    column_width,
                    collection,
                    None,
                )
            });
            let background_translation = background
                .filter(|_| options.render_translation)
                .and_then(|background| background.translation.as_deref())
                .filter(|text| !text.trim().is_empty())
                .map(|text| {
                    build_paragraph(
                        text.trim(),
                        &translation_style,
                        align,
                        column_width,
                        collection,
                        None,
                    )
                });
            LineBlock {
                main: build_paragraph(
                    line.text.trim(),
                    &main_style,
                    align,
                    column_width,
                    collection,
                    None,
                ),
                translation,
                background: background_paragraph,
                background_translation,
            }
        })
        .collect()
}

fn paginate(heights: &[f32], first_page_top: f32) -> Vec<PageLayout> {
    let content_bottom = A4_HEIGHT - PAGE_MARGIN - FOOTER_HEIGHT;
    let mut pages = vec![PageLayout::default()];
    let mut page_index = 0usize;
    let mut column = 0usize;
    let mut y = first_page_top;
    for (block_index, height) in heights.iter().copied().enumerate() {
        let mut gap = if pages[page_index].columns[column].is_empty() {
            0.0
        } else {
            BLOCK_GAP
        };
        if y + gap + height > content_bottom && !pages[page_index].columns[column].is_empty() {
            if column == 0 {
                column = 1;
            } else {
                pages.push(PageLayout::default());
                page_index += 1;
                column = 0;
            }
            y = first_page_top;
            gap = 0.0;
        }
        y += gap;
        pages[page_index].columns[column].push(block_index);
        y += height;
    }
    pages
}

#[allow(clippy::too_many_arguments)]
fn draw_page(
    canvas: &Canvas,
    page_index: usize,
    title: &str,
    column_width: f32,
    layout: &PageLayout,
    blocks: &[LineBlock],
    collection: &FontCollection,
    speed_printer: bool,
) {
    canvas.clear(Color::WHITE);
    let content_width = A4_WIDTH - PAGE_MARGIN * 2.0;
    let text_color = if speed_printer {
        Color::BLACK
    } else {
        Color::from_rgb(110, 110, 118)
    };
    let header = build_paragraph(
        title,
        &text_style(8.5, FontStyle::bold(), text_color, 1.1),
        TextAlign::Left,
        content_width,
        collection,
        Some(1),
    );
    header.paint(canvas, Point::new(PAGE_MARGIN, 12.0));

    let mut rule = Paint::default();
    rule.set_color(if speed_printer {
        Color::BLACK
    } else {
        Color::from_rgb(218, 218, 222)
    })
    .set_stroke_width(if speed_printer { 0.8 } else { 0.55 });
    canvas.draw_line(
        Point::new(A4_WIDTH / 2.0, CONTENT_TOP),
        Point::new(A4_WIDTH / 2.0, A4_HEIGHT - PAGE_MARGIN - FOOTER_HEIGHT),
        &rule,
    );

    for column in 0..2 {
        let x = PAGE_MARGIN + column as f32 * (column_width + COLUMN_GUTTER);
        let mut y = CONTENT_TOP;
        for (position, block_index) in layout.columns[column].iter().copied().enumerate() {
            if position > 0 {
                y += BLOCK_GAP;
            }
            let block = &blocks[block_index];
            block.paint(canvas, x, y);
            y += block.height();
        }
    }

    let footer = build_paragraph(
        &(page_index + 1).to_string(),
        &text_style(7.5, FontStyle::normal(), text_color, 1.0),
        TextAlign::Center,
        content_width,
        collection,
        Some(1),
    );
    footer.paint(
        canvas,
        Point::new(PAGE_MARGIN, A4_HEIGHT - PAGE_MARGIN + 1.0),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str) -> LyricLine {
        LyricLine {
            start_ms: 0,
            end_ms: 1_000,
            text: text.to_owned(),
            translation: Some("翻译".to_owned()),
            agent_id: None,
            is_duet: false,
            is_background: false,
            background_vocal: None,
            words: Vec::new(),
        }
    }

    #[test]
    fn pagination_fills_both_columns_before_new_page() {
        let pages = paginate(&[200.0; 7], 100.0);
        assert_eq!(pages.len(), 2);
        assert!(!pages[0].columns[0].is_empty());
        assert!(!pages[0].columns[1].is_empty());
        assert_eq!(pages[1].columns[0].len(), 1);
    }

    #[test]
    fn writes_a_valid_pdf_document() -> Result<()> {
        let output = std::env::temp_dir().join(format!(
            "goosy-pdf-test-{}-{}.pdf",
            std::process::id(),
            std::thread::current().name().unwrap_or("thread")
        ));
        let pages = render_lyrics_pdf(
            &output,
            &[line("第一行歌词"), line("第二行歌词")],
            &PdfOptions {
                title: Some("测试歌曲".to_owned()),
                ..Default::default()
            },
        )?;
        let bytes = std::fs::read(&output)?;
        assert_eq!(pages, 1);
        assert!(bytes.starts_with(b"%PDF-"));
        assert!(bytes.len() > 1_000);
        std::fs::remove_file(output)?;
        Ok(())
    }
}
