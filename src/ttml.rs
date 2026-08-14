use anyhow::{Context, Result, bail};
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use std::collections::HashMap;

use crate::lrc::{BackgroundVocal, LyricLine, LyricWord};

#[derive(Debug, Default)]
struct LineBuilder {
    id: Option<String>,
    agent_id: Option<String>,
    start_ms: Option<u64>,
    end_ms: Option<u64>,
    text: String,
    translation: Option<String>,
    words: Vec<LyricWord>,
    background_text: String,
    background_translation: Option<String>,
    background_words: Vec<LyricWord>,
}

#[derive(Debug, Default)]
struct SpanBuilder {
    begin_ms: Option<u64>,
    end_ms: Option<u64>,
    text: String,
    ignored: bool,
    translation: bool,
    background: bool,
    has_child: bool,
}

fn local_name(name: &[u8]) -> &str {
    let name = std::str::from_utf8(name).unwrap_or_default();
    name.rsplit(':').next().unwrap_or(name)
}

fn attr(element: &BytesStart<'_>, wanted: &str) -> Option<String> {
    element
        .attributes()
        .filter_map(|a| a.ok())
        .find_map(|attribute| {
            let key = local_name(attribute.key.as_ref());
            (key == wanted).then(|| String::from_utf8_lossy(attribute.value.as_ref()).into_owned())
        })
}

#[derive(Debug, Clone, Default)]
struct SidecarEntry {
    text: String,
    background_text: String,
}

fn parse_time(value: Option<&str>) -> Option<u64> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    if let Some(number) = value.strip_suffix("ms") {
        return number.parse::<f64>().ok().map(|ms| ms.round() as u64);
    }
    if let Some(number) = value.strip_suffix('s') {
        return number
            .parse::<f64>()
            .ok()
            .map(|seconds| (seconds * 1_000.0).round() as u64);
    }
    let mut parts = value.split(':');
    let first = parts.next()?;
    let second = parts.next();
    let third = parts.next();
    let seconds = match (second, third, parts.next()) {
        (None, None, None) => first.parse::<f64>().ok()?,
        (Some(seconds), None, None) => {
            first.parse::<f64>().ok()? * 60.0 + seconds.parse::<f64>().ok()?
        }
        (Some(minutes), Some(seconds), None) => {
            first.parse::<f64>().ok()? * 3_600.0
                + minutes.parse::<f64>().ok()? * 60.0
                + seconds.parse::<f64>().ok()?
        }
        _ => return None,
    };
    Some((seconds * 1_000.0).round() as u64)
}

fn normalize_fragment(raw: &str) -> String {
    if raw.trim().is_empty() {
        return if raw.contains(['\n', '\r', '\t']) {
            String::new()
        } else {
            " ".to_owned()
        };
    }
    let mut output = String::with_capacity(raw.len());
    let mut pending_space = false;
    for character in raw.chars() {
        if character.is_whitespace() {
            pending_space = true;
        } else {
            if pending_space && !output.is_empty() {
                output.push(' ');
            }
            output.push(character);
            pending_space = false;
        }
    }
    if pending_space && !output.is_empty() {
        output.push(' ');
    }
    output
}

fn append_fragment(
    raw: &str,
    spans: &mut [SpanBuilder],
    line: &mut Option<LineBuilder>,
    sidecar_target: &Option<String>,
    sidecar_text: &mut String,
    sidecar_background_depth: usize,
    sidecar_background_text: &mut String,
) {
    let fragment = normalize_fragment(raw);
    if fragment.is_empty() {
        return;
    }
    if sidecar_target.is_some() {
        if sidecar_background_depth > 0 {
            sidecar_background_text.push_str(&fragment);
        } else {
            sidecar_text.push_str(&fragment);
        }
    }
    if let Some(span) = spans.last_mut() {
        span.text.push_str(&fragment);
    } else if let Some(line) = line.as_mut() {
        if fragment.chars().all(char::is_whitespace) && !line.words.is_empty() {
            line.words.last_mut().unwrap().text.push_str(&fragment);
            line.text.push_str(&fragment);
        } else {
            line.text.push_str(&fragment);
        }
    }
}

