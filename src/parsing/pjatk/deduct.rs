use core::panic;

use chrono::{NaiveDateTime, Utc};

use crate::parsing::types::{Class, ClassKind, ClassPlace, Group, TimeRange};

use super::PjatkClass;

pub fn deduct_kind(class: &PjatkClass) -> ClassKind {
    match class.kind.as_str() {
        "Wykład" | "Lektorat" => ClassKind::Lecture,
        "Ćwiczenia" | "Internet - ćwiczenia" => ClassKind::Seminar,
        "Projekt dyplomowy" => ClassKind::DiplomaThesis,
        name => panic!("can't deduct pjatk class kind '{}'", name),
    }
}

pub fn deduct_groups(class: &PjatkClass) -> Vec<Group> {
    let raw_groups = class.groups.split(",");
    raw_groups
        .map(|x| Group {
            code: x.trim().to_owned(),
        })
        .collect()
}

use chrono::TimeZone;
pub fn deduct_range(class: &PjatkClass) -> TimeRange {
    let date = chrono::NaiveDate::parse_from_str(&class.date, "%d.%m.%Y").unwrap();
    let begin_time = chrono::NaiveTime::parse_from_str(&class.from, "%H:%M:%S").unwrap();
    let end_time = chrono::NaiveTime::parse_from_str(&class.to, "%H:%M:%S").unwrap();

    let datetime_begin = NaiveDateTime::new(date, begin_time);
    let utc_begin = chrono_tz::Europe::Warsaw
        .from_local_datetime(&datetime_begin)
        .unwrap();

    let datetime_end = NaiveDateTime::new(date, end_time);
    let utc_end = chrono_tz::Europe::Warsaw
        .from_local_datetime(&datetime_end)
        .unwrap();
    TimeRange {
        start: utc_begin.with_timezone(&Utc),
        end: utc_end.with_timezone(&Utc),
    }
}

pub fn deduct_place(class: &PjatkClass) -> ClassPlace {
    if class.is_online {
        ClassPlace::Online
    } else {
        ClassPlace::OnSite {
            room: class.room.to_owned(),
        }
    }
}

pub fn deduct_all(item: PjatkClass) -> Class {
    // lol, order of call and moves do actually matter here
    // I wonder why rust can't understand corect order for itself
    Class {
        kind: deduct_kind(&item),
        range: deduct_range(&item),
        place: deduct_place(&item),
        groups: deduct_groups(&item),
        lecturer: item.lecturer,
        name: item.name,
        code: item.code,
        class_id: item.id.strip_suffix(";z").unwrap().to_owned(),
    }
}
pub fn multi(input: impl Iterator<Item = PjatkClass>) -> Vec<Class> {
    input.map(deduct_all).collect()
}
