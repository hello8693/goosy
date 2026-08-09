use anyhow::{Result, bail};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundVocal {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    pub translation: Option<String>,
    pub words: Vec<LyricWord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LyricWord {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LyricLine {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    pub translation: Option<String>,
    pub agent_id: Option<String>,
    pub is_duet: bool,
    pub is_background: bool,
    pub background_vocal: Option<BackgroundVocal>,
    pub words: Vec<LyricWord>,
}

/// The visual scan completion moment used to coordinate pre-roll scrolling.
/// Untimed line-level karaoke finishes slightly before the nominal line end,
/// leaving a short handoff window before the next line.
pub fn scan_end_ms(line: &LyricLine) -> u64 {
    if let Some(end_ms) = line.words.iter().map(|word| word.end_ms).max() {
        return end_ms.max(line.start_ms + 1);
    }
    let duration = line.end_ms.saturating_sub(line.start_ms);
    let lead = (duration / 8)
        .clamp(120, 400)
        .min(duration.saturating_sub(1));
    line.end_ms.saturating_sub(lead).max(line.start_ms + 1)
}

fn parse_timestamp(value: &str) -> Option<u64> {
    let (minutes, seconds) = value.split_once(':')?;
    let minutes = minutes.parse::<u64>().ok()?;
    let (seconds, fraction) = seconds
        .split_once('.')
        .map_or((seconds, ""), |(s, f)| (s, f));
    let seconds = seconds.parse::<u64>().ok()?;
    if seconds >= 60 || fraction.len() > 3 || !fraction.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let fraction_ms = match fraction.len() {
        0 => 0,
        1 => fraction.parse::<u64>().ok()? * 100,
        2 => fraction.parse::<u64>().ok()? * 10,
        _ => fraction.parse::<u64>().ok()?,
    };
    Some(minutes * 60_000 + seconds * 1_000 + fraction_ms)
}

fn parse_line_tags(line: &str) -> (Vec<u64>, &str) {
    let mut rest = line;
    let mut timestamps = Vec::new();
    while rest.starts_with('[') {
        let Some(close) = rest.find(']') else { break };
        let Some(timestamp) = parse_timestamp(&rest[1..close]) else {
            break;
        };
        timestamps.push(timestamp);
        rest = &rest[close + 1..];
    }
    (timestamps, rest)
}

fn parse_enhanced(text: &str) -> (String, Vec<LyricWord>) {
    let mut plain = String::new();
    let mut words = Vec::new();
    let mut cursor = 0;
    while cursor < text.len() {
        let Some(relative_open) = text[cursor..].find('<') else {
            plain.push_str(&text[cursor..]);
            break;
        };
        let open = cursor + relative_open;
        plain.push_str(&text[cursor..open]);
        let Some(open_end_relative) = text[open..].find('>') else {
            plain.push_str(&text[open..]);
            break;
        };
        let open_end = open + open_end_relative;
        let tag = &text[open + 1..open_end];
        let Some(start_ms) = parse_timestamp(tag) else {
            plain.push_str(&text[open..=open_end]);
            cursor = open_end + 1;
            continue;
        };
        let close_tag = format!("</{tag}>");
        let content_start = open_end + 1;
        let Some(content_end_relative) = text[content_start..].find(&close_tag) else {
            plain.push_str(&text[open..]);
            break;
        };
        let content_end = content_start + content_end_relative;
        let word_text = &text[content_start..content_end];
        plain.push_str(word_text);
        words.push(LyricWord {
            start_ms,
            end_ms: start_ms,
            text: word_text.to_owned(),
        });
        cursor = content_end + close_tag.len();
    }
    (plain, words)
}

fn is_metadata_line(line: &str) -> bool {
    let Some(end) = line.find(']') else {
        return false;
    };
    line[1..end].starts_with("ti:")
        || line[1..end].starts_with("ar:")
        || line[1..end].starts_with("al:")
}

pub fn parse_lrc(input: &str) -> Result<Vec<LyricLine>> {
    let mut lines = Vec::new();
    for raw_line in input.lines() {
        let raw_line = raw_line.trim_end_matches('\r');
        let (timestamps, text) = parse_line_tags(raw_line);
        if timestamps.is_empty() {
            if raw_line.starts_with('[') && !is_metadata_line(raw_line) {
                eprintln!("warning: skipping invalid LRC timestamp line: {raw_line}");
            }
            continue;
        }
        let (text, words) = parse_enhanced(text);
        for start_ms in timestamps {
            lines.push(LyricLine {
                start_ms,
                end_ms: 0,
                text: text.clone(),
                translation: None,
                agent_id: None,
                is_duet: false,
                is_background: false,
                background_vocal: None,
                words: words.clone(),
            });
        }
    }
    if lines.is_empty() {
        bail!("no lyric lines");
    }
    lines.sort_by_key(|line| line.start_ms);
    for index in 0..lines.len() {
        let fallback_end = lines
            .get(index + 1)
            .map(|next| next.start_ms)
            .unwrap_or(lines[index].start_ms + 5_000);
        let mut next_start = fallback_end;
        for word in lines[index].words.iter_mut().rev() {
            if word.end_ms <= word.start_ms {
                word.end_ms = next_start.max(word.start_ms + 1);
            }
            next_start = word.start_ms;
        }
        let end_ms = lines[index]
            .words
            .last()
            .map(|word| word.end_ms)
            .unwrap_or(fallback_end);
        lines[index].end_ms = end_ms.max(lines[index].start_ms + 1);
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_multiple_timestamps() {
        let lines = parse_lrc("[00:01.00][00:03.00]hello\n").unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].start_ms, 1_000);
        assert_eq!(lines[1].start_ms, 3_000);
        assert_eq!(lines[0].end_ms, 3_000);
    }

    #[test]
    fn untimed_scan_finishes_before_nominal_line_end() {
        let lines = parse_lrc("[00:00.00]first\n[00:02.00]second\n").unwrap();
        assert_eq!(scan_end_ms(&lines[0]), 1_750);
        assert!(scan_end_ms(&lines[0]) < lines[1].start_ms);
    }

    #[test]
    fn parses_enhanced_words_and_text() {
        let lines =
            parse_lrc("[00:04.00]<00:04.00>He</00:04.00><00:04.50>llo</00:04.50>\n").unwrap();
        assert_eq!(lines[0].text, "Hello");
        assert_eq!(lines[0].words.len(), 2);
        assert_eq!(lines[0].words[1].start_ms, 4_500);
        assert_eq!(lines[0].words[0].end_ms, 4_500);
        assert_eq!(lines[0].words[1].end_ms, 9_000);
    }

    #[test]
    fn sorts_lines_and_uses_last_end_fallback() {
        let lines = parse_lrc("[00:05]later\n[00:01]first\n").unwrap();
        assert_eq!(lines[0].text, "first");
        assert_eq!(lines[0].end_ms, 5_000);
        assert_eq!(lines[1].end_ms, 10_000);
    }

    #[test]
    fn rejects_input_without_valid_lines() {
        assert!(parse_lrc("[ti:title]\nnot lyrics\n").is_err());
    }
}
