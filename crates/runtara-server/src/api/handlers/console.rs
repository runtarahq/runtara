//! Where the console is. runtara-server does not serve it: the console
//! (runtarahq/runtara-frontend) is deployed on its own, as a static site or
//! its container image, and calls this server's API across origins
//! (`CORS_ALLOWED_ORIGINS`).

/// The console's OAuth landing page (`oauth-complete.html`), where the OAuth
/// callback sends the consent popup.
///
/// `RUNTARA_UI_PUBLIC_URL` names where the console is served, e.g.
/// `https://app.example.com/`. `None` when it is unset: the callback then
/// answers in plain text.
pub fn oauth_complete_url() -> Result<Option<url::Url>, String> {
    oauth_complete_url_from(std::env::var("RUNTARA_UI_PUBLIC_URL").ok().as_deref())
}

fn oauth_complete_url_from(ui_public_url: Option<&str>) -> Result<Option<url::Url>, String> {
    let Some(console) = ui_public_url.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return Ok(None);
    };
    let console = url::Url::parse(&format!("{}/", console.trim_end_matches('/')))
        .map_err(|e| format!("the console URL '{console}' is not a URL: {e}"))?;
    if !matches!(console.scheme(), "http" | "https") || console.query().is_some() {
        return Err(format!(
            "the console URL '{console}' must be an http(s) URL without a query"
        ));
    }
    console
        .join("oauth-complete.html")
        .map(Some)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oauth_landing_page_is_under_the_configured_console() {
        let url = oauth_complete_url_from(Some("https://app.example.com"));
        assert_eq!(
            url.unwrap().unwrap().as_str(),
            "https://app.example.com/oauth-complete.html"
        );
        let url = oauth_complete_url_from(Some("https://app.example.com/console/"));
        assert_eq!(
            url.unwrap().unwrap().as_str(),
            "https://app.example.com/console/oauth-complete.html"
        );
    }

    #[test]
    fn no_console_url_means_no_landing_page() {
        assert_eq!(oauth_complete_url_from(None).unwrap(), None);
        assert_eq!(oauth_complete_url_from(Some("  ")).unwrap(), None);
    }

    #[test]
    fn oauth_landing_page_refuses_a_console_url_it_cannot_redirect_to() {
        assert!(oauth_complete_url_from(Some("app.example.com")).is_err());
        assert!(oauth_complete_url_from(Some("javascript:alert(1)")).is_err());
        assert!(oauth_complete_url_from(Some("https://app.example.com/?x=1")).is_err());
    }
}
