use std::{collections::HashMap, panic::Location, str::FromStr};

use chrono::{DateTime, Datelike, NaiveDate, Utc};
use chrono_tz::Africa::Mogadishu;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, CONTENT_TYPE, USER_AGENT};
use scraper::{html, selectable::Selectable, Html, Selector};

use super::types::Class;

const GENERAL_SCHEDULE_ENDPOINT: &'static str = "https://planzajec.pjwstk.edu.pl/PlanOgolny3.aspx";

#[derive(thiserror::Error, Debug)]
pub enum ParseError {
    #[error("HTTP request failed")]
    Http(#[from] reqwest::Error),
    #[error("PJATK has changed their webpage")]
    ParsingFailed(&'static Location<'static>),
}

#[derive(Debug)]
pub struct PjatkClass {
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
        Location::caller()
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

    fn modify_date(state: &mut ASPState, new_date: NaiveDate) {
        let date_value = new_date.to_string();
        println!("{}", date_value.to_string());

        state.insert("DataPicker".to_owned(), date_value.to_owned());
        state.insert("DataPicker$dateInput".to_owned(), date_value.to_owned());

        state.insert("DataPicker_dateInput_ClientState".to_owned(), format!(r#"{{"enabled":true,"emptyMessage":"","validationText":"{date_value}-00-00-00","valueAsString":"{date_value}-00-00-00","minDateStr":"1980-01-01-00-00-00","maxDateStr":"2099-12-31-00-00-00","lastSetTextBoxValue":"{date_value}"}}"#));

        let today_date = Utc::now().date_naive();
        state.insert("DataPicker_calendar_SD".to_owned(), "[]".to_owned());
        state.insert(
            "DataPicker_calendar_AD".to_owned(),
            format!(
                "[[1980,1,1],[2099,12,30],[{},{},{}]]",
                today_date.year(),
                today_date.month(),
                today_date.day()
            ),
        );
        println!("{:#?}", state);
    }
    fn create_initial_state(req: NaiveDate) -> ASPState {
        let mut state_override = HashMap::new();

        Self::modify_date(&mut state_override, req);
        state_override
    }

    fn is_reservation(html: &Html) -> bool {
        const RESERVATION_ID: &str = "#ctl06_TytulRezerwacjiLabel";
        let reservation_title_selector = scraper::Selector::parse(RESERVATION_ID).unwrap();

        html.select(&reservation_title_selector).count() > 0
    }
    fn parse_detail_html(fragment: &str, code: &str) -> Result<Option<PjatkClass>, ParseError> {
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
        key: &str,
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
                format!(r#"{{"AjaxTargetControl":"{key}","Value":"{key}"}}"#)
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

        Self::parse_detail_html(&fragment_html, style)
    }

    async fn parse_day_raw(&mut self, req: NaiveDate) -> Result<Vec<PjatkClass>, ParseError> {
        // self.state = Self::create_initial_state(req);
        let resp = self
            .client
            .get(GENERAL_SCHEDULE_ENDPOINT)
            // .form(&self.state)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        println!("{}", resp);

        let body = scraper::Html::parse_document(&resp);

        // println!("{}", body);

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
            // let key = hpe!(state_elem.attr("name")).to_string();

            // if key.starts_with("__") {
            // }
        }

        // Self::modify_date(&mut self.state, req);
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

        let table = &mut self.state;
        table_insert!(
            table,
            ["RadScriptManager1", "RadAjaxPanel1Panel|DataPicker"],
            ["__EVENTTARGET", "DataPicker"],
            ["__EVENTARGUMENT", ""],
            ["DataPicker", req.to_string()],
            ["DataPicker$dateInput", req.to_string()],
            [
                "DataPicker_dateInput_ClientState",
                format!(
                    r#"{{"enabled":true,"emptyMessage":"","validationText":"{req}-00-00-00","valueAsString":"{req}-00-00-00","minDateStr":"1980-01-01-00-00-00","maxDateStr":"2099-12-31-00-00-00","lastSetTextBoxValue":"{req}"}}"#
                )
            ],
            ["DataPicker_ClientState", ""],
            ["__ASYNCPOST", "true"],
            ["RadAJAXControlID", "RadAjaxPanel1"], 
            ["RadScriptManager1_TSM", ";;System.Web.Extensions, Version=4.0.0.0, Culture=neutral, PublicKeyToken=31bf3856ad364e35:en-US:ceece802-cb39-4409-a6c9-bfa3b2c8bf10:ea597d4b:b25378d2;Telerik.Web.UI, Version=2018.1.117.40, Culture=neutral, PublicKeyToken=121fae78165ba3d4:en-US:3346c3e6-3c4c-4be3-94e3-1928d6a828a1:16e4e7cd:f7645509:ed16cbdc:88144a7a:33715776:24ee1bba:f46195d3:c128760b:874f8ea2:19620875:cda80b3:383e4ce8:1e771326:2003d0b8:aa288e2d:258f1c72:8674cba1:7c926187:b7778d6c:c08e9f8a:a51ee93e:59462f1:6d43f6d9:2bef5fcc:e06b58fd"]
        );

        println!("{:#?}", self.state);

        let resp = self
            .client
            .post(GENERAL_SCHEDULE_ENDPOINT)
            .headers(headers)
            .form(&self.state)
            .send()
            .await?;

        println!("{:#?}", resp);

        let resp = resp.error_for_status()?.text().await?;
        println!("{}", resp);

        let body = scraper::Html::parse_document(&resp);

        // main class parsing logic
        const CLASS_TABLE_SELECTOR: &str = "#ZajeciaTable > tbody";

        let class_table_selector = Selector::parse(CLASS_TABLE_SELECTOR).expect("static_selector");
        let table = hpe!(body.select(&class_table_selector).next());

        let mut classes = Vec::new();
        const CLASS_ITEM_SELECTOR: &str = "td[id$=\";z\"]"; // every class id ends with ;z
        let class_item_selector = Selector::parse(CLASS_ITEM_SELECTOR).expect("static selector");

        for class in table.select(&class_item_selector) {
            println!("{:#?}", class);
            if let Some(class) = self
                .parse_detail(hpe!(class.attr("id")), class.attr("style").unwrap_or(""))
                .await?
            {
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
