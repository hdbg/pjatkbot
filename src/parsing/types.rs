#[derive(Debug, Hash, serde::Serialize, serde::Deserialize, Clone, PartialEq, Eq)]
pub enum ClassKind {
    Lecture,
    Seminar,
    DiplomaThesis,
}

use chrono::{DateTime, Utc};

use bson::serde_helpers::chrono_datetime_as_bson_datetime;

use crate::db::Model;
#[derive(Debug, Hash, serde::Serialize, serde::Deserialize, Clone, PartialEq, Eq)]
pub struct TimeRange {
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub start: chrono::DateTime<Utc>,
    #[serde(with = "chrono_datetime_as_bson_datetime")]
    pub end: chrono::DateTime<Utc>,
}

#[derive(Debug, Hash, serde::Serialize, serde::Deserialize, Clone, PartialEq, Eq)]
pub enum StudyMode {
    Online,
    OnSite,
    PartTime,
}
pub enum Language {
    English,
    Polish,
}
#[derive(Debug, Hash, serde::Serialize, serde::Deserialize, Clone, PartialEq, Eq)]
pub enum Degree {
    Bachelor,
    Master,
    Doctoral,
}
#[derive(Debug, Hash, serde::Serialize, serde::Deserialize, Clone, PartialEq, Eq)]
pub enum Semester {
    Number(u8),
    Retake,
}
#[derive(Debug, Hash, serde::Serialize, serde::Deserialize, Clone, PartialEq, Eq)]
pub struct Group {
    pub code: String,
}

#[derive(Debug, Hash, serde::Serialize, serde::Deserialize, Clone, PartialEq, Eq)]
pub enum ClassPlace {
    Online,
    OnSite { room: String },
}

#[derive(Debug, Hash, serde::Serialize, serde::Deserialize, Clone, PartialEq, Eq)]
pub struct Class {
    pub name: String,
    pub code: String,
    pub kind: ClassKind,
    pub lecturer: String,
    pub range: TimeRange,
    pub place: ClassPlace,
    pub groups: Vec<Group>,
}

impl Model for Class {
    const COLLECTION_NAME: &'static str = "classes";
}
