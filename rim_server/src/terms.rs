//! The terms page `RIM_TERMS_FILE` names: read once at start, rendered from a small subset of
//! Markdown into an HTML page in which every character of the source is escaped and `<!-- -->`
//! comments are left out, and served at [`TERMS_PATH`] under a policy that allows no script, no
//! frame and no request of any kind.

use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use actix_web::{HttpResponse, web};
use base64::Engine;
use sha2::{Digest, Sha256};

/// Where the server serves the terms page.
pub(crate) const TERMS_PATH: &str = "/api/terms";

/// Largest terms file, in bytes.
pub(crate) const MAX_TERMS_FILE_BYTES: usize = 64 * 1024;

/// What marks text an operator has to replace before the file is served.
const PLACEHOLDER: &str = "FILL:";

/// Title of a page whose source has no top-level heading.
const DEFAULT_TITLE: &str = "Terms of use";

/// The page's only style sheet; the content security policy allows it by its hash.
const STYLE: &str = "body{margin:0;font:16px/1.6 system-ui,sans-serif;color-scheme:light dark}\
main{max-width:42rem;margin:0 auto;padding:1rem 1rem 3rem}\
h1,h2,h3{line-height:1.25}code{font-size:.9em}a{overflow-wrap:anywhere}";

/// The rendered terms page and the URL a signup accepts it by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TermsPage {
    url: String,
    html: String,
    content_security_policy: String,
}

impl TermsPage {
    /// Read the Markdown file at `path` and render it; its URL is [`TERMS_PATH`] under
    /// `origin`, with a `v` parameter naming the file's content, so a changed file has a new
    /// URL.
    ///
    /// # Errors
    /// A message naming `RIM_TERMS_FILE` when the file cannot be read, is larger than
    /// [`MAX_TERMS_FILE_BYTES`], is not UTF-8, holds only whitespace and comments, or still
    /// has a `FILL:` placeholder outside a comment.
    pub(crate) fn load(path: &Path, origin: &str) -> Result<Self, String> {
        let name = path.display();
        let file = std::fs::File::open(path)
            .map_err(|e| format!("RIM_TERMS_FILE={name} cannot be read: {e}"))?;
        let mut bytes = Vec::new();
        file.take(MAX_TERMS_FILE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| format!("RIM_TERMS_FILE={name} cannot be read: {e}"))?;
        if bytes.len() > MAX_TERMS_FILE_BYTES {
            return Err(format!(
                "RIM_TERMS_FILE={name} is larger than {MAX_TERMS_FILE_BYTES} bytes"
            ));
        }
        let markdown = String::from_utf8(bytes)
            .map_err(|_| format!("RIM_TERMS_FILE={name} is not UTF-8 text"))?;
        let shown = strip_comments(&markdown);
        if shown.trim().is_empty() {
            return Err(format!("RIM_TERMS_FILE={name} is empty"));
        }
        if shown.contains(PLACEHOLDER) {
            return Err(format!(
                "RIM_TERMS_FILE={name} still has {PLACEHOLDER} placeholders: replace each with \
                 the text it asks for"
            ));
        }
        Ok(Self::from_markdown(&markdown, origin))
    }

    /// The page for `markdown`, with its URL under `origin`.
    pub(crate) fn from_markdown(markdown: &str, origin: &str) -> Self {
        let digest = Sha256::digest(markdown.as_bytes());
        let version: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
        let style_hash = base64::engine::general_purpose::STANDARD.encode(Sha256::digest(STYLE));
        Self {
            url: format!("{}{TERMS_PATH}?v={version}", origin.trim_end_matches('/')),
            html: render_page(markdown),
            content_security_policy: format!(
                "default-src 'none'; style-src 'sha256-{style_hash}'; base-uri 'none'; \
                 form-action 'none'; frame-ancestors 'none'"
            ),
        }
    }

    /// The URL a signup accepts the terms by.
    pub(crate) fn url(&self) -> &str {
        &self.url
    }
}

/// `GET /api/terms`: the terms page, cacheable for five minutes.
pub(crate) async fn handle(page: web::Data<Arc<TermsPage>>) -> HttpResponse {
    HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .insert_header((
            "content-security-policy",
            page.content_security_policy.as_str(),
        ))
        .insert_header(("x-content-type-options", "nosniff"))
        .insert_header(("referrer-policy", "no-referrer"))
        .insert_header(("cache-control", "public, max-age=300"))
        .body(page.html.clone())
}

/// `markdown` without its `<!-- … -->` comments; an unclosed comment runs to the end.
fn strip_comments(markdown: &str) -> String {
    let mut out = String::with_capacity(markdown.len());
    let mut rest = markdown;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        rest = match rest[start + 4..].find("-->") {
            Some(end) => &rest[start + 4 + end + 3..],
            None => "",
        };
    }
    out.push_str(rest);
    out
}

