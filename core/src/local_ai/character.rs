//! What the character says on its own: the prompt it gets and the checks its line must pass.
//! The model sees only the moment (time, trigger, a canned theme, recent lines). Never counseling text.
use crate::language::analyze_language;
use crate::safety::check_safety;
use chrono::{Datelike, Local, TimeZone, Timelike};

#[derive(Clone, Debug)]
pub enum Trigger {
    /// The regular talk timer (frequency setting).
    Timer,
    /// The user came back after being away for this many minutes.
    Returned(i64),
    /// The user asked to hear something now.
    Asked,
}

#[derive(Clone, Debug)]
pub struct Moment {
    pub name: String,
    pub now_ms: i64,
    pub trigger: Trigger,
    /// A canned advice line used as a theme, so topics stay varied and grounded.
    pub theme: Option<String>,
    /// Lines said recently; the model must not repeat them.
    pub recent: Vec<String>,
}

const MAX_CHARS: usize = 90;

fn when(now_ms: i64) -> String {
    let t = Local.timestamp_millis_opt(now_ms).single().unwrap_or_else(Local::now);
    let day = ["월", "화", "수", "목", "금", "토", "일"][t.weekday().num_days_from_monday() as usize];
    let h = t.hour();
    let part = match h {
        5..=8 => "이른 아침",
        9..=11 => "오전",
        12..=13 => "점심 무렵",
        14..=17 => "오후",
        18..=21 => "저녁",
        22..=23 => "밤늦은 시간",
        _ => "새벽",
    };
    // The exact hour made the model open every line with it; the part of day is enough for tone.
    format!("{day}요일 {part}")
}

pub fn prompt(m: &Moment) -> (String, String) {
    let name = m.name.trim();
    let name = if name.is_empty() { "사이" } else { name };
    let system = format!(
        "너는 사용자의 바탕화면에 사는 작은 불꽃 캐릭터 \"{name}\"야. 사용자에게 먼저 건네는 한마디를 만든다.\n\
규칙:\n\
- 친구에게 하듯 반말로만 말한다. 존댓말(~요, ~니다)을 쓰지 않는다.\n\
- 한두 문장, 60자 안팎.\n\
- 이모지, 이모티콘, 따옴표, 목록 기호, 영어를 쓰지 않는다.\n\
- 사용자가 지금 무엇을 하는지, 무엇을 했는지, 기분이 어떤지 짐작하거나 지어내지 않는다.\n\
- 쉽고 일상적인 말만 쓴다. 비유나 시적인 표현, 풍경 묘사를 하지 않는다.\n\
- 사용자가 방금 돌아왔다면 먼저 반긴다. 주제가 있으면 그 주제 하나만 말한다.\n\
- 시간대나 요일은 꼭 어울릴 때만 짧게 언급하고, 그것으로 문장을 시작하지 않는다.\n\
- 최근에 한 말과 다른 표현으로 말한다.\n\
- 질문은 많아야 하나. 훈계하거나 진단하지 않는다.\n\
- 결과 문장만 출력한다.\n\
좋은 예:\n\
- 다시 왔네. 천천히 다시 시작해 보자.\n\
- 눈이 좀 뻑뻑하지 않아? 창밖 한번 보고 오자.\n\
- 제일 작은 일 하나만 먼저 끝내 볼까?\n\
나쁜 예(짐작): 야구 보고 있구나? / 밤새 놀았나 봐? / 기분 좋아 보이네"
    );
    let trigger = match &m.trigger {
        Trigger::Timer => "잠깐 말을 걸 때가 됐어".to_owned(),
        Trigger::Returned(min) if *min >= 120 => "사용자가 한참 자리를 비웠다가 방금 돌아왔어. 반겨 줘".to_owned(),
        Trigger::Returned(_) => "사용자가 잠깐 자리를 비웠다가 방금 돌아왔어. 반겨 줘".to_owned(),
        Trigger::Asked => "사용자가 한마디 해 달라고 했어".to_owned(),
    };
    let mut user = format!("지금: {}\n계기: {trigger}\n", when(m.now_ms));
    if let Some(theme) = m.theme.as_deref().filter(|t| !t.trim().is_empty()) {
        user.push_str(&format!("참고할 주제(그대로 베끼지 말고 지금 상황에 맞게): {theme}\n"));
    }
    if !m.recent.is_empty() {
        user.push_str("최근에 한 말(반복하지 말 것):\n");
        for r in m.recent.iter().rev().take(5) {
            user.push_str(&format!("- {r}\n"));
        }
    }
    (system, user)
}

