//! The `.env` file's variables, which scripts run with: `KEY=value` lines,
//! as shells and dotenv libraries read them.

/// The variables of `text`, in order. A value's `$NAME` and `${NAME}` take
/// the value of an earlier line's `NAME`, else `base(NAME)`, else nothing.
/// Lines that are not variables are left out, each with a problem saying
/// which line it is and why.
///
/// A line is `KEY=value`, maybe after `export `; `#` starts a comment line.
/// Unquoted and double-quoted values expand variables (`\$` is a `$`);
/// single-quoted ones are kept as they are. An unquoted value is trimmed and
/// ends at ` #`. Values do not span lines.
pub(crate) fn parse(
    text: &str,
    base: impl Fn(&str) -> Option<String>,
) -> (Vec<(String, String)>, Vec<String>) {
    let mut vars: Vec<(String, String)> = Vec::new();
    let mut problems = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let lookup = |name: &str| {
            vars.iter().rev().find(|(key, _)| key == name).map(|(_, value)| value.clone()).or_else(|| base(name))
        };
        match parse_line(line, lookup) {
            Ok(var) => vars.push(var),
            Err(problem) => problems.push(format!(".env line {}: {problem}", index + 1)),
        }
    }
    (vars, problems)
}

fn parse_line(line: &str, lookup: impl Fn(&str) -> Option<String>) -> Result<(String, String), String> {
    let line = line.strip_prefix("export ").map_or(line, str::trim_start);
    let Some((key, raw)) = line.split_once('=') else {
        return Err("not KEY=value".into());
    };
    let key = key.trim();
    if !is_name(key) {
        return Err(format!("\"{key}\" is not a variable name"));
    }
    let raw = raw.trim();
    let value = if let Some(rest) = raw.strip_prefix('\'') {
        let Some((inner, after)) = rest.split_once('\'') else {
            return Err("the single quote is not closed".into());
        };
        trailing(after)?;
        inner.to_owned()
    } else if let Some(rest) = raw.strip_prefix('"') {
        let Some(end) = closing_quote(rest) else {
            return Err("the double quote is not closed".into());
        };
        trailing(&rest[end + 1..])?;
        expand(&rest[..end], &lookup, true)?
    } else {
        let value = raw.find(" #").map_or(raw, |at| &raw[..at]).trim_end();
        expand(value, &lookup, false)?
    };
    Ok((key.to_owned(), value))
}

fn is_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// What may follow a quoted value: nothing, or a comment.
fn trailing(after: &str) -> Result<(), String> {
    let after = after.trim_start();
    if after.is_empty() || after.starts_with('#') {
        Ok(())
    } else {
        Err(format!("unexpected \"{after}\" after the quoted value"))
    }
}

/// The index of the `"` that ends a double-quoted value starting at `text`.
fn closing_quote(text: &str) -> Option<usize> {
    let mut escaped = false;
    for (at, c) in text.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '"' => return Some(at),
            _ => {}
        }
    }
    None
}

/// `value` with its variables expanded and its escapes (`\$`, and in double
/// quotes `\"`, `\\`, `\n`) taken out.
fn expand(value: &str, lookup: &impl Fn(&str) -> Option<String>, quoted: bool) -> Result<String, String> {
    let mut out = String::new();
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('$') => out.push('$'),
                Some('"') if quoted => out.push('"'),
                Some('\\') if quoted => out.push('\\'),
                Some('n') if quoted => out.push('\n'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            },
            '$' if chars.peek() == Some(&'{') => {
                chars.next();
                let mut name = String::new();
                let mut closed = false;
                for c in chars.by_ref() {
                    if c == '}' {
                        closed = true;
                        break;
                    }
                    name.push(c);
                }
                if !closed || !is_name(&name) {
                    return Err(format!("\"${{{name}\" is not a variable"));
                }
                out.push_str(&lookup(&name).unwrap_or_default());
            }
            '$' if chars.peek().is_some_and(|&c| c.is_ascii_alphabetic() || c == '_') => {
                let mut name = String::new();
                while let Some(&c) = chars.peek().filter(|c| c.is_ascii_alphanumeric() || **c == '_') {
                    name.push(c);
                    chars.next();
                }
                out.push_str(&lookup(&name).unwrap_or_default());
            }
            c => out.push(c),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(name: &str) -> Option<String> {
        match name {
            "HOME" => Some("/Users/me".into()),
            "PATH" => Some("/usr/bin:/bin".into()),
            _ => None,
        }
    }

    #[test]
    fn prepends_to_the_path() {
        let (vars, problems) = parse("PATH=/opt/homebrew/bin:$HOME/.nix-profile/bin:$PATH\n", base);
        assert_eq!(vars, [("PATH".into(), "/opt/homebrew/bin:/Users/me/.nix-profile/bin:/usr/bin:/bin".into())]);
        assert!(problems.is_empty());
    }

    #[test]
    fn later_lines_see_earlier_ones() {
        let (vars, _) = parse("PATH=/a:$PATH\nPATH=/b:${PATH}\nTOOLS=$PATH/x\n", base);
        assert_eq!(vars[1].1, "/b:/a:/usr/bin:/bin");
        assert_eq!(vars[2].1, "/b:/a:/usr/bin:/bin/x");
    }

    #[test]
    fn quotes() {
        let text = "A='$HOME is kept'\nB=\"$HOME and \\\"quoted\\\"\"\nC=\"price \\$5\"\nD=plain  # a comment\nE=\"x\" # comment\n";
        let (vars, problems) = parse(text, base);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(vars[0].1, "$HOME is kept");
        assert_eq!(vars[1].1, "/Users/me and \"quoted\"");
        assert_eq!(vars[2].1, "price $5");
        assert_eq!(vars[3].1, "plain");
        assert_eq!(vars[4].1, "x");
    }

    #[test]
    fn export_comments_and_blank_lines() {
        let (vars, problems) = parse("# tools\n\nexport TOKEN=abc\n", base);
        assert_eq!(vars, [("TOKEN".into(), "abc".into())]);
        assert!(problems.is_empty());
    }

    #[test]
    fn an_unknown_variable_is_empty() {
        let (vars, _) = parse("A=x${NOPE}y$NOPE\n", base);
        assert_eq!(vars[0].1, "xy");
    }

    #[test]
    fn a_lone_dollar_is_kept() {
        let (vars, _) = parse("A=5$ and $1\n", base);
        assert_eq!(vars[0].1, "5$ and $1");
    }

    #[test]
    fn bad_lines_are_problems_and_skipped() {
        let text = "OK=1\nno equals\n1BAD=x\nQ=\"open\nS='open\nT=\"a\" b\nU=${bad-name}\nV=${HOME\n";
        let (vars, problems) = parse(text, base);
        assert_eq!(vars, [("OK".into(), "1".into())]);
        assert_eq!(problems.len(), 7, "{problems:?}");
        assert!(problems[0].starts_with(".env line 2:"));
    }
}
