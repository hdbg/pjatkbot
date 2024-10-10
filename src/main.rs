use chrono::Utc;
use parsers::pjatk::{ParseRequest, Parser, PjatkClass};
use slog::o;

pub mod parsers {

    pub trait IntoLocalized {
        fn localized(&self, locale: &str) -> &str;
    }
    pub enum ClassKind {
        Lecture,
        Seminar,
    }

    use chrono::{DateTime, Utc};
    pub struct TimeRange {
        start: chrono::DateTime<Utc>,
        duration: chrono::TimeDelta,
    }

    type I18NPath = &'static str;

    pub enum StudyMode {
        Online,
        OnSite,
        PartTime,
    }
    pub enum Language {
        English,
        Polish,
    }
    pub struct UniversityInfo {
        name: &'static str,
        branches: &'static [I18NPath],
    }

    pub struct ScheduleRequest {
        day: DateTime<Utc>,
        branch: &'static str,
        study_mode: &'static [StudyMode],
    }

    pub enum Degree {
        Bachelor,
        Master,
        Doctoral,
    }

    pub enum Semester {
        Number(u8),
        Retake,
    }
    pub struct Group {
        code: String,
    }

    pub enum ClassPlace {
        Online,
        OnSite { room: String },
    }

    pub struct Class {
        name: String,
        code: Option<String>,
        kind: ClassKind,
        lecturer: String,
        range: TimeRange,
        place: ClassPlace,

        group: Vec<Group>,

        semester: Semester,
        faculty: I18NPath,
        degree: Degree,
    }

    pub mod pjatk {
        use std::{collections::HashMap, panic::Location, str::FromStr};

        use chrono::{DateTime, Datelike, Utc};
        use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
        use scraper::{html, selectable::Selectable, Html, Selector};

        use super::{Class, ClassKind, TimeRange};

        const GENERAL_SCHEDULE_ENDPOINT: &'static str =
            "https://planzajec.pjwstk.edu.pl/PlanOgolny3.aspx";

        #[derive(thiserror::Error, Debug)]
        pub enum ParseError {
            #[error("HTTP request failed")]
            Http(#[from] reqwest::Error),
            #[error("PJATK has changed their webpage")]
            ParsingFailed(&'static Location<'static>),
        }

        pub struct ParseRequest {
            pub date: DateTime<Utc>,
        }
        pub struct Parser {
            client: reqwest::Client,
            state: HashMap<String, String>,
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

        mod deduct {
            use core::panic;

            use crate::parsers::{Class, ClassKind, I18NPath};

            use super::PjatkClass;

            pub fn deduct_faculty(part: &str) -> I18NPath {
                match part.chars().nth(1).unwrap() {
                    'I' => "pjatk.faculties.computer_science",
                    'K' => "pjatk.faculties.japanese_culture",
                    'G' => "pjatk.faculties.new_media_art",
                    'A' => "pjatk.faculties.interior_architecture",
                    'Z' => "pjatk.faculties.management",
                    'L' => "pjatk.common.language_studies",
                    _ => panic!("unknown pjatk faculty"),
                }
            }

            pub fn deduct_kind(class: &PjatkClass) -> ClassKind {
                match class.kind.as_str() {
                    "Wykład" => ClassKind::Lecture,
                    "Ćwiczenia" => ClassKind::Seminar,
                    name => panic!("can't deduct pjatk class kind '{}'", name),
                }
            }

            pub fn deduct_groups(class: &PjatkClass) -> Vec<Group> {
                let raw_groups = class.groups.split(",");
                let mut normalized_groups = Vec::new();

                const GROUPS_SUFFIXES: &[char] = &['c', 'w', 'l'];

                'next_group: for raw_group in raw_groups {
                    let this_group_parts = raw_group.split(' ').rev();
                    for single_group_part in this_group_parts {
                        if single_group_part.ends_with(GROUPS_SUFFIXES) {
                            let without_suffix = &single_group_part[..single_group_part.len() - 1];
                            if without_suffix.chars().all(|c| char::is_digit(c, 10)) {
                                normalized_groups.push(single_group_part.trim().to_owned());
                                continue 'next_group;
                            }
                        }
                    }

                    panic!("can't deduct pjatk group from '{}'", raw_group);
                }

                return normalized_groups;
            }
            pub fn multi(input: impl Iterator<Item = PjatkClass>) -> Vec<Class> {}
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

