//! Pure, I/O-free template functions producing [`EmailMessage`]s for each transactional flow.
//! Kept separate from the sending transport so composing a message is trivially unit-testable.

use super::EmailMessage;

/// Escapes the 5 HTML syntax characters (`&`, `<`, `>`, `"`, `'`) so an untrusted value (e.g. a
/// message snippet) can be safely interpolated into an HTML email body without enabling
/// HTML/script injection or phishing markup in a recipient's inbox.
///
/// `&` is escaped first so the later replacements don't get double-escaped.
///
/// # Examples
/// ```
/// use rim_infra_services::email::templates::escape_html;
/// assert_eq!(
///     escape_html("<script>alert(1)</script>"),
///     "&lt;script&gt;alert(1)&lt;/script&gt;"
/// );
/// assert_eq!(escape_html("Tom & Jerry's \"show\""), "Tom &amp; Jerry&#39;s &quot;show&quot;");
/// ```
pub fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Password-reset email. `link` is the full, already-built reset URL (or bare token if
/// `public_base_url` was unset — see `password_reset.rs`).
///
/// # Examples
/// ```
/// use rim_infra_services::email::templates::password_reset;
/// let msg = password_reset("user@example.com", "https://rim.example.com/reset-password/abc123");
/// assert_eq!(msg.to, "user@example.com");
/// assert!(msg.body_text.contains("https://rim.example.com/reset-password/abc123"));
/// ```
pub fn password_reset(to: &str, link: &str) -> EmailMessage {
    let subject = "Reset your RIM password".to_string();
    let body_text = format!(
        "We received a request to reset your RIM password.\n\n\
         Reset your password: {link}\n\n\
         This link expires in 1 hour. If you didn't request this, you can safely ignore this \
         email — your password will not be changed."
    );
    let body_html = Some(format!(
        "<p>We received a request to reset your RIM password.</p>\
         <p><a href=\"{link}\">Reset your password</a></p>\
         <p>This link expires in 1 hour. If you didn't request this, you can safely ignore this \
         email — your password will not be changed.</p>"
    ));

    EmailMessage {
        to: to.to_string(),
        subject,
        body_text,
        body_html,
    }
}

/// Invite email (server invite or instance invite). `link` is the full join URL (or bare token
/// if `public_base_url` was unset).
///
/// # Examples
/// ```
/// use rim_infra_services::email::templates::invite;
/// let msg = invite("user@example.com", "https://rim.example.com/join/abc123");
/// assert!(msg.body_text.contains("https://rim.example.com/join/abc123"));
/// ```
pub fn invite(to: &str, link: &str) -> EmailMessage {
    let subject = "You've been invited to RIM".to_string();
    let body_text = format!(
        "You've been invited to join a workspace on RIM.\n\n\
         Accept your invite: {link}"
    );
    let body_html = Some(format!(
        "<p>You've been invited to join a workspace on RIM.</p>\
         <p><a href=\"{link}\">Accept your invite</a></p>"
    ));

    EmailMessage {
        to: to.to_string(),
        subject,
        body_text,
        body_html,
    }
}

/// One message in a [`notification_digest`]: its text snippet (already cut to length, not yet
/// escaped) and its permalink, if `RIM_PUBLIC_BASE_URL` is configured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DigestItem {
    pub snippet: String,
    pub link: Option<String>,
}

/// The notifications of one conversation in a [`notification_digest`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DigestGroup {
    pub conversation_label: String,
    pub items: Vec<DigestItem>,
}