fn trim_words_to_text(mut words: Vec<LyricWord>, text: &str) -> Vec<LyricWord> {
    let mut word_bytes: usize = words.iter().map(|word| word.text.len()).sum();
    while word_bytes > text.len() {
        let Some(last) = words.last_mut() else {
            break;
        };
        let Some(character) = last.text.pop() else {
            words.pop();
            continue;
        };
        word_bytes -= character.len_utf8();
    }
    words
}
fn finalize_line(
    builder: LineBuilder,
    sidecar: &HashMap<String, SidecarEntry>,
) -> Option<LyricLine> {
    let text = builder.text.trim().to_owned();
    if text.is_empty() {
        return None;
    }
    let word_start = builder.words.iter().map(|word| word.start_ms).min();
    let word_end = builder.words.iter().map(|word| word.end_ms).max();
    let start_ms = builder.start_ms.or(word_start).unwrap_or(0);
    let end_ms = builder
        .end_ms
        .or(word_end)
        .unwrap_or(start_ms + 5_000)
        .max(start_ms + 1);
    let sidecar_entry = builder.id.as_ref().and_then(|id| sidecar.get(id));
    let translation = builder.translation.or_else(|| {
        sidecar_entry
            .map(|entry| entry.text.clone())
            .filter(|text| !text.trim().is_empty())
    });
    let background_text = if builder.background_text.trim().is_empty() {
        sidecar_entry
            .map(|entry| entry.background_text.clone())
            .unwrap_or_default()
    } else {
        builder.background_text.clone()
    };
    let mut words = if builder.words.is_empty() {
        vec![LyricWord {
            start_ms,
            end_ms,
            text: text.clone(),
        }]
    } else {
        builder.words
    };
    words = trim_words_to_text(words, &text);
    let background_vocal = if background_text.trim().is_empty() {
        None
    } else {
        let background_start = builder
            .background_words
            .iter()
            .map(|word| word.start_ms)
            .min()
            .unwrap_or(start_ms);
        let background_end = builder
            .background_words
            .iter()
            .map(|word| word.end_ms)
            .max()
            .unwrap_or(end_ms)
            .max(background_start + 1);
        Some(BackgroundVocal {
            start_ms: background_start,
            end_ms: background_end,
            text: background_text.trim().to_owned(),
            translation: builder.background_translation,
            words: trim_words_to_text(builder.background_words, &background_text),
        })
    };
    Some(LyricLine {
        start_ms,
        end_ms,
        text,
        translation,
        agent_id: builder.agent_id,
        is_duet: false,
        is_background: false,
        background_vocal,
        words,
    })
}

pub fn looks_like_ttml(input: &str) -> bool {
    let mut reader = Reader::from_str(input);
    reader.config_mut().trim_text(false);
    loop {
        match reader.read_event() {
            Ok(Event::Start(element)) | Ok(Event::Empty(element)) => {
                return local_name(element.name().as_ref()) == "tt";
            }
            Ok(Event::Eof) | Err(_) => return false,
            Ok(_) => {}
        }
    }
}

