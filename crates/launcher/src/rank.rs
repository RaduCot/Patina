//! Match quality, best to worst: exact, prefix, a word's prefix, initials
//! ("vsc" -> Visual Studio Code), substring, scattered subsequence.
//! Inputs are already lowercase.

pub fn score(hay: &str, q: &str) -> Option<i32> {
    if q.is_empty() {
        return Some(0);
    }
    if hay == q {
        return Some(2000);
    }
    if hay.starts_with(q) {
        return Some(1600 - hay.len().min(200) as i32);
    }
    let words = word_starts(hay);
    if let Some(&at) = words.iter().find(|&&i| hay[i..].starts_with(q)) {
        return Some(1300 - at.min(200) as i32);
    }
    if initials_match(hay, &words, q) {
        return Some(1150 - hay.len().min(200) as i32);
    }
    if let Some(at) = hay.find(q) {
        return Some(800 - at.min(200) as i32);
    }
    subsequence(hay, &words, q).map(|gaps| 300 - gaps)
}

/// Byte offsets where a word begins: start, after a separator, or a lower->Upper
/// step in the original (lost after lowercasing, so separators only here).
fn word_starts(hay: &str) -> Vec<usize> {
    let mut out = vec![0];
    let mut prev_sep = false;
    for (i, c) in hay.char_indices() {
        if prev_sep && c.is_alphanumeric() {
            out.push(i);
        }
        prev_sep = !c.is_alphanumeric();
    }
    out
}

fn initials_match(hay: &str, words: &[usize], q: &str) -> bool {
    let initials: String = words.iter().filter_map(|&i| hay[i..].chars().next()).collect();
    q.chars().count() >= 2 && initials.starts_with(q)
}

/// Typo-tolerant last resort ("vscde"), kept tight so it doesn't drag in
/// names that merely contain the letters somewhere ("code" -> "miCrOsoft
/// eDgE"): the first letter must start a word, and the letters may skip at
/// most twice the query length in total.
fn subsequence(hay: &str, words: &[usize], q: &str) -> Option<i32> {
    let first = q.chars().next()?;
    let budget = 2 * q.chars().count() as i32;
    words
        .iter()
        .filter(|&&w| hay[w..].starts_with(first))
        .filter_map(|&w| {
            let mut chars = hay[w..].chars();
            let mut gaps = 0;
            for qc in q.chars() {
                loop {
                    match chars.next() {
                        Some(c) if c == qc => break,
                        Some(_) => gaps += 1,
                        None => return None,
                    }
                }
            }
            (gaps <= budget).then_some(gaps)
        })
        .min()
}

#[cfg(test)]
mod tests {
    use super::score;

    #[test]
    fn ordering() {
        let s = |h: &str, q: &str| score(h, q).unwrap_or(i32::MIN);
        assert!(s("zed", "zed") > s("zed preview", "zed"));
        assert!(s("visual studio code", "code") > s("unicode table", "code"));
        assert!(s("visual studio code", "vsc") > s("vlc media player", "vsc"));
        assert!(s("routling", "rout") > s("traceroute", "rout"));
        assert!(score("notepad", "xyz").is_none());
    }

    #[test]
    fn scattered_letters_are_not_a_match() {
        // Real Start-menu names that "code" pulled in before the subsequence was tightened.
        for name in ["microsoft edge", "recovery drive", "click to do (preview)", "application stack builder"] {
            assert!(score(name, "code").is_none(), "{name} should not match 'code'");
        }
        // ...while a genuine typo still lands.
        assert!(score("visual studio code", "visal studio").is_some());
        assert!(score("notepad++", "ntpd").is_some());
    }
}
