//! Languages of the web UI.
//!
//! The texts are Fluent messages in `server/locales`, compiled into the
//! binary. Every request gets a [`Lang`]: the one the user picked with
//! `?lang=...` (remembered in a cookie), else the first one the browser asks
//! for that the server has, else `--default-lang`.

use std::{convert::Infallible, fmt, sync::LazyLock};

use axum::{
    extract::{FromRequestParts, Request, State},
    http::{HeaderMap, HeaderValue, header, request::Parts},
    middleware::Next,
    response::Response,
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use clap::ValueEnum;
use fluent_bundle::{FluentArgs, FluentResource, FluentValue, concurrent::FluentBundle};

use crate::{error::PageError, state::AppState};

/// Cookie remembering the language picked with `?lang=...`, which is what
/// the language selector on the main page sends.
const LANG_COOKIE: &str = "lang";

/// Defines [`Lang`] from one line per language: its variant, its tag (which
/// also names its file in `server/locales`) and its name in itself.
macro_rules! languages {
    ($($variant:ident = $code:literal, $name:literal;)+) => {
        /// A language of the web UI.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
        pub enum Lang {
            $(#[value(name = $code)] $variant,)+
        }

        impl Lang {
            /// All the languages, in the order of the language selector.
            pub const ALL: &[Lang] = &[$(Lang::$variant),+];

            /// The language's tag, as in `<html lang="...">`.
            pub fn code(self) -> &'static str {
                match self {
                    $(Lang::$variant => $code,)+
                }
            }

            /// Name of the language, in the language itself.
            pub fn name(self) -> &'static str {
                match self {
                    $(Lang::$variant => $name,)+
                }
            }

            fn source(self) -> &'static str {
                match self {
                    $(Lang::$variant => include_str!(concat!("../locales/", $code, ".ftl")),)+
                }
            }
        }
    };
}

// Adding a language takes a line here and its translation of en.ftl.
languages! {
    En = "en", "English";
    Cs = "cs", "Čeština";
}

/// The first language, the one every other one is translated from.
impl Default for Lang {
    fn default() -> Self {
        Lang::ALL[0]
    }
}

impl Lang {
    /// The language with the primary subtag of `tag`, e.g. `en` for `en-GB`.
    pub fn from_tag(tag: &str) -> Option<Lang> {
        let primary = tag.trim().split(['-', '_']).next()?;
        Lang::ALL
            .iter()
            .copied()
            .find(|lang| lang.code().eq_ignore_ascii_case(primary))
    }

    /// The text of message `id`.
    pub fn t(self, id: &'static str) -> String {
        self.msg(id).to_string()
    }

    /// Message `id`, to be given arguments with [`Msg::arg`].
    pub fn msg<'a>(self, id: &'static str) -> Msg<'a> {
        Msg {
            lang: self,
            id,
            args: FluentArgs::new(),
        }
    }

    fn bundle(self) -> &'static FluentBundle<FluentResource> {
        static BUNDLES: LazyLock<Vec<FluentBundle<FluentResource>>> =
            LazyLock::new(|| Lang::ALL.iter().map(|lang| lang.load()).collect());
        &BUNDLES[self as usize]
    }

    /// Parses the Fluent file of the language. A broken one is a bug of the
    /// server, caught by the tests below.
    fn load(self) -> FluentBundle<FluentResource> {
        let resource = FluentResource::try_new(self.source().to_owned())
            .unwrap_or_else(|(_, errors)| panic!("{}.ftl: {errors:?}", self.code()));
        let mut bundle =
            FluentBundle::new_concurrent(vec![self.code().parse().expect("a valid language tag")]);
        // The Unicode isolation marks Fluent puts around variables are of no
        // use in left-to-right languages, and they would get into the way of
        // anybody searching the page.
        bundle.set_use_isolating(false);
        bundle
            .add_resource(resource)
            .unwrap_or_else(|errors| panic!("{}.ftl: {errors:?}", self.code()));
        bundle
    }
}

impl fmt::Display for Lang {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

/// The language of a request, as chosen by [`negotiate`].
impl<S: Send + Sync> FromRequestParts<S> for Lang {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Infallible> {
        Ok(parts.extensions.get::<Lang>().copied().unwrap_or_default())
    }
}

/// A message being given its arguments. It displays as plain text; [`html`]
/// gives the text of a message with markup.
///
/// [`html`]: Msg::html
pub struct Msg<'a> {
    lang: Lang,
    id: &'static str,
    args: FluentArgs<'a>,
}

impl<'a> Msg<'a> {
    pub fn arg(mut self, name: &'static str, value: impl Into<FluentValue<'a>>) -> Self {
        self.args.set(name, value);
        self
    }

