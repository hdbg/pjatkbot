use std::{collections::HashMap, panic::Location, str::FromStr, sync::Arc};

use chrono::{DateTime, Datelike, NaiveDate, Utc};
use chrono_tz::Africa::Mogadishu;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, CONTENT_TYPE, USER_AGENT};
use scraper::{html, selectable::Selectable, Html, Selector};

use super::types::Class;

const GENERAL_SCHEDULE_ENDPOINT: &'static str = "https://planzajec.pjwstk.edu.pl/PlanOgolny3.aspx";

pub type BacktraceFix = std::backtrace::Backtrace;
#[derive(thiserror::Error, Debug)]
pub enum ParseError {
    #[error("HTTP request failed")]
    Http(#[from] reqwest::Error),
    #[error("PJATK has changed their webpage")]
    ParsingFailed(BacktraceFix),
}

#[derive(Debug)]
pub struct PjatkClass {
    pub id: String,
    pub name: String,
    pub code: String,
    pub kind: String,
    pub groups: String,
    pub lecturer: String,
    pub room: String,
    pub from: String,
    pub to: String,
    pub date: String,

    // temporary value used for resolving if class is online
    pub is_online: bool,
}

macro_rules! loc {
    () => {
        std::backtrace::Backtrace::force_capture()
    };
}

macro_rules! table_insert (
      ($table:ident, $([$key: expr, $lit:expr]),*) => {
            $($table.insert($key.to_string(), $lit.to_string());)*
        }
    );
macro_rules! hpe {
    ($e:expr) => {
        $e.ok_or(ParseError::ParsingFailed(loc!()))?
    };
}

mod deduct;

pub type ASPState = HashMap<String, String>;

pub struct Parser {
    client: reqwest::Client,
    state: ASPState,
}

impl Parser {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            state: HashMap::new(),
        }
    }

    fn is_reservation(html: &Html) -> bool {
        const RESERVATION_ID: &str = "#ctl06_TytulRezerwacjiLabel";
        let reservation_title_selector = scraper::Selector::parse(RESERVATION_ID).unwrap();

        html.select(&reservation_title_selector).count() > 0
    }
    fn parse_detail_html(
        class_id: &str,
        fragment: &str,
        code: &str,
    ) -> Result<Option<PjatkClass>, ParseError> {
        let document = scraper::Html::parse_fragment(fragment);
        if Self::is_reservation(&document) {
            return Ok(None);
        }

        const NAME_SELECTOR: &str = "#ctl06_NazwaPrzedmiotyLabel";
        const CODE_SELECTOR: &str = "#ctl06_KodPrzedmiotuLabel";
        const LECTURE_KIND: &str = "#ctl06_TypZajecLabel";
        const GROUPS_SELECTOR: &str = "#ctl06_GrupyLabel";
        const LECTURER: &str = "#ctl06_DydaktycyLabel";

        const ROOM_SELECTOR: &str = "#ctl06_SalaLabel";
        const DATE_SELECTOR: &str = "#ctl06_DataZajecLabel";

        const FROM_TIME_SELECTORS: &str = "#ctl06_GodzRozpLabel";
        const TO_TIME_SELECTORS: &str = "#ctl06_GodzZakonLabel";

        const ONLINE_COLOR_SUBSTR: &str = "background-color:#3AEB34;";

        macro_rules! parse_selector {
            ($document:ident, $selector:ident) => {
                hpe!($document
                    .select(&Selector::parse($selector).unwrap())
                    .next())
                .text()
                .collect::<String>()
                .trim()
                .to_owned()
            };
        }
        Ok(Some(PjatkClass {
            id: class_id.to_owned(),
            name: parse_selector!(document, NAME_SELECTOR),
            code: parse_selector!(document, CODE_SELECTOR),
            kind: parse_selector!(document, LECTURE_KIND),
            groups: parse_selector!(document, GROUPS_SELECTOR),
            lecturer: parse_selector!(document, LECTURER),
            room: parse_selector!(document, ROOM_SELECTOR),
            from: parse_selector!(document, FROM_TIME_SELECTORS),
            to: parse_selector!(document, TO_TIME_SELECTORS),
            is_online: code.contains(ONLINE_COLOR_SUBSTR),
            date: parse_selector!(document, DATE_SELECTOR),
        }))
    }

    async fn parse_detail(
        &mut self,
        class_id: &str,
        style: &str,
    ) -> Result<Option<PjatkClass>, ParseError> {
        let mut state = self.state.clone();

        table_insert!(
            state,
            [
                "RadScriptManager1",
                "RadToolTipManager1RTMPanel|RadToolTipManager1RTMPanel"
            ],
            ["__EVENTTARGET", "RadToolTipManager1RTMPanel"],
            ["__EVENTARGUMENT", "undefined"],
            [
                "RadToolTipManager1_ClientState",
                format!(r#"{{"AjaxTargetControl":"{class_id}","Value":"{class_id}"}}"#)
            ],
            ["RadToolTipManager2_ClientState", ""],
            ["__ASYNCPOST", "true"],
            ["DataPicker_ClientState", ""] ,
            ["RadScriptManager1_TSM", ";;System.Web.Extensions, Version=4.0.0.0, Culture=neutral, PublicKeyToken=31bf3856ad364e35:en-US:ceece802-cb39-4409-a6c9-bfa3b2c8bf10:ea597d4b:b25378d2;Telerik.Web.UI, Version=2018.1.117.40, Culture=neutral, PublicKeyToken=121fae78165ba3d4:en-US:3346c3e6-3c4c-4be3-94e3-1928d6a828a1:16e4e7cd:f7645509:ed16cbdc:88144a7a:33715776:24ee1bba:f46195d3:c128760b:874f8ea2:19620875:cda80b3:383e4ce8:1e771326:2003d0b8:aa288e2d:258f1c72:8674cba1:7c926187:b7778d6c:c08e9f8a:a51ee93e:59462f1:6d43f6d9:2bef5fcc:e06b58fd"]
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            "x-requested-with",
            HeaderValue::from_static("XMLHttpRequest"),
        );
        headers.insert("x-microsoftajax", HeaderValue::from_static("Delta=true"));
        headers.insert("user-agent", HeaderValue::from_static("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36"));
        headers.insert(
            "content-type",
            HeaderValue::from_static("application/x-www-form-urlencoded; charset=UTF-8"),
        );

        let fragment = self
            .client
            .post(GENERAL_SCHEDULE_ENDPOINT)
            .headers(headers)
            .form(&state)
            .send()
            .await?;

        let fragment_text = fragment.text().await?;

        // update state from fragment
        let state_line = hpe!(fragment_text.lines().last());

        let mut new_state = state_line.split('|');

        while let Some(next_state_id) = new_state.next() {
            if !next_state_id.starts_with("__") {
                continue;
            }

            if let Some(state_value) = new_state.next() {
                self.state
                    .insert(next_state_id.to_owned(), state_value.to_owned());
            }
        }

        let mut fragment_html: Vec<_> = fragment_text.split("\n").collect();

        // remove last and first lines
        fragment_html.remove(0);
        fragment_html.pop();

        let fragment_html = fragment_html.join("\n");

        Self::parse_detail_html(class_id, &fragment_html, style)
    }

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

    fn default_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-requested-with",
            HeaderValue::from_static("XMLHttpRequest"),
        );
        headers.insert("x-microsoftajax", HeaderValue::from_static("Delta=true"));
        headers.insert(USER_AGENT, HeaderValue::from_static("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36"));
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/x-www-form-urlencoded; charset=UTF-8"),
        );
        headers
    }

    fn prepare_date_update_state(&mut self, date: &NaiveDate) {
        let table = &mut self.state;
        table_insert!(
            table,
            ["RadScriptManager1", "RadAjaxPanel1Panel|DataPicker"],
            ["__EVENTTARGET", "DataPicker"],
            ["__EVENTARGUMENT", ""],
            ["DataPicker", date.to_string()],
            ["DataPicker$dateInput", date.to_string()],
            [
                "DataPicker_dateInput_ClientState",
                format!(
                    r#"{{"enabled":true,"emptyMessage":"","validationText":"{date}-00-00-00","valueAsString":"{date}-00-00-00","minDateStr":"1980-01-01-00-00-00","maxDateStr":"2099-12-31-00-00-00","lastSetTextBoxValue":"{date}"}}"#
                )
            ],
            ["DataPicker_ClientState", ""],
            ["__ASYNCPOST", "true"],
            ["RadAJAXControlID", "RadAjaxPanel1"], 
            ["RadScriptManager1_TSM", ";;System.Web.Extensions, Version=4.0.0.0, Culture=neutral, PublicKeyToken=31bf3856ad364e35:en-US:ceece802-cb39-4409-a6c9-bfa3b2c8bf10:ea597d4b:b25378d2;Telerik.Web.UI, Version=2018.1.117.40, Culture=neutral, PublicKeyToken=121fae78165ba3d4:en-US:3346c3e6-3c4c-4be3-94e3-1928d6a828a1:16e4e7cd:f7645509:ed16cbdc:88144a7a:33715776:24ee1bba:f46195d3:c128760b:874f8ea2:19620875:cda80b3:383e4ce8:1e771326:2003d0b8:aa288e2d:258f1c72:8674cba1:7c926187:b7778d6c:c08e9f8a:a51ee93e:59462f1:6d43f6d9:2bef5fcc:e06b58fd"]
        );
    }

    fn collect_class_ids(&mut self, document: &str) -> Result<Vec<(String, String)>, ParseError> {
        let body = scraper::Html::parse_document(&document);

        // main class parsing logic
        const CLASS_TABLE_SELECTOR: &str = "#ZajeciaTable > tbody";

        let class_table_selector = Selector::parse(CLASS_TABLE_SELECTOR).expect("static_selector");
        let Some(table) = body.select(&class_table_selector).next() else {
            return Ok(Vec::default());
        };

        const CLASS_ITEM_SELECTOR: &str = "td[id$=\";z\"]"; // every class id ends with ;z
        let class_item_selector = Selector::parse(CLASS_ITEM_SELECTOR).expect("static selector");

        let mut class_id_style_collected = Vec::new();

        for class in table.select(&class_item_selector) {
            class_id_style_collected.push((
                hpe!(class.attr("id").map(String::from)),
                class
                    .attr("style")
                    .map(String::from)
                    .unwrap_or("".to_owned()),
            ));
        }

        Ok(class_id_style_collected)
    }

    async fn parse_day_raw(&mut self, req: NaiveDate) -> Result<Vec<PjatkClass>, ParseError> {
        let mut classes = Vec::new();

        let mut resp = self
            .client
            .get(GENERAL_SCHEDULE_ENDPOINT)
            // .form(&self.state)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;

        self.update_state_from_html(&resp)?;
        self.prepare_date_update_state(&req);

        if req != Utc::now().date_naive() {
            resp = self
                .client
                .post(GENERAL_SCHEDULE_ENDPOINT)
                .headers(Self::default_headers())
                .form(&self.state)
                .send()
                .await?
                .error_for_status()?
                .text()
                .await?;
        }

        let class_id_style_collected = self.collect_class_ids(&resp)?;

        for class in class_id_style_collected.iter() {
            if let Some(class) = self.parse_detail(&class.0, &class.1).await? {
                classes.push(class);
            }
        }
        Ok(classes)
    }

    pub async fn parse_day(&mut self, req: NaiveDate) -> Result<Vec<Class>, ParseError> {
        let raw = self.parse_day_raw(req).await?;
        Ok(deduct::multi(raw.into_iter()))
    }
}

impl super::ScheduleParser for Parser {
    fn parse_day(
        &mut self,
        day: chrono::NaiveDate,
    ) -> impl std::future::Future<Output = eyre::Result<Vec<Class>>> {
        async move { self.parse_day(day).await.map_err(eyre::Report::from) }
    }

    const NAME: &'static str = "pjatk";
}