fn is_hangul(c: char) -> bool {
    matches!(c, '\u{AC00}'..='\u{D7A3}' | '\u{1100}'..='\u{11FF}' | '\u{3130}'..='\u{318F}')
}
/// Emoji and pictographs: the bubble is plain text in the character's voice.
fn is_pictograph(c: char) -> bool {
    matches!(c as u32, 0x1F000..=0x1FAFF | 0x2600..=0x27BF | 0x2B00..=0x2BFF | 0xFE0F)
}

/// Characters a plain spoken line may contain: Hangul, digits, spaces and light punctuation.
fn is_plain(c: char) -> bool {
    is_hangul(c) || c.is_ascii_digit() || c == ' ' || ".,!?~…".contains(c)
}

/// Polite endings break the character's voice; guesses about what the user is doing invent facts.
fn off_voice(line: &str) -> bool {
    let mut words = line.split(|c: char| c == ' ' || ".,!?~…".contains(c)).filter(|w| !w.is_empty());
    let polite = words.any(|w| w.ends_with('요') || w.ends_with("니다"));
    let squashed: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    let guess = ["고있구나", "고있네", "나봐", "보이네", "보이는구나", "었구나", "았구나", "했구나"]
        .iter()
        .any(|g| squashed.replace("왔구나", "").contains(g));
    polite || guess
}

/// Near-repeats read as a broken record: compare two-letter chunks, ignoring spaces and punctuation.
fn similar(a: &str, b: &str) -> bool {
    use std::collections::HashSet;
    let grams = |s: &str| {
        let c: Vec<char> = s.chars().filter(|c| is_hangul(*c) || c.is_ascii_digit()).collect();
        c.windows(2).map(|w| (w[0], w[1])).collect::<HashSet<_>>()
    };
    let (x, y) = (grams(a), grams(b));
    if x.is_empty() || y.is_empty() {
        return x == y;
    }
    x.intersection(&y).count() * 2 >= x.len().min(y.len())
}

/// Returns the line to show, or None when the model's output must not be shown.
pub fn clean(raw: &str, m: &Moment) -> Option<String> {
    // Some models still wrap the line in quotes, prefix the speaker, or add a second paragraph.
    let first = raw.trim().lines().map(str::trim).find(|l| !l.is_empty())?;
    let unquote = |s: &str| {
        s.trim_matches(|c: char| matches!(c, '"' | '\'' | '“' | '”' | '‘' | '’' | '「' | '」' | '『' | '』'))
            .trim()
            .to_owned()
    };
    let mut line = unquote(first);
    let name = m.name.trim();
    if !name.is_empty() {
        if let Some(rest) = line.strip_prefix(name).and_then(|r| r.trim_start().strip_prefix(':')) {
            line = unquote(rest);
        }
    }
    let numbered = line.chars().next().is_some_and(|c| c.is_ascii_digit()) && line.chars().nth(1) == Some('.');
    if numbered || line.starts_with(['-', '*', '#', '•', '>']) || line.contains("**") {
        return None;
    }
    // A trailing emoji or quoted word ("고마워") is the most common slip; drop those marks rather than the line.
    let line: String = line
        .chars()
        .filter(|c| !is_pictograph(*c) && !matches!(c, '\u{200D}' | '"' | '“' | '”'))
        .collect();
    let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
    let count = line.chars().count();
    if !(4..=MAX_CHARS).contains(&count) {
        return None;
    }
    // Anything else (Latin, kaomoji, links, @, brackets) means the line is not plain speech.
    if !line.chars().all(is_plain) || line.chars().filter(char::is_ascii_digit).count() > 4 {
        return None;
    }
    if off_voice(&line) || check_safety(&line).is_some() || analyze_language(&line).profanity {
        return None;
    }
    if m.recent.iter().any(|r| similar(r, &line)) {
        return None;
    }
    Some(line)
}