/// The whole HTML document for `markdown`, without its comments, titled by its first
/// top-level heading.
fn render_page(markdown: &str) -> String {
    let markdown = &strip_comments(markdown);
    let title = markdown
        .lines()
        .find_map(|line| line.trim().strip_prefix("# "))
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .unwrap_or(DEFAULT_TITLE);
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <meta name=\"referrer\" content=\"no-referrer\">\n<title>{}</title>\n\
         <style>{STYLE}</style>\n</head>\n<body>\n<main>\n{}</main>\n</body>\n</html>\n",
        escape(&plain_inline(title)),
        render_blocks(markdown)
    )
}

/// An open block while lines are read.
enum Block {
    None,
    Paragraph(Vec<String>),
    List { ordered: bool, items: Vec<String> },
}

/// The blocks of `markdown` as HTML: headings (`#` to `######`), paragraphs, bulleted lists
/// (`-`, `*`, `+`), numbered lists (`1.`) and rules (`---`). A line that continues a list item
/// joins it; anything else is text.
fn render_blocks(markdown: &str) -> String {
    let mut out = String::new();
    let mut open = Block::None;
    for line in markdown.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            close(&mut open, &mut out);
            continue;
        }
        if let Some((level, text)) = heading(trimmed) {
            close(&mut open, &mut out);
            out.push_str(&format!("<h{level}>{}</h{level}>\n", inline(text)));
            continue;
        }
        if is_rule(trimmed) {
            close(&mut open, &mut out);
            out.push_str("<hr>\n");
            continue;
        }
        if let Some((ordered, text)) = list_item(trimmed) {
            match &mut open {
                Block::List {
                    ordered: same,
                    items,
                } if *same == ordered => items.push(text.to_string()),
                _ => {
                    close(&mut open, &mut out);
                    open = Block::List {
                        ordered,
                        items: vec![text.to_string()],
                    };
                }
            }
            continue;
        }
        match &mut open {
            Block::Paragraph(lines) => lines.push(trimmed.to_string()),
            Block::List { items, .. } => {
                if let Some(last) = items.last_mut() {
                    last.push(' ');
                    last.push_str(trimmed);
                }
            }
            Block::None => open = Block::Paragraph(vec![trimmed.to_string()]),
        }
    }
    close(&mut open, &mut out);
    out
}

/// Write the open block to `out` and leave none open.
fn close(open: &mut Block, out: &mut String) {
    match std::mem::replace(open, Block::None) {
        Block::None => {}
        Block::Paragraph(lines) => {
            out.push_str(&format!("<p>{}</p>\n", inline(&lines.join(" "))));
        }
        Block::List { ordered, items } => {
            let tag = if ordered { "ol" } else { "ul" };
            out.push_str(&format!("<{tag}>\n"));
            for item in items {
                out.push_str(&format!("<li>{}</li>\n", inline(&item)));
            }
            out.push_str(&format!("</{tag}>\n"));
        }
    }
}

/// The level and text of a heading line such as `## Moderation`.
fn heading(line: &str) -> Option<(usize, &str)> {
    let level = line.chars().take_while(|&c| c == '#').count();
    let text = line[level..].strip_prefix(' ')?;
    (1..=6)
        .contains(&level)
        .then(|| (level, text.trim().trim_end_matches('#').trim_end()))
}

/// Whether `line` is a rule: three or more `-`, `*` or `_` and nothing else.
fn is_rule(line: &str) -> bool {
    let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    compact.len() >= 3
        && ['-', '*', '_']
            .iter()
            .any(|&mark| compact.chars().all(|c| c == mark))
}

/// Whether the item is numbered, and its text, for a list line such as `- text` or `2. text`.
fn list_item(line: &str) -> Option<(bool, &str)> {
    for marker in ["- ", "* ", "+ "] {
        if let Some(text) = line.strip_prefix(marker) {
            return Some((false, text.trim()));
        }
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if (1..=9).contains(&digits) {
        let rest = &line[digits..];
        if let Some(text) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            return Some((true, text.trim()));
        }
    }
    None
}

