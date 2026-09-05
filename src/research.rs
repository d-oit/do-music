//! Web research for art direction: fetch visual references and distil them
//! into short notes via the LLM.
//!
//! Split from `providers.rs` to keep both files under the ~500-line rule.
//! The one network call here is strictly best-effort: research is an
//! enhancement, never a reason to fail a render.

use anyhow::Result;
use reqwest::Client;
use std::time::Duration;

use crate::providers::chat_completion;

/// Fetch visual references from the web and distil them into short notes.
///
/// Uses DuckDuckGo's HTML endpoint (no API key, no account). Research is
/// strictly best-effort: any failure returns `Ok(None)` so the caller
/// proceeds with LLM-only art direction rather than failing the render.
pub async fn research_visual_style(
    key: &str,
    base: &str,
    model: &str,
    music_prompt: &str,
) -> Result<Option<String>> {
    use crate::artdirection::{RESEARCH_SYSTEM_PROMPT, research_query};
    let query = research_query(music_prompt);
    let response = Client::new()
        .get("https://html.duckduckgo.com/html/")
        .query(&[("q", query.as_str())])
        .header("User-Agent", "Mozilla/5.0 (compatible; do-music/0.1)")
        .timeout(Duration::from_secs(20))
        .send()
        .await;
    let Ok(response) = response else {
        return Ok(None);
    };
    let Ok(html) = response.text().await else {
        return Ok(None);
    };
    let snippets = extract_search_snippets(&html, 8);
    if snippets.is_empty() {
        return Ok(None);
    }
    let notes = chat_completion(
        key,
        base,
        model,
        RESEARCH_SYSTEM_PROMPT,
        &snippets,
        0.3,
        false,
    )
    .await?;
    let notes = notes.trim().to_string();
    Ok(if notes.is_empty() { None } else { Some(notes) })
}

/// Pull result snippets out of a DuckDuckGo HTML page.
///
/// Deliberately a tiny tag-stripper rather than a scraping dependency: the
/// text only ever reaches an LLM summarizer, so imperfect extraction costs
/// quality, not correctness. Pure, so it is offline-testable.
pub fn extract_search_snippets(html: &str, limit: usize) -> String {
    let mut out: Vec<String> = Vec::new();
    for chunk in html.split("result__snippet").skip(1) {
        let Some(start) = chunk.find('>') else {
            continue;
        };
        let Some(end) = chunk[start..].find("</a>") else {
            continue;
        };
        let text = strip_tags(&chunk[start + 1..start + end]);
        let text = text.trim();
        if text.len() > 30 {
            out.push(text.to_string());
        }
        if out.len() >= limit {
            break;
        }
    }
    out.join("\n")
}

/// Remove HTML tags and decode the handful of entities DuckDuckGo emits.
fn strip_tags(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut depth = 0usize;
    for ch in input.chars() {
        match ch {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            c if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_snippets_and_strips_markup() {
        let html = concat!(
            r#"<a class="result__snippet">Warm <b>golden</b> light over terraced rice fields, "#,
            r#"misty mountains &amp; soft haze</a>"#,
            r#"<a class="result__snippet">Deep indigo night skies with paper lanterns and "#,
            r#"still reflective water</a>"#
        );
        let out = extract_search_snippets(html, 8);
        assert!(out.contains("golden"), "{out}");
        assert!(!out.contains("<b>"), "markup leaked: {out}");
        assert!(out.contains('&'), "entity not decoded: {out}");
        assert_eq!(out.lines().count(), 2, "{out}");
    }

    #[test]
    fn drops_fragments_that_are_too_short_to_be_useful() {
        let html = r#"<a class="result__snippet">short</a>"#.repeat(5);
        assert!(extract_search_snippets(&html, 3).is_empty());
    }

    #[test]
    fn honours_the_result_limit() {
        let one = concat!(
            r#"<a class="result__snippet">a vivid painterly reference phrase for testing"#,
            r#"</a>"#
        );
        let many = one.repeat(10);
        assert_eq!(extract_search_snippets(&many, 3).lines().count(), 3);
    }

    #[test]
    fn non_ascii_snippets_do_not_panic_on_slice_boundaries() {
        // find() returns byte offsets; slicing on them is only sound because
        // the delimiters ('>' and "</a>") are ASCII. Guard that invariant.
        let html = concat!(
            r#"<a class="result__snippet">Grüße über Tokyo — 東京の夜景 with paper "#,
            r#"lanterns and soft haze</a>"#
        );
        let out = extract_search_snippets(html, 4);
        assert!(out.contains("東京"), "{out}");
        assert!(out.contains("Grüße"), "{out}");
    }

    #[test]
    fn empty_or_junk_html_yields_nothing() {
        assert!(extract_search_snippets("", 5).is_empty());
        assert!(extract_search_snippets("<html><body>no results</body></html>", 5).is_empty());
    }
}
