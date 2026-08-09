use anyhow::{Result, bail};

use crate::lrc::{LyricLine, LyricWord};

fn parse_pair(value: &str) -> Option<(u64, u64)> {
    let mut fields = value.split(',');
    let start_ms = fields.next()?.trim().parse().ok()?;
    let duration_ms = fields.next()?.trim().parse().ok()?;
    Some((start_ms, duration_ms))
}

fn line_header(line: &str) -> Option<(u64, u64, &str)> {
    let line = line.trim_start();
    let close = line.strip_prefix('[')?.find(']')? + 1;
    let (start_ms, duration_ms) = parse_pair(&line[1..close])?;
    Some((start_ms, duration_ms, &line[close + 1..]))
}

fn word_tag_at(text: &str, open: usize) -> Option<(usize, u64, u64)> {
    let close_relative = text[open + 1..].find(')')?;
    let close = open + 1 + close_relative;
    let (start_ms, duration_ms) = parse_pair(&text[open + 1..close])?;
    Some((close, start_ms, duration_ms))
}

fn next_word_tag(text: &str, from: usize) -> Option<(usize, usize, u64, u64)> {
    text[from..].match_indices('(').find_map(|(relative, _)| {
        let open = from + relative;
        word_tag_at(text, open)
            .map(|(close, start_ms, duration_ms)| (open, close, start_ms, duration_ms))
    })
}

fn parse_words(text: &str) -> (String, Vec<LyricWord>) {
    let mut plain = String::new();
    let mut words = Vec::new();
    let mut cursor = 0;
    while let Some((open, close, start_ms, duration_ms)) = next_word_tag(text, cursor) {
        plain.push_str(&text[cursor..open]);
        let content_start = close + 1;
        let next_open = next_word_tag(text, content_start)
            .map(|(next_open, _, _, _)| next_open)
            .unwrap_or(text.len());
        let word_text = &text[content_start..next_open];
        plain.push_str(word_text);
        if !word_text.is_empty() {
            words.push(LyricWord {
                start_ms,
                end_ms: start_ms
                    .saturating_add(duration_ms)
                    .max(start_ms.saturating_add(1)),
                text: word_text.to_owned(),
            });
        }
        cursor = next_open;
    }
    plain.push_str(&text[cursor..]);
    (plain, words)
}

pub fn looks_like_yrc(input: &str) -> bool {
    input
        .lines()
        .map(str::trim)
        .any(|line| line_header(line).is_some_and(|(_, _, content)| !content.is_empty()))
}

pub fn parse_yrc(input: &str) -> Result<Vec<LyricLine>> {
    let mut lines = Vec::new();
    for raw_line in input.lines() {
        let raw_line = raw_line.trim_end_matches('\r');
        let Some((start_ms, duration_ms, content)) = line_header(raw_line) else {
            continue;
        };
        let (text, words) = parse_words(content);
        if text.is_empty() {
            continue;
        }
        let word_end = words
            .iter()
            .map(|word| word.end_ms)
            .max()
            .unwrap_or(start_ms);
        let declared_end = start_ms.saturating_add(duration_ms);
        lines.push(LyricLine {
            start_ms,
            end_ms: declared_end.max(word_end).max(start_ms.saturating_add(1)),
            text,
            translation: None,
            agent_id: None,
            is_duet: false,
            is_background: false,
            background_vocal: None,
            words,
        });
    }
    if lines.is_empty() {
        bail!("no YRC lyric lines");
    }
    lines.sort_by_key(|line| line.start_ms);
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_absolute_word_start_and_duration() {
        let lines =
            parse_yrc("[44810,3780](44810,240,0)Think (45050,450,0)about(45500,510,0), ").unwrap();
        assert_eq!(lines[0].start_ms, 44_810);
        assert_eq!(lines[0].end_ms, 48_590);
        assert_eq!(lines[0].text, "Think about, ");
        assert_eq!(lines[0].words.len(), 3);
        assert_eq!(lines[0].words[1].start_ms, 45_050);
        assert_eq!(lines[0].words[1].end_ms, 45_500);
    }

    #[test]
    fn keeps_zero_duration_punctuation_and_parentheses() {
        let lines = parse_yrc(
            "[344030,5970](344030,720,0)You(344750,0,0)（(344750,420,0)and(345170,0,0)）",
        )
        .unwrap();
        assert_eq!(lines[0].text, "You（and）");
        assert_eq!(lines[0].words[1].text, "（");
        assert_eq!(lines[0].words[1].end_ms, 344_751);
    }

    #[test]
    fn detects_yrc_without_confusing_lrc() {
        assert!(looks_like_yrc("[1000,500](1000,500,0)hello"));
        assert!(!looks_like_yrc("[00:01.00]hello"));
    }

    #[test]
    fn detects_yrc_after_metadata() {
        assert!(looks_like_yrc(
            "[by:artist]\n[id:123]\n[1000,500](1000,500,0)hello"
        ));
    }

    #[test]
    fn detects_yrc_without_word_tags() {
        assert!(looks_like_yrc("[1000,500]hello"));
    }

    #[test]
    fn rejects_input_without_yrc_lines() {
        assert!(parse_yrc("[ti:title]\nnot lyrics").is_err());
    }

    #[test]
    fn parses_repository_yrc_fixture() {
        let lines = parse_yrc(include_str!("../assets/Heal the World.yrc")).unwrap();
        assert_eq!(lines.len(), 94);
        assert_eq!(lines[0].start_ms, 21_950);
        assert_eq!(lines[0].words[4].text, "Monologue");
        assert_eq!(lines.last().unwrap().end_ms, 385_040);
        assert!(lines.iter().all(|line| !line.words.is_empty()));
    }
}
