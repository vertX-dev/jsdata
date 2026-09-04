//! Minecraft Bedrock `.lang` parsing for `--pull` `langs` bundles.
//! `languages.json` is a JSON array of locale codes; each `<locale>.lang` is
//! `key=value` lines with `#` full-line comments.

/// Locale codes from a `languages.json` (JSON array of strings). Tolerant of
/// `//` and `/* */` comments. ponytail: collects every JSON string literal —
/// correct for an array-of-strings file, which is all languages.json ever is.
pub fn parse_languages_json(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '/' if chars.peek() == Some(&'/') => {
                for n in chars.by_ref() {
                    if n == '\n' {
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut prev = ' ';
                for n in chars.by_ref() {
                    if prev == '*' && n == '/' {
                        break;
                    }
                    prev = n;
                }
            }
            '"' => {
                let mut s = String::new();
                while let Some(nc) = chars.next() {
                    match nc {
                        '\\' => {
                            if let Some(esc) = chars.next() {
                                s.push(match esc {
                                    'n' => '\n',
                                    't' => '\t',
                                    'r' => '\r',
                                    other => other, // covers " \ /
                                });
                            }
                        }
                        '"' => break,
                        _ => s.push(nc),
                    }
                }
                out.push(s);
            }
            _ => {}
        }
    }
    out
}

/// `key=value` pairs from a `.lang` file, in file order. Blank lines and
/// full-line `#` / `##` comments skipped; split on the first `=`, value kept
/// verbatim. ponytail: no inline (`\t#…`) comment stripping — full-line only.
pub fn parse_lang(text: &str) -> Vec<(String, String)> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut out = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let key = k.trim();
            if !key.is_empty() {
                out.push((key.to_string(), v.to_string()));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_array() {
        assert_eq!(
            parse_languages_json("[\n  \"en_US\",\n  \"ar_SA\",\n  \"zh_CN\"\n]"),
            vec!["en_US", "ar_SA", "zh_CN"]
        );
    }

    #[test]
    fn languages_tolerates_comments() {
        assert_eq!(
            parse_languages_json("[ // locales\n  \"en_US\", /* primary */ \"fr_FR\"\n]"),
            vec!["en_US", "fr_FR"]
        );
    }

    #[test]
    fn lang_pairs_skip_comments_and_blanks() {
        let src = "pack.name=Rarity & Stats\n\n## rarity tier names\nrrs.rarity.common=§7§lCommon\n# note\nrrs.rarity.rare=§9§lRare\n";
        assert_eq!(
            parse_lang(src),
            vec![
                ("pack.name".to_string(), "Rarity & Stats".to_string()),
                ("rrs.rarity.common".to_string(), "§7§lCommon".to_string()),
                ("rrs.rarity.rare".to_string(), "§9§lRare".to_string()),
            ]
        );
    }

    #[test]
    fn lang_value_may_contain_equals() {
        assert_eq!(
            parse_lang("k=a=b=c"),
            vec![("k".to_string(), "a=b=c".to_string())]
        );
    }
}