/// `text` with its inline markup as HTML: `**strong**`, `*emphasis*`, `` `code` ``,
/// `[label](url)` for an `https`, `http` or `mailto` URL, `<https://…>` and `<name@host>`
/// links, and `\` escapes. Everything else is escaped text.
fn inline(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if c == '\\'
            && let Some(next) = rest[1..].chars().next()
            && next.is_ascii_punctuation()
        {
            out.push_str(&escape(&next.to_string()));
            rest = &rest[1 + next.len_utf8()..];
            continue;
        }
        if let Some(inner) = rest.strip_prefix("**")
            && let Some(end) = inner.find("**")
            && end > 0
        {
            out.push_str(&format!("<strong>{}</strong>", inline(&inner[..end])));
            rest = &inner[end + 2..];
            continue;
        }
        if let Some(inner) = rest.strip_prefix('*')
            && let Some(end) = inner.find('*')
            && end > 0
            && !inner.starts_with(' ')
        {
            out.push_str(&format!("<em>{}</em>", inline(&inner[..end])));
            rest = &inner[end + 1..];
            continue;
        }
        if let Some(inner) = rest.strip_prefix('`')
            && let Some(end) = inner.find('`')
        {
            out.push_str(&format!("<code>{}</code>", escape(&inner[..end])));
            rest = &inner[end + 1..];
            continue;
        }
        if let Some((label, url, len)) = markdown_link(rest) {
            out.push_str(&format!(
                "<a href=\"{}\">{}</a>",
                escape(&url),
                inline(label)
            ));
            rest = &rest[len..];
            continue;
        }
        if let Some((url, label, len)) = angle_link(rest) {
            out.push_str(&format!(
                "<a href=\"{}\">{}</a>",
                escape(&url),
                escape(label)
            ));
            rest = &rest[len..];
            continue;
        }
        out.push_str(&escape(&c.to_string()));
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// `text` without its inline markup, for the page title.
fn plain_inline(text: &str) -> String {
    text.replace("**", "").replace(['`', '*'], "")
}

/// The label, URL and length of a `[label](url)` link at the start of `text`, when the URL is
/// one a link may carry.
fn markdown_link(text: &str) -> Option<(&str, String, usize)> {
    let inner = text.strip_prefix('[')?;
    let label_end = inner.find("](")?;
    let label = &inner[..label_end];
    if label.is_empty() || label.contains('[') {
        return None;
    }
    let after = &inner[label_end + 2..];
    let url_end = after.find(')')?;
    let url = link_url(&after[..url_end])?;
    Some((label, url, 1 + label_end + 2 + url_end + 1))
}

/// The URL, label and length of a `<https://…>` or `<name@host>` link at the start of `text`.
fn angle_link(text: &str) -> Option<(String, &str, usize)> {
    let inner = text.strip_prefix('<')?;
    let end = inner.find('>')?;
    let target = &inner[..end];
    if target.contains(char::is_whitespace) {
        return None;
    }
    let url = if target.contains("://") {
        link_url(target)?
    } else if target.contains('@') && !target.contains(':') {
        link_url(&format!("mailto:{target}"))?
    } else {
        return None;
    };
    Some((url, target, end + 2))
}

/// `raw` when it is an absolute `https`, `http` or `mailto` URL, the only kinds a link on the
/// page may carry.
fn link_url(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let url = url::Url::parse(raw).ok()?;
    match url.scheme() {
        "https" | "http" if url.host().is_some() => Some(url.to_string()),
        "mailto" if raw.len() > "mailto:".len() => Some(url.to_string()),
        _ => None,
    }
}

/// `text` with `&`, `<`, `>`, `"` and `'` escaped for HTML text and attribute values.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(markdown: &str) -> String {
        render_blocks(markdown)
    }

    #[test]
    fn headings_paragraphs_lists_and_rules_render() {
        let html = body(
            "# Terms\n\nFirst line\nsecond line.\n\n## Use\n\n- one\n  more\n- two\n\n\
             1. first\n2. second\n\n---\n",
        );
        assert_eq!(
            html,
            "<h1>Terms</h1>\n<p>First line second line.</p>\n<h2>Use</h2>\n\
             <ul>\n<li>one more</li>\n<li>two</li>\n</ul>\n\
             <ol>\n<li>first</li>\n<li>second</li>\n</ol>\n<hr>\n"
        );
    }

    #[test]
    fn inline_markup_renders_and_the_rest_is_escaped() {
        assert_eq!(
            inline("**Bold** and *soft* `a<b` & \"q\" 'x' \\*literal\\*"),
            "<strong>Bold</strong> and <em>soft</em> <code>a&lt;b</code> &amp; &quot;q&quot; \
             &#39;x&#39; *literal*"
        );
    }

    #[test]
    fn links_carry_only_https_http_and_mailto_urls() {
        assert_eq!(
            inline("[self-hosting](https://rim.example.org/docs?a=1&b=2)"),
            "<a href=\"https://rim.example.org/docs?a=1&amp;b=2\">self-hosting</a>"
        );
        assert_eq!(
            inline("write to <abuse@rim.example.org>"),
            "write to <a href=\"mailto:abuse@rim.example.org\">abuse@rim.example.org</a>"
        );
        assert_eq!(
            inline("<https://rim.example.org/abuse>"),
            "<a href=\"https://rim.example.org/abuse\">https://rim.example.org/abuse</a>"
        );
        for hostile in [
            "[x](javascript:alert(1))",
            "[x](data:text/html,<script>alert(1)</script>)",
            "[x](/relative)",
            "<javascript:alert(1)>",
        ] {
            let html = inline(hostile);
            assert!(!html.contains("<a "), "{hostile} became {html}");
            assert!(!html.contains("<script"), "{hostile} became {html}");
        }
    }

    #[test]
    fn raw_html_in_the_source_is_text() {
        let html = render_page(
            "# <img src=x onerror=alert(1)>\n\n<script>alert(1)</script>\n\n\
             - <iframe src=\"https://evil.example\"></iframe>\n",
        );
        assert!(!html.contains("<script>alert"), "{html}");
        assert!(!html.contains("<img"), "{html}");
        assert!(!html.contains("<iframe"), "{html}");
        assert!(
            html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"),
            "{html}"
        );
        assert!(
            html.contains("<title>&lt;img src=x onerror=alert(1)&gt;</title>"),
            "{html}"
        );
    }

    #[test]
    fn comments_are_left_out_of_the_page() {
        assert_eq!(
            strip_comments("a<!-- note\nfor operators -->b<!-- unclosed\nc"),
            "ab"
        );
        let html = render_page("<!-- Edit before use. -->\n# Terms\n\nText.\n");
        assert!(!html.contains("Edit before use"), "{html}");
        assert!(html.contains("<title>Terms</title>"), "{html}");
    }

    #[test]
    fn a_file_with_placeholders_or_only_comments_is_refused() {
        let dir = std::env::temp_dir();
        let write = |content: &str| {
            let path = dir.join(format!("rim-terms-{}.md", uuid::Uuid::now_v7()));
            std::fs::write(&path, content).expect("write");
            path
        };
        let filled = write("<!-- FILL: in comments is fine -->\n# Terms\n");
        assert!(TermsPage::load(&filled, "https://rim.example.org").is_ok());
        let placeholder = write("# Terms\n\nRun by FILL: legal name.\n");
        let err = TermsPage::load(&placeholder, "https://rim.example.org").unwrap_err();
        assert!(err.contains("FILL: placeholders"), "{err}");
        let comments = write("<!-- nothing else -->\n");
        let err = TermsPage::load(&comments, "https://rim.example.org").unwrap_err();
        assert!(err.contains("is empty"), "{err}");
        for path in [filled, placeholder, comments] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn the_title_is_the_first_top_level_heading_or_the_default() {
        assert!(render_page("## Sub\n\n# Our **terms**\n").contains("<title>Our terms</title>"));
        assert!(render_page("No heading.\n").contains("<title>Terms of use</title>"));
    }

    #[test]
    fn the_url_names_the_content_and_the_policy_allows_only_the_style() {
        let a = TermsPage::from_markdown("# Terms\n", "https://rim.example.org/");
        let b = TermsPage::from_markdown("# Terms\n\nChanged.\n", "https://rim.example.org");
        assert!(a.url().starts_with("https://rim.example.org/api/terms?v="));
        assert_eq!(
            a.url().len(),
            "https://rim.example.org/api/terms?v=".len() + 16
        );
        assert_ne!(a.url(), b.url());
        assert_eq!(
            a.url(),
            TermsPage::from_markdown("# Terms\n", "https://rim.example.org").url()
        );
        assert!(
            a.content_security_policy
                .starts_with("default-src 'none'; style-src 'sha256-")
        );
        assert!(a.content_security_policy.contains("frame-ancestors 'none'"));
    }

    #[actix_web::test]
    async fn the_handler_serves_the_page_with_its_headers() {
        use actix_web::{App, test};

        let page = Arc::new(TermsPage::from_markdown(
            "# Terms\n\nBe kind.\n",
            "https://rim.example.org",
        ));
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(Arc::clone(&page)))
                .route(TERMS_PATH, web::get().to(handle)),
        )
        .await;
        let res = test::call_service(
            &app,
            test::TestRequest::get()
                .uri("/api/terms?v=anything")
                .to_request(),
        )
        .await;
        assert!(res.status().is_success());
        let header = |name: &str| {
            res.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        assert_eq!(
            header("content-type").as_deref(),
            Some("text/html; charset=utf-8")
        );
        assert_eq!(
            header("content-security-policy").as_deref(),
            Some(page.content_security_policy.as_str())
        );
        assert_eq!(header("x-content-type-options").as_deref(), Some("nosniff"));
        let html = String::from_utf8(test::read_body(res).await.to_vec()).expect("utf-8");
        assert!(html.contains("<p>Be kind.</p>"), "{html}");
    }
}
