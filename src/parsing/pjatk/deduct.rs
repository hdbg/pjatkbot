use core::panic;
use std::str::FromStr;

use chrono::{DateTime, NaiveDateTime, Utc};

use crate::parsing::types::{Class, ClassKind, ClassPlace, Group, TimeRange};

use super::PjatkClass;

// pub fn deduct_faculty(part: &str) -> I18NPath {
//     match part.chars().nth(1).unwrap() {
//         'I' => "pjatk.faculties.computer_science",
//         'K' => "pjatk.faculties.japanese_culture",
//         'G' => "pjatk.faculties.new_media_art",
//         'A' => "pjatk.faculties.interior_architecture",
//         'Z' => "pjatk.faculties.management",
//         'L' => "pjatk.common.language_studies",
//         _ => panic!("unknown pjatk faculty"),
//     }
// }

pub fn deduct_kind(class: &PjatkClass) -> ClassKind {
    match class.kind.as_str() {
        "Wykład" | "Lektorat" => ClassKind::Lecture,
        "Ćwiczenia" => ClassKind::Seminar,
        "Projekt dyplomowy" => ClassKind::DiplomaThesis,
        name => panic!("can't deduct pjatk class kind '{}'", name),
    }
}

pub fn deduct_groups(class: &PjatkClass) -> Vec<Group> {
    let raw_groups = class.groups.split(",");
    // let mut normalized_groups = Vec::new();

    // const GROUPS_SUFFIXES: &[char] = &['c', 'w', 'l'];

    // 'next_group: for raw_group in raw_groups {
    //     let this_group_parts = raw_group.split(' ').rev();
    //     for single_group_part in this_group_parts {
    //         if single_group_part.ends_with(GROUPS_SUFFIXES) {
    //             let without_suffix = &single_group_part[..single_group_part.len() - 1];
    //             if without_suffix.chars().all(|c| char::is_digit(c, 10)) {
    //                 normalized_groups.push(single_group_part.trim().to_owned());
    //                 continue 'next_group;
    //             }
    //         }
    //     }

    //     panic!("can't deduct pjatk group from '{}'", raw_group);
    // }

    // return normalized_groups;
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
    }
}
pub fn multi(input: impl Iterator<Item = PjatkClass>) -> Vec<Class> {
    input.map(deduct_all).collect()
}