            // temporary value used for resolving if class is online
            pub is_online: bool,
        }

        impl Parser {
            pub fn new() -> Self {
                Self {
                    client: reqwest::Client::new(),
                    state: HashMap::new(),
                }
            }
            fn create_initial_state(req: ParseRequest) -> HashMap<String, String> {
                let mut state_override = HashMap::new();

                let date_value = req.date.date_naive().to_string();

                state_override.insert("DataPicker".to_owned(), date_value.to_owned());
                state_override.insert("DataPicker$dateInput".to_owned(), date_value.to_owned());

                state_override.insert("DataPicker_dateInput_ClientState".to_owned(), format!(r#"{{"enabled":true,"emptyMessage":"","validationText":"{date_value}-00-00-00","valueAsString":"{date_value}-00-00-00","minDateStr":"1980-01-01-00-00-00","maxDateStr":"2099-12-31-00-00-00","lastSetTextBoxValue":"{date_value}"}}"#));

                let today_date = Utc::now().date_naive();
                state_override.insert(
                    "DataPicker_calendar_AD".to_owned(),
                    format!(
                        "[[1980,1,1],[2099,12,30],[{},{},{}]]",
                        today_date.year(),
                        today_date.month(),
                        today_date.day()
                    ),
                );
                state_override
            }

            fn is_reservation(html: &Html) -> bool {
                const RESERVATION_ID: &str = "#ctl06_TytulRezerwacjiLabel";
                let reservation_title_selector = scraper::Selector::parse(RESERVATION_ID).unwrap();

                html.select(&reservation_title_selector).count() > 0
            }
            fn parse_detail_html(
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
                    ["DataPicker_ClientState", ""],
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

            async fn parse_day_raw(
                &mut self,
                req: ParseRequest,
            ) -> Result<Vec<PjatkClass>, ParseError> {
                self.state = Self::create_initial_state(req);
                let initial_data = self
                    .client
                    .get(GENERAL_SCHEDULE_ENDPOINT)
                    .form(&self.state)
                    .send()
                    .await?
                    .error_for_status()?
                    .text()
                    .await?;

                let initial_body = scraper::Html::parse_document(&initial_data);

                // setup element initial state
                let input_selector = scraper::Selector::parse("input").expect("selector is static");
                for state_elem in initial_body.select(&input_selector) {
                    let key = hpe!(state_elem.attr("id")).to_string();
                    if !self.state.contains_key(&key) && key.starts_with("__") {
                        self.state
                            .insert(key.to_string(), hpe!(state_elem.attr("value")).to_string());
                    }
                }

                // main class parsing logic
                const CLASS_TABLE_SELECTOR: &str = "#ZajeciaTable > tbody";

                let class_table_selector =
                    Selector::parse(CLASS_TABLE_SELECTOR).expect("static_selector");
                let table = hpe!(initial_body.select(&class_table_selector).next());

                let mut classes = Vec::new();
                const CLASS_ITEM_SELECTOR: &str = "td[id$=\";z\"]"; // every class id ends with ;z
                let class_item_selector =
                    Selector::parse(CLASS_ITEM_SELECTOR).expect("static selector");

                for class in table.select(&class_item_selector) {
                    if let Some(class) = self
                        .parse_detail(hpe!(class.attr("id")), hpe(class.attr("style")))
                        .await?
                    {
                        classes.push(class);
                    }
                }
                Ok(classes)
            }

            pub async fn parse_day(&mut self, req: ParseRequest) -> Result<Vec<Class>, ParseError> {
                let raw = self.parse_day_raw(req).await?;
            }
        }
    }
}

#[tokio::main]
async fn main() {
    use slog::info;
    use sloggers::terminal::{Destination, TerminalLoggerBuilder};
    use sloggers::types::Severity;
    use sloggers::Build;

    let mut builder = TerminalLoggerBuilder::new();
    builder.level(Severity::Debug);
    builder.format(sloggers::types::Format::Full);
    builder.destination(Destination::Stdout);

    let logger = builder.build().unwrap();
    info!(logger, "boot");

    let mut pjatk = Parser::new();

    let mut day = pjatk
        .parse_day(ParseRequest { date: Utc::now() })
        .await
        .unwrap();
    day.retain(|x| x.groups.contains("1w"));
    println!("{:#?}", day);
}
