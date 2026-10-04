//! Pure, I/O-free template functions producing [`EmailMessage`]s for each transactional flow.
//! Kept separate from the sending transport so composing a message is trivially unit-testable.

use super::EmailMessage;

/// Escapes the 5 HTML syntax characters (`&`, `<`, `>`, `"`, `'`) so an untrusted value (e.g. an
/// alert-ingest incident title, which arrives verbatim from the webhook payload of any
/// ingest-token holder) can be safely interpolated into an HTML email body without enabling
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

/// Incident-declared notification, sent to workspace owners. `severity_label` is the incident's
/// severity as a display string (e.g. `"SEV1"`), or `"unspecified"` if none was set. `link` is
/// the full URL to the incident's channel.
///
/// # Examples
/// ```
/// use rim_infra_services::email::templates::incident_declared;
/// let msg = incident_declared(
///     "owner@example.com",
///     "SEV1",
///     "disk full",
///     "https://rim.example.com/servers/s/channels/c",
/// );
/// assert!(msg.subject.contains("SEV1"));
/// assert!(msg.body_text.contains("disk full"));
/// ```
pub fn incident_declared(to: &str, severity_label: &str, title: &str, link: &str) -> EmailMessage {
    let subject = format!("[{severity_label}] incident declared: {title}");
    let body_text = format!(
        "A new incident has been declared in your workspace.\n\n\
         Severity: {severity_label}\n\
         Title: {title}\n\n\
         View the incident: {link}"
    );
    let severity_label_html = escape_html(severity_label);
    let title_html = escape_html(title);
    let body_html = Some(format!(
        "<p>A new incident has been declared in your workspace.</p>\
         <p><strong>Severity:</strong> {severity_label_html}<br>\
         <strong>Title:</strong> {title_html}</p>\
         <p><a href=\"{link}\">View the incident</a></p>"
    ));

    EmailMessage {
        to: to.to_string(),
        subject,
        body_text,
        body_html,
    }
}

/// Escalation-step notification email (§5, C1b), sent to each step's resolved target(s).
/// `link` is the incident's channel URL, or `None` if `RIM_PUBLIC_BASE_URL` is unconfigured (the
/// email is still sent — just without a clickable link, mirroring `incident_declared`'s
/// degrade-gracefully stance rather than skipping the send entirely, since an escalation page
/// with no link is still more useful than no page at all).
///
/// # Examples
/// ```
/// use rim_infra_services::email::templates::escalation_step;
/// let msg = escalation_step(
///     "oncall@example.com",
///     "SEV1",
///     "disk full",
///     Some("https://rim.example.com/servers/s/channels/c"),
///     2,
/// );
/// assert!(msg.subject.contains("step 2"));
/// assert!(msg.body_text.contains("disk full"));
/// ```
pub fn escalation_step(
    to: &str,
    severity_label: &str,
    title: &str,
    link: Option<&str>,
    step_number: i32,
) -> EmailMessage {
    let subject = format!("[{severity_label}] escalation step {step_number}: {title}");
    let link_line_text = link
        .map(|l| format!("\n\nView the incident: {l}"))
        .unwrap_or_default();
    let body_text = format!(
        "You've been paged by an incident escalation.\n\n\
         Severity: {severity_label}\n\
         Title: {title}\n\
         Escalation step: {step_number}{link_line_text}"
    );

    let severity_label_html = escape_html(severity_label);
    let title_html = escape_html(title);
    let link_line_html = link
        .map(|l| format!("<p><a href=\"{l}\">View the incident</a></p>"))
        .unwrap_or_default();
    let body_html = Some(format!(
        "<p>You've been paged by an incident escalation.</p>\
         <p><strong>Severity:</strong> {severity_label_html}<br>\
         <strong>Title:</strong> {title_html}<br>\
         <strong>Escalation step:</strong> {step_number}</p>\
         {link_line_html}"
    ));

    EmailMessage {
        to: to.to_string(),
        subject,
        body_text,
        body_html,
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
    fn incident_declared_includes_severity_title_and_link() {
        let msg = incident_declared(
            "owner@example.com",
            "SEV1",
            "disk full",
            "https://rim.example.com/servers/s/channels/c",
        );
        assert_eq!(msg.to, "owner@example.com");
        assert!(msg.subject.contains("SEV1"));
        assert!(msg.subject.contains("disk full"));
        assert!(msg.body_text.contains("SEV1"));
        assert!(msg.body_text.contains("disk full"));
        assert!(
            msg.body_text
                .contains("https://rim.example.com/servers/s/channels/c")
        );
        let html = msg.body_html.expect("html part expected");
        assert!(html.contains("SEV1"));
    }

    /// Regression guard: `title` (and `severity_label`) arrive verbatim from the alert-ingest
    /// webhook payload of any ingest-token holder — an untrusted source. The HTML part must
    /// escape them so a malicious title can't inject markup/scripts/links into a workspace
    /// owner's inbox. The plain-text part is intentionally left unescaped (it isn't rendered as
    /// HTML) — see `body_text` assertions below.
    #[test]
    fn incident_declared_escapes_html_in_title_and_severity() {
        let msg = incident_declared(
            "owner@example.com",
            "SEV1",
            "<script>alert(1)</script><a href=\"javascript:evil()\">click</a>",
            "https://rim.example.com/servers/s/channels/c",
        );
        let html = msg.body_html.expect("html part expected");
        assert!(!html.contains("<script>"));
        assert!(!html.contains("<a href=\"javascript:evil()\">"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(html.contains("&lt;a href=&quot;javascript:evil()&quot;&gt;click&lt;/a&gt;"));

        // Plain-text part is untouched by escaping (not HTML, not injected into a DOM).
        assert!(
            msg.body_text
                .contains("<script>alert(1)</script><a href=\"javascript:evil()\">click</a>")
        );
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

    #[test]
    fn escalation_step_includes_severity_title_step_and_link() {
        let msg = escalation_step(
            "oncall@example.com",
            "SEV1",
            "disk full",
            Some("https://rim.example.com/servers/s/channels/c"),
            2,
        );
        assert_eq!(msg.to, "oncall@example.com");
        assert!(msg.subject.contains("SEV1"));
        assert!(msg.subject.contains("step 2"));
        assert!(msg.body_text.contains("disk full"));
        assert!(
            msg.body_text
                .contains("https://rim.example.com/servers/s/channels/c")
        );
        let html = msg.body_html.expect("html part expected");
        assert!(html.contains("SEV1"));
        assert!(html.contains("2"));
    }

    #[test]
    fn escalation_step_without_link_omits_link_line_but_still_sends() {
        let msg = escalation_step("oncall@example.com", "SEV2", "db down", None, 1);
        assert!(!msg.body_text.contains("View the incident"));
        let html = msg.body_html.expect("html part expected");
        assert!(!html.contains("<a href"));
    }

    /// Regression guard mirroring `incident_declared_escapes_html_in_title_and_severity`: the
    /// incident title is untrusted (may originate from an alert-ingest webhook payload).
    #[test]
    fn escalation_step_escapes_html_in_title() {
        let msg = escalation_step(
            "oncall@example.com",
            "SEV1",
            "<script>alert(1)</script>",
            None,
            1,
        );
        let html = msg.body_html.expect("html part expected");
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
    }
}
