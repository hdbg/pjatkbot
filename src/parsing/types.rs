#[derive(
    Debug,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    Clone,
    PartialEq,
    Eq,
    strum::Display,
    strum::IntoStaticStr,
    strum::EnumString,
)]
pub enum ClassKind {
    #[strum(serialize = "lecture")]
    Lecture,
    #[strum(serialize = "seminar")]
    Seminar,
    #[strum(serialize = "diploma_thesis")]
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
#[serde(transparent)]
pub struct Group {
    pub code: String,
}

#[derive(Debug, Hash, serde::Serialize, serde::Deserialize, Clone, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum ClassPlace {
    Online,
    OnSite { room: String },
}

#[derive(Debug, Hash, serde::Serialize, serde::Deserialize, Clone, PartialEq, Eq)]
pub struct Class {
    pub class_id: String,
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
