use chrono::Utc;
use parsers::pjatk::{ParseRequest, Parser, PjatkClass};
use slog::o;

pub mod types {
    #[derive(Debug)]
    pub enum ClassKind {
        Lecture,
        Seminar,
    }

    use chrono::{DateTime, Utc};

    #[derive(Debug)]
    pub struct TimeRange {
        pub start: chrono::DateTime<Utc>,
        pub duration: chrono::TimeDelta,
    }

    type I18NPath = &'static str;
    #[derive(Debug)]

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
    #[derive(Debug)]

    pub enum Degree {
        Bachelor,
        Master,
        Doctoral,
    }
    #[derive(Debug)]
    pub enum Semester {
        Number(u8),
        Retake,
    }
    #[derive(Debug)]

    pub struct Group {
        pub code: String,
    }

    #[derive(Debug)]
    pub enum ClassPlace {
        Online,
        OnSite { room: String },
    }

    #[derive(Debug)]
    pub struct Class {
        pub name: String,
        pub code: String,
        pub kind: ClassKind,
        pub lecturer: String,
        pub range: TimeRange,
        pub place: ClassPlace,
        pub groups: Vec<Group>,
    }
}

pub mod parsers;

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
    println!("{:#?}", day);
}