/// Unread-notification digest (Track 1.3): the count, then each conversation with its message
/// snippets and links. The subject names only the count and the workspace, so the log-only
/// sender (which logs recipient and subject) never logs message text. Snippets, conversation
/// labels and the workspace name are user content and are HTML-escaped in the HTML part.
///
/// # Examples
/// ```
/// use rim_infra_services::email::templates::{DigestGroup, DigestItem, notification_digest};
/// let groups = vec![DigestGroup {
///     conversation_label: "#ops".to_string(),
///     items: vec![DigestItem { snippet: "deploy failed".to_string(), link: None }],
/// }];
/// let msg = notification_digest("bob@example.com", "Acme", &groups, None);
/// assert!(msg.subject.contains("1 unread notification"));
/// assert!(msg.body_text.contains("deploy failed"));
/// ```
pub fn notification_digest(
    to: &str,
    workspace_name: &str,
    groups: &[DigestGroup],
    workspace_link: Option<&str>,
) -> EmailMessage {
    let count: usize = groups.iter().map(|g| g.items.len()).sum();
    let noun = if count == 1 {
        "notification"
    } else {
        "notifications"
    };
    let subject = format!("{count} unread {noun} in {workspace_name}");

    let mut body_text = format!("You have {count} unread {noun} in {workspace_name}.\n");
    let workspace_name_html = escape_html(workspace_name);
    let mut body_html =
        format!("<p>You have {count} unread {noun} in <strong>{workspace_name_html}</strong>.</p>");
    for group in groups {
        body_text.push_str(&format!("\n{}\n", group.conversation_label));
        body_html.push_str(&format!(
            "<h3>{}</h3><ul>",
            escape_html(&group.conversation_label)
        ));
        for item in &group.items {
            match &item.link {
                Some(link) => {
                    body_text.push_str(&format!("- {} ({link})\n", item.snippet));
                    body_html.push_str(&format!(
                        "<li><a href=\"{}\">{}</a></li>",
                        escape_html(link),
                        escape_html(&item.snippet)
                    ));
                }
                None => {
                    body_text.push_str(&format!("- {}\n", item.snippet));
                    body_html.push_str(&format!("<li>{}</li>", escape_html(&item.snippet)));
                }
            }
        }
        body_html.push_str("</ul>");
    }
    if let Some(link) = workspace_link {
        body_text.push_str(&format!("\nOpen RIM: {link}\n"));
        body_html.push_str(&format!(
            "<p><a href=\"{}\">Open RIM</a></p>",
            escape_html(link)
        ));
    }
    body_text.push_str(
        "\nYou get this email because your notification digest is on. Change it in Settings > \
         Notifications.",
    );
    body_html.push_str(
        "<p>You get this email because your notification digest is on. Change it in Settings \
         &gt; Notifications.</p>",
    );

    EmailMessage {
        to: to.to_string(),
        subject,
        body_text,
        body_html: Some(body_html),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_reset_includes_link_and_expiry_note() {
        let msg = password_reset(
            "user@example.com",
            "https://rim.example.com/reset-password/tok",
        );
        assert_eq!(msg.to, "user@example.com");
        assert!(!msg.subject.is_empty());
        assert!(
            msg.body_text
                .contains("https://rim.example.com/reset-password/tok")
        );
        assert!(msg.body_text.contains("1 hour"));
        let html = msg.body_html.expect("html part expected");
        assert!(html.contains("https://rim.example.com/reset-password/tok"));
    }

    #[test]
    fn invite_includes_link() {
        let msg = invite("user@example.com", "https://rim.example.com/join/tok");
        assert_eq!(msg.to, "user@example.com");
        assert!(!msg.subject.is_empty());
        assert!(msg.body_text.contains("https://rim.example.com/join/tok"));
        let html = msg.body_html.expect("html part expected");
        assert!(html.contains("https://rim.example.com/join/tok"));
    }

    #[test]
    fn escape_html_escapes_all_five_special_characters() {
        assert_eq!(
            escape_html(r#"<>&"'"#),
            "&lt;&gt;&amp;&quot;&#39;".to_string()
        );
        // `&` must be escaped first so it isn't re-escaped by the later replacements.
        assert_eq!(escape_html("&amp;"), "&amp;amp;");
    }

    fn digest_groups() -> Vec<DigestGroup> {
        vec![
            DigestGroup {
                conversation_label: "#ops".to_string(),
                items: vec![
                    DigestItem {
                        snippet: "deploy failed".to_string(),
                        link: Some(
                            "https://rim.example.com/workspaces/w/conversations/c/messages/m1"
                                .to_string(),
                        ),
                    },
                    DigestItem {
                        snippet: "rollback done".to_string(),
                        link: None,
                    },
                ],
            },
            DigestGroup {
                conversation_label: "Direct message".to_string(),
                items: vec![DigestItem {
                    snippet: "lunch?".to_string(),
                    link: None,
                }],
            },
        ]
    }

    #[test]
    fn notification_digest_counts_groups_and_links() {
        let msg = notification_digest(
            "bob@example.com",
            "Acme",
            &digest_groups(),
            Some("https://rim.example.com/workspaces/w"),
        );
        assert_eq!(msg.to, "bob@example.com");
        assert_eq!(msg.subject, "3 unread notifications in Acme");
        assert!(msg.body_text.contains("#ops"));
        assert!(msg.body_text.contains(
            "- deploy failed (https://rim.example.com/workspaces/w/conversations/c/messages/m1)"
        ));
        assert!(msg.body_text.contains("- rollback done\n"));
        assert!(msg.body_text.contains("Direct message"));
        assert!(
            msg.body_text
                .contains("Open RIM: https://rim.example.com/workspaces/w")
        );
        // Conversation order and item order are kept.
        let ops = msg.body_text.find("#ops").expect("ops");
        let dm = msg.body_text.find("Direct message").expect("dm");
        assert!(ops < dm);
        let html = msg.body_html.expect("html part expected");
        assert!(html.contains("<h3>#ops</h3>"));
        assert!(html.contains("<a href=\"https://rim.example.com/workspaces/w/conversations/c/messages/m1\">deploy failed</a>"));
        assert!(html.contains("<li>rollback done</li>"));
    }

    #[test]
    fn notification_digest_singular_subject_without_links() {
        let groups = vec![DigestGroup {
            conversation_label: "#ops".to_string(),
            items: vec![DigestItem {
                snippet: "one".to_string(),
                link: None,
            }],
        }];
        let msg = notification_digest("bob@example.com", "Acme", &groups, None);
        assert_eq!(msg.subject, "1 unread notification in Acme");
        assert!(!msg.body_text.contains("Open RIM"));
        assert!(!msg.body_html.expect("html").contains("<a href"));
    }

    /// Message text, conversation names and the workspace name are user content: the HTML part
    /// escapes them, and the subject never carries message text.
    #[test]
    fn notification_digest_escapes_user_content() {
        let groups = vec![DigestGroup {
            conversation_label: "#<img src=x>".to_string(),
            items: vec![DigestItem {
                snippet: "<script>alert(1)</script>".to_string(),
                link: Some("https://rim.example.com/a\"onmouseover=\"x".to_string()),
            }],
        }];
        let msg = notification_digest("bob@example.com", "<b>Acme</b>", &groups, None);
        assert!(!msg.subject.contains("script"));
        let html = msg.body_html.expect("html part expected");
        assert!(!html.contains("<script>"));
        assert!(!html.contains("<img"));
        assert!(!html.contains("<b>Acme</b>"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(html.contains("&lt;b&gt;Acme&lt;/b&gt;"));
        assert!(html.contains("a&quot;onmouseover=&quot;x"));
    }
}