pub fn parse_ttml(input: &str) -> Result<Vec<LyricLine>> {
    if input.trim().is_empty() {
        bail!("no TTML content");
    }
    let mut reader = Reader::from_str(input);
    reader.config_mut().trim_text(false);
    let mut current_line: Option<LineBuilder> = None;
    let mut spans: Vec<SpanBuilder> = Vec::new();
    let mut lines = Vec::new();
    let mut sidecar: HashMap<String, SidecarEntry> = HashMap::new();
    let mut agents: HashMap<String, String> = HashMap::new();
    let mut translation_depth = 0usize;
    let mut sidecar_target: Option<String> = None;
    let mut sidecar_text = String::new();
    let mut sidecar_background_depth = 0usize;
    let mut sidecar_background_text = String::new();

    loop {
        match reader.read_event().context("read TTML XML event")? {
            Event::Start(element) => {
                let element_name = element.name();
                let name = local_name(element_name.as_ref());
                if current_line.is_none() && name == "translation" {
                    translation_depth += 1;
                }
                if current_line.is_none() && name == "agent" {
                    if let (Some(id), Some(agent_type)) =
                        (attr(&element, "id"), attr(&element, "type"))
                    {
                        agents.insert(id, agent_type);
                    }
                }
                if current_line.is_none() && name == "text" && translation_depth > 0 {
                    sidecar_target = attr(&element, "for");
                    sidecar_text.clear();
                    sidecar_background_text.clear();
                    sidecar_background_depth = 0;
                }
                if current_line.is_none()
                    && sidecar_target.is_some()
                    && name == "span"
                    && attr(&element, "role").as_deref() == Some("x-bg")
                {
                    sidecar_background_depth += 1;
                }
                match name {
                    "p" if current_line.is_none() => {
                        current_line = Some(LineBuilder {
                            id: attr(&element, "key"),
                            agent_id: attr(&element, "agent"),
                            start_ms: parse_time(attr(&element, "begin").as_deref()),
                            end_ms: parse_time(attr(&element, "end").as_deref()),
                            ..LineBuilder::default()
                        });
                    }
                    "span" if current_line.is_some() => {
                        let parent_ignored = spans.last().map(|span| span.ignored).unwrap_or(false);
                        let parent_background =
                            spans.last().map(|span| span.background).unwrap_or(false);
                        let parent_translation =
                            spans.last().map(|span| span.translation).unwrap_or(false);
                        if let Some(parent) = spans.last_mut() {
                            parent.has_child = true;
                        }
                        let role = attr(&element, "role");
                        spans.push(SpanBuilder {
                            begin_ms: parse_time(attr(&element, "begin").as_deref()),
                            end_ms: parse_time(attr(&element, "end").as_deref()),
                            ignored: parent_ignored || role.as_deref() == Some("x-bg"),
                            translation: parent_translation
                                || role.as_deref() == Some("x-translation"),
                            background: parent_background || role.as_deref() == Some("x-bg"),
                            ..SpanBuilder::default()
                        });
                    }
                    _ => {}
                }
            }
            Event::Empty(element) => {
                let element_name = element.name();
                let name = local_name(element_name.as_ref());
                if current_line.is_none() && name == "agent" {
                    if let (Some(id), Some(agent_type)) =
                        (attr(&element, "id"), attr(&element, "type"))
                    {
                        agents.insert(id, agent_type);
                    }
                }
                if name == "p" && current_line.is_none() {
                    let builder = LineBuilder {
                        id: attr(&element, "key"),
                        agent_id: attr(&element, "agent"),
                        start_ms: parse_time(attr(&element, "begin").as_deref()),
                        end_ms: parse_time(attr(&element, "end").as_deref()),
                        ..LineBuilder::default()
                    };
                    if let Some(line) = finalize_line(builder, &sidecar) {
                        lines.push(line);
                    }
                }
            }
            Event::Text(text) => append_fragment(
                &text.unescape().context("decode TTML text")?,
                &mut spans,
                &mut current_line,
                &sidecar_target,
                &mut sidecar_text,
                sidecar_background_depth,
                &mut sidecar_background_text,
            ),
            Event::CData(text) => append_fragment(
                &String::from_utf8_lossy(text.as_ref()),
                &mut spans,
                &mut current_line,
                &sidecar_target,
                &mut sidecar_text,
                sidecar_background_depth,
                &mut sidecar_background_text,
            ),
            Event::End(element) => {
                let element_name = element.name();
                let name = local_name(element_name.as_ref());
                if name == "text" && current_line.is_none() {
                    if let Some(target) = sidecar_target.take() {
                        let entry = SidecarEntry {
                            text: sidecar_text.trim().to_owned(),
                            background_text: sidecar_background_text.trim().to_owned(),
                        };
                        if !entry.text.is_empty() || !entry.background_text.is_empty() {
                            sidecar.insert(target, entry);
                        }
                    }
                    sidecar_text.clear();
                    sidecar_background_text.clear();
                    sidecar_background_depth = 0;
                }
                if name == "span"
                    && current_line.is_none()
                    && sidecar_target.is_some()
                    && sidecar_background_depth > 0
                {
                    sidecar_background_depth -= 1;
                }
                if name == "translation" && current_line.is_none() {
                    translation_depth = translation_depth.saturating_sub(1);
                }
                match name {
                    "span" => {
                        if let Some(span) = spans.pop() {
                            if let Some(parent) = spans.last_mut() {
                                if span.background || !span.ignored {
                                    parent.text.push_str(&span.text);
                                }
                            } else if let Some(line) = current_line.as_mut() {
                                if span.background {
                                    if span.translation {
                                        let value = span.text.trim();
                                        if !value.is_empty() {
                                            line.background_translation = Some(value.to_owned());
                                        }
                                    } else {
                                        line.background_text.push_str(&span.text);
                                    }
                                } else if span.translation {
                                    let value = span.text.trim();
                                    if !value.is_empty() {
                                        line.translation = Some(value.to_owned());
                                    }
                                } else if !span.ignored {
                                    line.text.push_str(&span.text);
                                }
                            }
                            if !span.translation && !span.has_child {
                                if let (Some(start_ms), Some(end_ms)) = (span.begin_ms, span.end_ms)
                                {
                                    if !span.text.trim().is_empty() {
                                        if let Some(line) = current_line.as_mut() {
                                            let word = LyricWord {
                                                start_ms,
                                                end_ms: end_ms.max(start_ms + 1),
                                                text: span.text,
                                            };
                                            if span.background {
                                                line.background_words.push(word);
                                            } else if !span.ignored {
                                                line.words.push(word);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    "p" => {
                        spans.clear();
                        if let Some(builder) = current_line.take() {
                            if let Some(line) = finalize_line(builder, &sidecar) {
                                lines.push(line);
                            }
                        }
                    }
                    _ => {}
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }

    if lines.is_empty() {
        bail!("no TTML lyric lines");
    }
    lines.sort_by_key(|line| line.start_ms);
    let mut last_agent: Option<String> = None;
    let mut last_duet = false;
    for line in &mut lines {
        let agent = line.agent_id.clone().unwrap_or_else(|| "v1".to_owned());
        let is_other = agents
            .get(&agent)
            .map(|kind| kind == "other")
            .unwrap_or(false);
        if last_agent.is_none() {
            line.is_duet = is_other;
        } else if last_agent.as_deref() == Some(agent.as_str()) {
            line.is_duet = last_duet;
        } else {
            line.is_duet = !last_duet;
        }
        last_duet = line.is_duet;
        last_agent = Some(agent);
    }
    for index in 0..lines.len().saturating_sub(1) {
        if lines[index].end_ms <= lines[index].start_ms {
            lines[index].end_ms = lines[index + 1].start_ms.max(lines[index].start_ms + 1);
        }
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_spaces_between_timed_english_spans() {
        let input = r#"<tt><body><p begin="1s" end="4s"><span begin="1s" end="2s">We’re</span> <span begin="2s" end="4s">wild</span></p></body></tt>"#;
        let lines = parse_ttml(input).unwrap();
        assert_eq!(lines[0].text, "We’re wild");
        assert_eq!(
            lines[0]
                .words
                .iter()
                .map(|word| word.text.as_str())
                .collect::<String>(),
            lines[0].text
        );
    }

    #[test]
    fn parses_namespaced_word_ttml() {
        let input = r#"<tt xmlns="http://www.w3.org/ns/ttml" xmlns:xml="http://www.w3.org/XML/1998/namespace"><body><p xml:begin="00:01.000" xml:end="00:03.000"><span xml:begin="00:01.000" xml:end="00:01.500">Hel</span><span xml:begin="00:01.500" xml:end="00:02.000">lo </span><span xml:begin="00:02.000" xml:end="00:03.000">world</span></p></body></tt>"#;
        let lines = parse_ttml(input).unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "Hello world");
        assert_eq!(lines[0].words.len(), 3);
        assert_eq!(lines[0].words[1].start_ms, 1_500);
        assert_eq!(lines[0].end_ms, 3_000);
    }

    #[test]
    fn parses_inline_translation_without_mixing_it_into_main_line() {
        let input = r#"<tt><body><p begin="1s" end="3s"><span begin="1s" end="2s">main </span><span ttm:role="x-translation">翻译</span></p></body></tt>"#;
        let lines = parse_ttml(input).unwrap();
        assert_eq!(lines[0].text, "main");
        assert_eq!(lines[0].translation.as_deref(), Some("翻译"));
    }

    #[test]
    fn parses_sidecar_translation_by_line_key() {
        let input = r#"<tt><head><metadata><iTunesMetadata><translations><translation><text for="L1">译文</text></translation></translations></iTunesMetadata></metadata></head><body><p begin="1s" end="3s" itunes:key="L1">main</p></body></tt>"#;
        let lines = parse_ttml(input).unwrap();
        assert_eq!(lines[0].translation.as_deref(), Some("译文"));
    }

    #[test]
    fn supports_seconds_and_fallback_line_word() {
        let input = r#"<tt><body><p begin="2s" end="4s">Line</p></body></tt>"#;
        let lines = parse_ttml(input).unwrap();
        assert_eq!(lines[0].start_ms, 2_000);
        assert_eq!(lines[0].end_ms, 4_000);
        assert_eq!(lines[0].words[0].text, "Line");
    }

    #[test]
    fn parses_background_vocal_without_mixing_main_line() {
        let input = r#"<tt><body><p begin="1s" end="3s"><span begin="1s" end="2s">main </span><span ttm:role="x-bg"><span begin="2s" end="3s">background</span></span></p></body></tt>"#;
        let lines = parse_ttml(input).unwrap();
        assert_eq!(lines[0].text, "main");
        assert_eq!(lines[0].words.len(), 1);
        let background = lines[0].background_vocal.as_ref().unwrap();
        assert_eq!(background.text, "background");
        assert_eq!(background.words.len(), 1);
        assert_eq!(background.words[0].start_ms, 2_000);
    }

    #[test]
    fn parses_sidecar_background_vocal() {
        let input = r#"<tt><head><metadata><iTunesMetadata><translations><translation><text for="L1">译文 <span ttm:role="x-bg">(伴唱)</span></text></translation></translations></iTunesMetadata></metadata></head><body><p begin="1s" end="3s" itunes:key="L1">main</p></body></tt>"#;
        let lines = parse_ttml(input).unwrap();
        assert_eq!(lines[0].translation.as_deref(), Some("译文"));
        assert_eq!(lines[0].background_vocal.as_ref().unwrap().text, "(伴唱)");
    }

    #[test]
    fn records_timed_leaf_spans_nested_for_styling() {
        let input = r#"<tt><body><p begin="1s" end="3s"><span tts:fontStyle="italic"><span begin="1s" end="2s">nested</span></span></p></body></tt>"#;
        let lines = parse_ttml(input).unwrap();
        assert_eq!(lines[0].text, "nested");
        assert_eq!(lines[0].words.len(), 1);
        assert_eq!(lines[0].words[0].start_ms, 1_000);
    }

    #[test]
    fn sniffs_ttml_root_independent_of_file_extension() {
        assert!(looks_like_ttml(
            "<?xml version=\"1.0\"?><tt xmlns=\"http://www.w3.org/ns/ttml\"><body/></tt>"
        ));
        assert!(!looks_like_ttml("[00:01]not XML"));
    }
}
