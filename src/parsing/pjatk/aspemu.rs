use std::{backtrace::Backtrace, borrow::Cow, collections::HashMap};

use reqwest::{
    header::{HeaderMap, HeaderValue, CONTENT_TYPE, USER_AGENT},
    StatusCode,
};

use super::ParseError;

pub type MaybeString = Cow<'static, str>;

fn event_headers(is_delta: bool) -> HeaderMap {
    let mut headers = HeaderMap::new();

    let mut ins = |name, value| {
        headers.insert(name, HeaderValue::from_static(value));
    };

    ins("x-requested-with", "XMLHttpRequest");
    ins("user-agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36");
    ins(
        "content-type",
        "application/x-www-form-urlencoded; charset=UTF-8",
    );

    if is_delta {
        ins("x-microsoftajax", "Delta=true");
    }

    headers
}

pub type ASPState = HashMap<String, String>;

#[derive(Debug, Default, Clone)]
pub enum RequestKind {
    #[default]
    Initial,
    Event {
        target: MaybeString,
        argument: Option<MaybeString>,
    },
}

#[derive(Debug, derive_builder::Builder)]
#[builder(pattern = "mutable")]
pub struct ASPRequest {
    #[builder(default)]
    endpoint: Cow<'static, str>,
    kind: RequestKind,

    #[builder(default)]
    is_delta: bool,

    #[builder(setter(custom), default)]
    state_override: ASPState,
}

impl ASPRequestBuilder {
    pub fn state_override(
        &mut self,
        state_key: impl Into<String>,
        state_value: impl Into<String>,
    ) -> &mut Self {
        let state = match &mut self.state_override {
            Some(state) => state,
            state_opt @ None => {
                *state_opt = Some(ASPState::new());
                self.state_override.as_mut().unwrap()
            }
        };

        state.insert(state_key.into(), state_value.into());
        self
    }

    pub fn states_override(&mut self, states: ASPState) -> &mut Self {
        self.state_override = Some(states);
        self
    }
}

pub struct ASPResponse {
    pub code: StatusCode,
    pub body: Option<String>,
}

const EVENTTARGET_STATE: &str = "__EVENTTARGET";
const EVENTARG_STATE: &str = "__EVENTARGUMENT";

async fn process_resp<T: FnOnce(&mut String) -> Result<(), ParseError>>(
    resp: reqwest::Response,
    functor: T,
) -> Result<ASPResponse, ParseError> {
    let status = resp.status();
    let text = resp.text().await?;

    let mut body = Some(text).filter(|t| !t.is_empty());

    if let Some(ref mut body) = body {
        functor(body)?;
    }

    Ok(ASPResponse { code: status, body })
}

#[derive(derive_new::new)]
pub struct ASPEmulator {
    #[new(default)]
    state: ASPState,
    #[new(default)]
    client: reqwest::Client,

    #[new(into)]
    url_base: Cow<'static, str>,
}

impl ASPEmulator {
    fn update_state_from_html(&mut self, text: &str) -> Result<(), ParseError> {
        let body = scraper::Html::parse_document(&text);

        // setup element initial state
        let input_selector = scraper::Selector::parse("input").expect("selector is static");
        for state_elem in body.select(&input_selector) {
            if let Some(key) = state_elem.attr("id") {
                if !key.starts_with("__") {
                    continue;
                }
                if let Some(value) = state_elem.attr("value").map(String::from) {
                    self.state.insert(key.to_string(), value);
                }
            }
        }

        Ok(())
    }

    fn update_state_from_fragment(&mut self, fragment_line: &str) -> Result<(), ParseError> {
        let mut new_state = fragment_line.split('|');

        while let Some(next_state_id) = new_state.next() {
            if !next_state_id.starts_with("__") {
                continue;
            }

            if let Some(state_value) = new_state.next() {
                self.state
                    .insert(next_state_id.to_owned(), state_value.to_owned());
            }
        }

        Ok(())
    }

    pub async fn request(&mut self, req: ASPRequest) -> Result<ASPResponse, ParseError> {
        let url = self.url_base.clone().into_owned() + req.endpoint.as_ref();

        match req.kind {
            RequestKind::Initial => {
                let resp = self.client.get(url).send().await?;

                process_resp(resp, |text| self.update_state_from_html(text)).await
            }
            RequestKind::Event { target, argument } => {
                let mut state = self.state.clone();
                state.insert(EVENTTARGET_STATE.to_owned(), target.to_string());
                state.insert(
                    EVENTARG_STATE.to_owned(),
                    argument
                        .map(|arg| arg.to_string())
                        .unwrap_or_else(|| String::default()),
                );

                state.extend(req.state_override);

                let headers = event_headers(req.is_delta);

                let resp = self
                    .client
                    .post(url)
                    .headers(headers)
                    .form(&state)
                    .send()
                    .await?;

                process_resp(resp, |body| {
                    let lines = body.lines().skip(1);
                    let mut lines: Vec<String> = lines.map(String::from).collect();

                    let Some(fragment) = lines.pop() else {
                        return Err(ParseError::BodyAbrupted(Backtrace::capture()));
                    };

                    self.update_state_from_fragment(&fragment)?;

                    let full_body = lines.join("\n");

                    *body = full_body;

                    return Ok(());
                })
                .await
            }
        }
    }
}
