//! Fold the project's local, unconditional imports without rewriting CSS rules.
use anyhow::{Context, Result, bail};
use std::path::Path;

mod syntax;
use syntax::{Cursor, Url};

const ENTRY: &str = "zxs.css";
const MAX_DEPTH: usize = 64;
const MAX_OUTPUT: usize = 16 * 1024 * 1024;

pub(super) fn bundle(web: &Path) -> Result<String> {
    let mut output = String::new();
    expand(web, ENTRY, &mut Vec::new(), &mut output)?;
    Ok(output)
}

fn append(output: &mut String, text: &str) -> Result<()> {
    if text.len() > MAX_OUTPUT - output.len() {
        bail!("bundled CSS exceeds {MAX_OUTPUT} bytes");
    }
    output.push_str(text);
    Ok(())
}

fn expand(
    web: &Path,
    published: &str,
    active: &mut Vec<String>,
    output: &mut String,
) -> Result<()> {
    if active.iter().any(|path| path == published) {
        bail!("CSS import cycle: {} -> {published}", active.join(" -> "));
    }
    if active.len() == MAX_DEPTH {
        bail!("CSS imports exceed {MAX_DEPTH} levels at {published}");
    }
    let source = if published == ENTRY {
        web.join("styles.css")
    } else {
        let mut source = web.join("public");
        for segment in published.split('/') {
            source.push(decode_segment(segment)?);
        }
        source
    };
    let text = String::from_utf8(
        super::read_input(&source)
            .with_context(|| format!("cannot read CSS input {}", source.display()))?,
    )
    .with_context(|| format!("CSS input is not UTF-8: {}", source.display()))?;
    if text.starts_with('\u{feff}') {
        bail!(
            "CSS byte-order marks are not supported: {}",
            source.display()
        );
    }
    active.push(published.to_owned());
    fold(web, published, &text, active, output)
        .with_context(|| format!("cannot bundle CSS {}", source.display()))?;
    active.pop();
    Ok(())
}

fn fold(
    web: &Path,
    published: &str,
    text: &str,
    active: &mut Vec<String>,
    output: &mut String,
) -> Result<()> {
    let mut cursor = Cursor::new(text);
    let mut copied = 0;
    let mut blocks = Vec::new();
    let mut custom_property = None;
    let mut imports_allowed = true;
    while !cursor.done() {
        if cursor.trivia()?.is_some() {
            continue;
        }
        let start = cursor.position();
        if cursor.peek() == Some(b'@') && blocks.is_empty() {
            cursor.advance();
            let name = cursor.name()?;
            if name.eq_ignore_ascii_case("import") {
                if !imports_allowed {
                    bail!("@import must precede CSS rules at byte {start}");
                }
                let import = parse_import(&mut cursor)?;
                let target = LocalUrl::resolve(published, &import.url)?;
                append(output, &text[copied..start])?;
                append(output, &import.comments)?;
                append(output, "\n")?;
                expand(web, &target.path, active, output)?;
                append(output, "\n")?;
                copied = cursor.position();
                continue;
            }
            if name.eq_ignore_ascii_case("charset") || name.eq_ignore_ascii_case("namespace") {
                bail!("@{name} is not supported by the UTF-8 CSS bundle");
            }
        } else if matches!(cursor.peek(), Some(b'\'' | b'"')) {
            cursor.string()?;
        } else if cursor.starts_name() {
            let name = cursor.name()?;
            if name.starts_with("--")
                && blocks.last() == Some(&b'}')
                && cursor.followed_by_colon()?
                && custom_property.is_none()
            {
                custom_property = Some(blocks.len());
            }
            if cursor.peek() == Some(b'(')
                && ["image", "image-set", "src"]
                    .iter()
                    .any(|function| name.eq_ignore_ascii_case(function))
            {
                bail!("CSS {name}() URL semantics are not supported by the bundle");
            }
            if name.eq_ignore_ascii_case("url") && cursor.peek() == Some(b'(') {
                let url = cursor.url()?;
                if is_relative(&url.value) {
                    if custom_property.is_some() {
                        // Variables resolve relative URLs in the consuming
                        // declaration's stylesheet, not at their definition.
                        bail!("relative URLs in CSS custom properties are not supported");
                    }
                    let target = LocalUrl::resolve(published, &url.value)?;
                    let rebased = format!("./{}{}", target.path, target.suffix);
                    if rebased != url.value {
                        replace_url(output, text, &mut copied, &url, &rebased)?;
                    }
                }
            }
        } else {
            let delimiter = cursor.peek().expect("invariant: cursor is not at EOF");
            if custom_property == Some(blocks.len()) && matches!(delimiter, b';' | b'}') {
                custom_property = None;
            }
            match delimiter {
                b'{' => blocks.push(b'}'),
                b'(' => blocks.push(b')'),
                b'[' => blocks.push(b']'),
                close @ (b'}' | b')' | b']') => {
                    if blocks.pop() != Some(close) {
                        bail!("unbalanced CSS delimiter at byte {start}");
                    }
                }
                _ => {}
            }
            cursor.advance();
        }
        imports_allowed = false;
    }
    if !blocks.is_empty() {
        bail!("unclosed CSS block or function");
    }
    append(output, &text[copied..])
}

