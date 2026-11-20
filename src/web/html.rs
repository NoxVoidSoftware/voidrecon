//! Minimal HTML handling: readable text extraction and DuckDuckGo result parsing.

use super::SearchResult;

/// Elements whose content is never readable text.
const SKIPPED_ELEMENTS: [&str; 6] = ["script", "style", "noscript", "svg", "template", "head"];
/// Elements that start a new line in the extracted text.
const BLOCK_ELEMENTS: [&str; 24] = [
    "p", "div", "br", "li", "ul", "ol", "tr", "table", "section", "article", "header", "footer",
    "h1", "h2", "h3", "h4", "h5", "h6", "pre", "blockquote", "hr", "dt", "dd", "title",
];

/// Converts HTML to plain text: drops scripts/styles, turns block elements
/// into line breaks, decodes entities and collapses whitespace.
pub fn html_to_text(html: &str) -> String {
    let mut text = String::with_capacity(html.len() / 2);
    let mut rest = html;
    while let Some(open) = rest.find('<') {
        text.push_str(&rest[..open]);
        rest = &rest[open..];
        let Some(close) = rest.find('>') else {
            rest = "";
            break;
        };
        let tag = &rest[1..close];
        rest = &rest[close + 1..];

        if tag.starts_with("!--") {
            // Comment: skip to its end (the tag may already contain it).
            if !tag.ends_with("--") {
                rest = rest.find("-->").map_or("", |end| &rest[end + 3..]);
            }
            continue;
        }
        let name = tag
            .trim_start_matches('/')
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        if !tag.starts_with('/') && SKIPPED_ELEMENTS.contains(&name.as_str()) {
            let closing = format!("</{}", name);
            rest = find_ignore_case(rest, &closing)
                .and_then(|end| rest[end..].find('>').map(|gt| &rest[end + gt + 1..]))
                .unwrap_or("");
            continue;
        }
        if BLOCK_ELEMENTS.contains(&name.as_str()) {
            if !text.ends_with('\n') {
                text.push('\n');
            }
        } else if name == "td" || name == "th" {
            text.push(' ');
        }
    }
    text.push_str(rest);
    normalize_whitespace(&decode_entities(&text))
}

fn find_ignore_case(haystack: &str, needle: &str) -> Option<usize> {
    let needle = needle.to_ascii_lowercase();
    haystack
        .char_indices()
        .map(|(index, _)| index)
        .find(|&index| {
            haystack
                .get(index..index + needle.len())
                .is_some_and(|slice| slice.eq_ignore_ascii_case(&needle))
        })
}

/// Decodes the common named entities and numeric character references.
pub fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let entity_end = rest[1..].find(';').map(|i| i + 1).filter(|&i| i <= 10);
        let decoded = entity_end.and_then(|end| {
            let entity = &rest[1..end];
            let character = match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some(' '),
                _ => entity.strip_prefix('#').and_then(|number| {
                    let code = match number.strip_prefix(['x', 'X']) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok(),
                        None => number.parse().ok(),
                    };
                    code.and_then(char::from_u32)
                }),
            };
            character.map(|c| (c, end))
        });
        match decoded {
            Some((character, end)) => {
                out.push(character);
                rest = &rest[end + 1..];
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

/// Trims lines, collapses runs of spaces and keeps at most one blank line.
fn normalize_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank_lines = 0;
    for line in text.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            blank_lines += 1;
            if blank_lines > 1 || out.is_empty() {
                continue;
            }
        } else {
            blank_lines = 0;
        }
        out.push_str(&line);
        out.push('\n');
    }
    out.trim().to_string()
}

