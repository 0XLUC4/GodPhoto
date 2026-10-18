//! Just enough XML for the small documents inside layered files: tags and attributes.

/// One XML tag, enough of XML for the small documents in layered files.
pub(super) struct Tag<'a> {
    pub name: &'a str,
    pub closing: bool,
    /// Self-closing: `<layer/>`.
    pub empty: bool,
    attrs: Vec<(&'a str, String)>,
}

impl Tag<'_> {
    /// The unescaped value of attribute `key`.
    pub fn attr(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// The tags of `xml` in order, skipping declarations and comments.
pub(super) fn tags(xml: &str) -> Vec<Tag<'_>> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find('<') {
        rest = &rest[start + 1..];
        if let Some(comment) = rest.strip_prefix("!--") {
            let Some(end) = comment.find("-->") else {
                break;
            };
            rest = &comment[end + 3..];
            continue;
        }
        let Some(end) = rest.find('>') else { break };
        let body = &rest[..end];
        rest = &rest[end + 1..];
        if body.starts_with('?') || body.starts_with('!') {
            continue;
        }
        let closing = body.starts_with('/');
        let empty = body.ends_with('/');
        let body = body.trim_start_matches('/').trim_end_matches('/');
        let name_end = body.find(char::is_whitespace).unwrap_or(body.len());
        out.push(Tag {
            name: &body[..name_end],
            closing,
            empty,
            attrs: attributes(&body[name_end..]),
        });
    }
    out
}

fn attributes(mut text: &str) -> Vec<(&str, String)> {
    let mut out = Vec::new();
    while let Some(eq) = text.find('=') {
        let key = text[..eq].trim();
        let after = text[eq + 1..].trim_start();
        let Some(quote) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else {
            break;
        };
        let value = &after[1..];
        let Some(close) = value.find(quote) else {
            break;
        };
        out.push((key, unescape(&value[..close])));
        text = &value[close + 1..];
    }
    out
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let decoded = rest.find(';').and_then(|semi| {
            let entity = &rest[1..semi];
            let c = match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ => {
                    let code = match entity
                        .strip_prefix("#x")
                        .or_else(|| entity.strip_prefix("#X"))
                    {
                        Some(hex) => u32::from_str_radix(hex, 16).ok(),
                        None => entity.strip_prefix('#').and_then(|dec| dec.parse().ok()),
                    };
                    code.and_then(char::from_u32)
                }
            };
            c.map(|c| (c, semi))
        });
        match decoded {
            Some((c, semi)) => {
                out.push(c);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}