fn replace_url(
    output: &mut String,
    text: &str,
    copied: &mut usize,
    url: &Url,
    rebased: &str,
) -> Result<()> {
    append(output, &text[*copied..url.argument.start])?;
    append(output, "\"")?;
    for part in rebased.split_inclusive(['\\', '"']) {
        if part.ends_with(['\\', '"']) {
            append(output, &part[..part.len() - 1])?;
            append(output, "\\")?;
            append(output, &part[part.len() - 1..])?;
        } else {
            append(output, part)?;
        }
    }
    append(output, "\"")?;
    *copied = url.argument.end;
    Ok(())
}

struct Import {
    url: String,
    comments: String,
}

fn parse_import(cursor: &mut Cursor<'_>) -> Result<Import> {
    let mut comments = String::new();
    while let Some(trivia) = cursor.trivia()? {
        comments.push_str(trivia);
    }
    let url = if matches!(cursor.peek(), Some(b'\'' | b'"')) {
        cursor.string()?
    } else if cursor.name()?.eq_ignore_ascii_case("url") && cursor.peek() == Some(b'(') {
        cursor.url()?.value
    } else {
        bail!("@import requires a quoted path or url(path)");
    };
    while let Some(trivia) = cursor.trivia()? {
        comments.push_str(trivia);
    }
    if cursor.peek() != Some(b';') {
        bail!("only unconditional @import paths followed by ';' are supported");
    }
    cursor.advance();
    Ok(Import { url, comments })
}

fn is_relative(url: &str) -> bool {
    !url.starts_with(['/', '#'])
        && !url
            .split(['/', '?', '#'])
            .next()
            .expect("invariant: split is nonempty")
            .contains(':')
}

/// A normalized, percent-encoded publication path confined to the public root.
struct LocalUrl {
    path: String,
    suffix: String,
}

impl LocalUrl {
    fn resolve(base: &str, url: &str) -> Result<Self> {
        // The browser trims outer C0/space from a CSS-decoded URL before
        // resolving it. Reject this spelling rather than encode those spaces
        // as filename bytes. Percent-encoded %20 is intentionally not trimmed.
        if url.starts_with(' ') || url.ends_with(' ') {
            bail!("outer ASCII spaces in CSS local URLs are not supported: {url:?}");
        }
        if !is_relative(url)
            || url.is_empty()
            || url.contains('\\')
            || url.chars().any(char::is_control)
        {
            bail!("CSS imports and relative assets require a local path: {url:?}");
        }
        let end = url.find(['?', '#']).unwrap_or(url.len());
        let (path, suffix) = url.split_at(end);
        let mut parts: Vec<String> = base.split('/').map(str::to_owned).collect();
        if !path.is_empty() {
            parts.pop();
            let mut segments = path.split('/').peekable();
            while let Some(part) = segments.next() {
                if part.is_empty() {
                    bail!("empty CSS URL path segment: {url:?}");
                }
                // Decode before checking dot segments and filesystem boundaries.
                let decoded = decode_segment(part)?;
                if segments.peek().is_none() && matches!(decoded.as_str(), "." | "..") {
                    bail!("CSS URL names a directory: {url:?}");
                }
                match decoded.as_str() {
                    "." => {}
                    ".." => {
                        if parts.pop().is_none() {
                            bail!("CSS URL escapes the public root: {url:?}");
                        }
                    }
                    _ => parts.push(encode_segment(&decoded)),
                }
            }
        }
        if parts.is_empty() || path.ends_with('/') {
            bail!("CSS URL does not name a file: {url:?}");
        }
        Ok(Self {
            path: parts.join("/"),
            suffix: suffix.to_owned(),
        })
    }
}

fn encode_segment(segment: &str) -> String {
    use std::fmt::Write;
    let mut encoded = String::new();
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            write!(&mut encoded, "%{byte:02X}").expect("invariant: String writes cannot fail");
        }
    }
    encoded
}

fn decode_segment(segment: &str) -> Result<String> {
    let mut bytes = Vec::new();
    let mut index = 0;
    while index < segment.len() {
        let byte = segment.as_bytes()[index];
        if byte == b'%' {
            let digits = segment
                .as_bytes()
                .get(index + 1..index + 3)
                .context("incomplete percent escape in CSS URL")?;
            let high = (digits[0] as char)
                .to_digit(16)
                .context("invalid CSS URL percent escape")?;
            let low = (digits[1] as char)
                .to_digit(16)
                .context("invalid CSS URL percent escape")?;
            bytes.push((high * 16 + low) as u8);
            index += 3;
        } else {
            bytes.push(byte);
            index += 1;
        }
    }
    let decoded = String::from_utf8(bytes).context("non-UTF-8 CSS URL path")?;
    if decoded.contains(['/', '\\', ':']) || decoded.chars().any(char::is_control) {
        bail!("unsafe CSS URL path segment: {segment:?}");
    }
    Ok(decoded)
}

#[cfg(test)]
#[path = "../../../test/tools/web-build/unit/css_bundle_tests.rs"]
mod tests;