/// Extracts results from DuckDuckGo's HTML endpoint, skipping ads.
pub fn parse_duckduckgo_results(body: &str) -> Vec<SearchResult> {
    const RESULT_LINK: &str = "class=\"result__a\"";
    const SNIPPET: &str = "class=\"result__snippet\"";
    let mut results = Vec::new();
    let mut search_from = 0;
    while let Some(found) = body[search_from..].find(RESULT_LINK) {
        let class_pos = search_from + found;
        search_from = class_pos + RESULT_LINK.len();
        let Some(tag_start) = body[..class_pos].rfind('<') else {
            continue;
        };
        let Some(tag_len) = body[class_pos..].find('>') else {
            break;
        };
        let tag_end = class_pos + tag_len;
        let Some(href) = attribute(&body[tag_start..tag_end], "href") else {
            continue;
        };
        let Some(url) = resolve_duckduckgo_link(&href) else {
            continue;
        };
        let title_end = body[tag_end..].find("</a>").map_or(body.len(), |end| tag_end + end);
        let title = html_to_text(&body[tag_end + 1..title_end]);

        // The snippet belongs to this result only if it comes before the next one.
        let next_result = body[search_from..].find(RESULT_LINK).map_or(body.len(), |i| search_from + i);
        let snippet = body[tag_end..next_result]
            .find(SNIPPET)
            .and_then(|i| {
                let start = tag_end + i;
                let content_start = start + body[start..].find('>')? + 1;
                let content_end = body[content_start..next_result]
                    .find("</a>")
                    .or_else(|| body[content_start..next_result].find("</td>"))
                    .map_or(next_result, |end| content_start + end);
                Some(html_to_text(&body[content_start..content_end]))
            })
            .unwrap_or_default();

        if !results.iter().any(|existing: &SearchResult| existing.url == url) {
            results.push(SearchResult { title, url, snippet });
        }
    }
    results
}

fn attribute(tag: &str, name: &str) -> Option<String> {
    let marker = format!("{}=\"", name);
    let start = tag.find(&marker)? + marker.len();
    let end = tag[start..].find('"')? + start;
    Some(decode_entities(&tag[start..end]))
}

/// Result links point at a DuckDuckGo redirect carrying the target in `uddg`.
fn resolve_duckduckgo_link(href: &str) -> Option<String> {
    let absolute = if href.starts_with("//") {
        format!("https:{}", href)
    } else {
        href.to_string()
    };
    let url = reqwest::Url::parse(&absolute).ok()?;
    let host = url.host_str().unwrap_or("");
    if host.ends_with("duckduckgo.com") {
        if url.path().starts_with("/y.js") {
            return None; // ad
        }
        return url
            .query_pairs()
            .find(|(key, _)| key == "uddg")
            .map(|(_, value)| value.into_owned());
    }
    Some(absolute)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_to_text_keeps_readable_content_only() {
        let html = "<html><head><title>T</title><style>p{}</style></head><body>\
            <script>var x = '<p>';</script><h1>Title &amp; more</h1><!-- note -->\
            <p>First&nbsp;para &#39;quoted&#39; &#x41;</p><ul><li>one</li><li>two</li></ul></body></html>";
        assert_eq!(html_to_text(html), "Title & more\nFirst para 'quoted' A\none\ntwo");
    }

    #[test]
    fn duckduckgo_results_are_parsed_and_ads_skipped() {
        let body = r#"
            <a rel="nofollow" class="result__a" href="//duckduckgo.com/y.js?ad_domain=x">Ad</a>
            <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fnvd.nist.gov%2Fvuln%2Fdetail%2FCVE-2021-41773&amp;rut=abc">CVE-2021-41773 <b>Detail</b></a>
            <a class="result__snippet" href="x">Path traversal in <b>Apache</b> 2.4.49</a>
            <a rel="nofollow" class="result__a" href="https://example.com/page">Example</a>
        "#;
        let results = parse_duckduckgo_results(body);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].url, "https://nvd.nist.gov/vuln/detail/CVE-2021-41773");
        assert_eq!(results[0].title, "CVE-2021-41773 Detail");
        assert_eq!(results[0].snippet, "Path traversal in Apache 2.4.49");
        assert_eq!(results[1].url, "https://example.com/page");
        assert_eq!(results[1].snippet, "");
    }
}
