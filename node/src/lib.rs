use goosy::{LyricFormat, RenderOptions as RustRenderOptions};
use napi::bindgen_prelude::*;
use napi_derive::napi;
use std::path::PathBuf;

fn format_from_string(format: Option<String>) -> Result<LyricFormat> {
    match format
        .unwrap_or_else(|| "auto".to_owned())
        .to_ascii_lowercase()
        .as_str()
    {
        "auto" => Ok(LyricFormat::Auto),
        "lrc" => Ok(LyricFormat::Lrc),
        "ttml" | "xml" => Ok(LyricFormat::Ttml),
        other => Err(Error::from_reason(format!(
            "unsupported lyric format: {other}"
        ))),
    }
}

#[napi(object)]
pub struct RenderOptions {
    pub song: String,
    pub output: String,
    pub lyrics: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<u32>,
    pub background: Option<String>,
    pub cover: Option<String>,
    pub title: Option<String>,
    pub no_embedded_cover: Option<bool>,
    pub no_audio: Option<bool>,
    pub format: Option<String>,
}

#[napi(object)]
pub struct Word {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

#[napi(object)]
pub struct BackgroundVocal {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
    pub translation: Option<String>,
    pub words: Vec<Word>,
}

#[napi(object)]
pub struct LyricLine {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
    pub translation: Option<String>,
    pub agent_id: Option<String>,
    pub is_duet: bool,
    pub is_background: bool,
    pub background_vocal: Option<BackgroundVocal>,
    pub words: Vec<Word>,
}

fn word(word: &goosy::LyricWord) -> Word {
    Word {
        start_ms: word.start_ms as i64,
        end_ms: word.end_ms as i64,
        text: word.text.clone(),
    }
}

fn line(line: &goosy::LyricLine) -> LyricLine {
    LyricLine {
        start_ms: line.start_ms as i64,
        end_ms: line.end_ms as i64,
        text: line.text.clone(),
        translation: line.translation.clone(),
        agent_id: line.agent_id.clone(),
        is_duet: line.is_duet,
        is_background: line.is_background,
        background_vocal: line
            .background_vocal
            .as_ref()
            .map(|background| BackgroundVocal {
                start_ms: background.start_ms as i64,
                end_ms: background.end_ms as i64,
                text: background.text.clone(),
                translation: background.translation.clone(),
                words: background.words.iter().map(word).collect(),
            }),
        words: line.words.iter().map(word).collect(),
    }
}

#[napi]
pub fn render(options: RenderOptions) -> Result<()> {
    let mut rust = RustRenderOptions::new(options.song, options.output);
    rust.lyrics = options.lyrics.map(PathBuf::from);
    rust.width = options.width.unwrap_or(1920);
    rust.height = options.height.unwrap_or(1080);
    rust.fps = options.fps.unwrap_or(30);
    rust.background = options.background.map(PathBuf::from);
    rust.cover = options.cover.map(PathBuf::from);
    rust.title = options.title;
    rust.no_embedded_cover = options.no_embedded_cover.unwrap_or(false);
    rust.no_audio = options.no_audio.unwrap_or(false);
    rust.format = format_from_string(options.format)?;
    goosy::render(&rust).map_err(|error| Error::from_reason(error.to_string()))
}

#[napi]
pub fn parse_lyrics(input: String, format: Option<String>) -> Result<Vec<LyricLine>> {
    let lines = goosy::parse_lyrics(&input, format_from_string(format)?)
        .map_err(|error| Error::from_reason(error.to_string()))?;
    Ok(lines.iter().map(line).collect())
}