    /// The text of a message that contains HTML markup, with its string
    /// arguments escaped, to be inserted into a page as is.
    pub fn html(self) -> String {
        let args = self
            .args
            .iter()
            .map(|(name, value)| match value {
                FluentValue::String(text) => (name.to_owned(), FluentValue::from(escape(text))),
                other => (name.to_owned(), other.clone()),
            })
            .collect();
        Msg { args, ..self }.to_string()
    }
}

impl fmt::Display for Msg<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bundle = self.lang.bundle();
        let Some(pattern) = bundle.get_message(self.id).and_then(|m| m.value()) else {
            tracing::error!(id = self.id, lang = %self.lang, "missing message");
            return f.write_str(self.id);
        };
        let mut errors = Vec::new();
        let text = bundle.format_pattern(pattern, Some(&self.args), &mut errors);
        if !errors.is_empty() {
            tracing::error!(id = self.id, lang = %self.lang, ?errors, "broken message");
        }
        f.write_str(&text)
    }
}

fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            c => escaped.push(c),
        }
    }
    escaped
}

/// Middleware that picks the language of every request and renders the error
/// pages in it.
pub async fn negotiate(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let picked = request.uri().query().and_then(|query| {
        url::form_urlencoded::parse(query.as_bytes())
            .find(|(name, _)| name == "lang")
            .and_then(|(_, value)| Lang::from_tag(&value))
    });
    let lang = picked
        .or_else(|| from_headers(request.headers()))
        .unwrap_or(state.cfg.default_lang);
    request.extensions_mut().insert(lang);

    let mut response = next.run(request).await;

    if let Some(error) = response.extensions_mut().remove::<PageError>() {
        response = error.render(lang);
    }
    if let Some(picked) = picked {
        let cookie = Cookie::build((LANG_COOKIE, picked.code()))
            .path("/")
            .same_site(SameSite::Lax)
            .max_age(time::Duration::days(365))
            .build();
        if let Ok(value) = HeaderValue::from_str(&cookie.to_string()) {
            response.headers_mut().append(header::SET_COOKIE, value);
        }
    }
    response
}

/// The language remembered in the cookie, else the first one in
/// `Accept-Language` the server has. Browsers list the languages from the most
/// preferred one, so the weights are not needed.
fn from_headers(headers: &HeaderMap) -> Option<Lang> {
    let cookie = CookieJar::from_headers(headers)
        .get(LANG_COOKIE)
        .and_then(|cookie| Lang::from_tag(cookie.value()));
    cookie.or_else(|| {
        headers
            .get(header::ACCEPT_LANGUAGE)?
            .to_str()
            .ok()?
            .split(',')
            .filter_map(|item| Lang::from_tag(item.split(';').next()?))
            .next()
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{Lang, from_headers};
    use axum::http::{HeaderMap, HeaderValue, header};

    /// Ids of the messages of `lang`: the lines that start with one.
    fn ids(lang: Lang) -> BTreeSet<&'static str> {
        lang.source()
            .lines()
            .filter_map(|line| line.split_once(" ="))
            .map(|(id, _)| id)
            .filter(|id| id.starts_with(|c: char| c.is_ascii_lowercase()))
            .collect()
    }

    #[test]
    fn every_language_has_every_message() {
        for &lang in Lang::ALL {
            // Panics on a syntax error.
            let _ = lang.bundle();
            assert_eq!(ids(lang), ids(Lang::default()), "messages of {lang}");
        }
    }

    #[test]
    fn arguments_of_markup_are_escaped() {
        let html = Lang::En.msg("your-answer-was").arg("label", "<b>").html();
        assert_eq!(html, "Your answer was <strong>&lt;b&gt;</strong>.");
        let text = Lang::En.msg("page-title").arg("number", 3).to_string();
        assert_eq!(text, "Quiz – question 3");
    }

    #[test]
    fn the_browser_preference_wins_over_the_default() {
        let headers = |cookie: Option<&str>, accept: &str| {
            let mut headers = HeaderMap::new();
            headers.insert(
                header::ACCEPT_LANGUAGE,
                HeaderValue::from_str(accept).unwrap(),
            );
            if let Some(cookie) = cookie {
                headers.insert(header::COOKIE, HeaderValue::from_str(cookie).unwrap());
            }
            headers
        };
        assert_eq!(
            from_headers(&headers(None, "de-DE,en;q=0.8,cs;q=0.5")),
            Some(Lang::En)
        );
        assert_eq!(from_headers(&headers(None, "cs-CZ,en")), Some(Lang::Cs));
        assert_eq!(from_headers(&headers(None, "de")), None);
        assert_eq!(
            from_headers(&headers(Some("lang=cs"), "en")),
            Some(Lang::Cs)
        );
    }
}
