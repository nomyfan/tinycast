//! The launcher's fuzzy search: a query matches a text holding its
//! characters in order, ignoring case and spaces ("vsc" matches "Visual
//! Studio Code", "qq music" matches "QQMusic"). Chinese characters also match
//! by their pinyin, toneless, any of their readings ("yinyue", "yy" and
//! "yiny" match "音乐"). Matches rank as fzf's do: a prefix, word starts and
//! runs of adjacent characters first.

use nsh_base::CommandItem;
use pinyin::ToPinyinMulti;

/// Each character matched.
const MATCH: i32 = 16;
/// A match on the text's first character.
const START: i32 = 12;
/// A match at a word's start: after a space or punctuation, or where the
/// script changes ("QQ音乐").
const BOUNDARY: i32 = 8;
/// A match on a camelCase hump ("QQMusic").
const CAMEL: i32 = 6;
/// A match right after the previous one.
const CONSECUTIVE: i32 = 6;
/// Each character skipped between two matches.
const GAP: i32 = 1;

/// At most this many pinyin spellings of a text: past it, a character with
/// several readings is spelled by its most common one only.
const MAX_SPELLINGS: usize = 16;

/// How well `text` matches `query`, `None` when it does not: the best
/// alignment of the query's characters over the text's, or over a pinyin
/// spelling of it.
pub(crate) fn score(query: &str, text: &str) -> Option<i32> {
    let query: Vec<char> = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(fold)
        .collect();
    if query.is_empty() {
        return Some(0);
    }
    let chars: Vec<char> = text.chars().collect();
    let bonus: Vec<i32> = (0..chars.len()).map(|j| bonus(&chars, j)).collect();
    let direct = align(&query, &chars, &bonus);
    pinyin_spellings(&chars, &bonus)
        .iter()
        .filter_map(|(chars, bonus)| align(&query, chars, bonus))
        .chain(direct)
        .max()
}

/// `chars` with each Chinese character spelled in pinyin, with the bonus of
/// a match on each letter: a syllable's first letter starts a word. One
/// spelling per combination of readings, none when `chars` has no Chinese.
fn pinyin_spellings(chars: &[char], bonus: &[i32]) -> Vec<(Vec<char>, Vec<i32>)> {
    if !chars.iter().any(|c| c.to_pinyin_multi().is_some()) {
        return Vec::new();
    }
    let mut spellings = vec![(Vec::new(), Vec::new())];
    for (&c, &b) in chars.iter().zip(bonus) {
        let Some(mut syllables) = syllables(c) else {
            for (chars, bonus) in &mut spellings {
                chars.push(fold(c));
                bonus.push(b);
            }
            continue;
        };
        if spellings.len() * syllables.len() > MAX_SPELLINGS {
            syllables.truncate(1);
        }
        spellings = spellings
            .into_iter()
            .flat_map(|spelling| {
                syllables.iter().map(move |syllable| {
                    let (mut chars, mut bonus) = spelling.clone();
                    chars.extend(syllable);
                    bonus.extend(
                        (0..syllable.len()).map(|i| if i == 0 { b.max(BOUNDARY) } else { 0 }),
                    );
                    (chars, bonus)
                })
            })
            .collect();
    }
    spellings
}

/// The toneless pinyin readings of a Chinese character, the most common
/// first, `None` for another character. "ü" is "v", as typed with a pinyin
/// input method.
fn syllables(c: char) -> Option<Vec<Vec<char>>> {
    let mut syllables: Vec<Vec<char>> = Vec::new();
    for reading in c.to_pinyin_multi()? {
        let syllable = reading
            .plain()
            .chars()
            .map(|letter| if letter == 'ü' { 'v' } else { letter })
            .collect();
        // The readings differ by tone only, often.
        if !syllables.contains(&syllable) {
            syllables.push(syllable);
        }
    }
    Some(syllables)
}

/// The best alignment of `query`, folded, over `chars`, given the bonus of
/// a match on each of them.
fn align(query: &[char], chars: &[char], bonus: &[i32]) -> Option<i32> {
    if query.len() > chars.len() {
        return None;
    }
    let folded: Vec<char> = chars.iter().copied().map(fold).collect();

    // `best[j]`: the best score of the query's characters so far with the
    // last of them matched at `j`.
    let mut best: Vec<Option<i32>> = folded
        .iter()
        .zip(bonus)
        .map(|(&c, &b)| (c == query[0]).then_some(MATCH + b))
        .collect();
    for &q in &query[1..] {
        let mut next = vec![None; chars.len()];
        // The best previous match before `j - 1`, less the gap to `j`.
        let mut gapped: Option<i32> = None;
        for j in 1..chars.len() {
            if folded[j] == q {
                let adjacent = best[j - 1].map(|s| s + CONSECUTIVE);
                next[j] = adjacent.max(gapped).map(|s| s + MATCH + bonus[j]);
            }
            gapped = gapped.max(best[j - 1]).map(|s| s - GAP);
        }
        best = next;
    }
    best.into_iter().flatten().max()
}

fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

fn bonus(chars: &[char], j: usize) -> i32 {
    let c = chars[j];
    let Some(&prev) = j.checked_sub(1).and_then(|i| chars.get(i)) else {
        return START;
    };
    if !prev.is_alphanumeric() && c.is_alphanumeric() || prev.is_ascii() != c.is_ascii() {
        BOUNDARY
    } else if prev.is_lowercase() && c.is_uppercase() {
        CAMEL
    } else {
        0
    }
}

/// The items `query` matches by their label or a keyword, best first (a
/// tie goes to the shorter text, then to the earlier item), out of their
/// groups: one list ranked across them. `boost` adds to the score of an
/// item by its id (a recent one's).
pub(crate) fn rank(
    items: Vec<CommandItem>,
    query: &str,
    boost: impl Fn(&str) -> i32,
) -> Vec<CommandItem> {
    let mut ranked: Vec<_> = items
        .into_iter()
        .filter_map(|item| {
            let (score, len) = std::iter::once(&item.label)
                .chain(&item.keywords)
                .filter_map(|text| Some((score(query, text)?, text.chars().count())))
                .max_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)))?;
            Some((score + boost(&item.id), len, item))
        })
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    ranked
        .into_iter()
        .map(|(_, _, mut item)| {
            item.group = None;
            item
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spaces_are_ignored() {
        assert!(score("qq music", "QQMusic").is_some());
        assert!(score("qqmusic", "QQ Music").is_some());
    }

    #[test]
    fn characters_match_in_order() {
        assert!(score("vsc", "Visual Studio Code").is_some());
        assert!(score("音乐", "QQ音乐").is_some());
        assert!(score("csv", "Visual Studio Code").is_none());
        assert!(score("safarix", "Safari").is_none());
    }

    #[test]
    fn chinese_matches_by_pinyin() {
        assert!(score("weixin", "微信").is_some());
        assert!(score("wx", "微信").is_some());
        assert!(score("weix", "微信").is_some());
        assert!(score("qqyy", "QQ音乐").is_some());
        // Any reading of a character with several: 乐 is "le" most often.
        assert!(score("yinyue", "音乐").is_some());
        assert!(score("yinle", "音乐").is_some());
        assert!(score("lvse", "绿色").is_some());
        assert!(score("yinyue", "Music").is_none());
    }

    #[test]
    fn pinyin_syllables_start_words() {
        assert!(score("xt", "系统设置") > score("xt", "下载"));
        assert!(score("xtsz", "系统设置") > score("xtsz", "系统信息设置"));
    }

    #[test]
    fn many_readings_are_capped() {
        // 行 and 和 have three and four readings each.
        let chars: Vec<char> = "行和行和行和".chars().collect();
        let spellings = pinyin_spellings(&chars, &[0; 6]);
        assert!(spellings.len() <= MAX_SPELLINGS);
        assert!(score("xinghexinghexinghe", "行和行和行和").is_some());
    }

    #[test]
    fn prefix_beats_middle() {
        assert!(score("note", "Notes") > score("note", "Sticky Notes"));
    }

    #[test]
    fn word_starts_beat_scattered() {
        assert!(score("ss", "System Settings") > score("ss", "Mission Control"));
        assert!(score("code", "Visual Studio Code") > score("code", "Xcode"));
    }

    #[test]
    fn rank_orders_and_ungroups() {
        let items = vec![
            CommandItem::new("a", "Sticky Notes").group("Applications"),
            CommandItem::new("b", "Safari").group("Applications"),
            CommandItem::new("c", "Notes").group("Applications"),
            CommandItem::new("d", "QQ音乐")
                .keywords(["QQMusic"])
                .group("Applications"),
        ];
        let ids = |items: Vec<CommandItem>| items.into_iter().map(|i| i.id).collect::<Vec<_>>();
        let none = |_: &str| 0;
        assert_eq!(ids(rank(items.clone(), "note", none)), ["c", "a"]);
        assert_eq!(ids(rank(items.clone(), "qq music", none)), ["d"]);
        assert!(
            rank(items.clone(), "note", none)
                .iter()
                .all(|i| i.group.is_none())
        );
        // A recent item passes a match about as good.
        let recent = |id: &str| if id == "a" { 10 } else { 0 };
        assert_eq!(ids(rank(items, "note", recent)), ["a", "c"]);
    }
}
