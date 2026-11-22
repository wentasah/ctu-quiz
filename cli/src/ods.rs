//! The points of a room as an OpenDocument spreadsheet, with a second sheet
//! of how long the students took to answer.
//!
//! The workbook is built with `spreadsheet_ods`. Color scales and data bars
//! are not part of ODF, though, but a LibreOffice extension stored as
//! `<calcext:conditional-formats>` at the end of each `<table:table>`, and
//! `spreadsheet_ods` cannot create them. So the workbook is written to memory
//! first, and the conditional formats are put into its `content.xml` before
//! the archive is written to the file.

use std::{
    collections::BTreeMap,
    io::{Cursor, Read, Write},
    path::Path,
};

use anyhow::{Context, Result};
use spreadsheet_ods::{
    CellRange, CellStyleRef, Length, Sheet, WorkBook, format,
    formula::frangeref,
    style::{
        CellStyle,
        units::{Angle, CellAlignVertical, TextAlign},
    },
};
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

use crate::report::{Room, room};
use quiz_common::api::Results;

/// How a point of a color scale, or an end of a data bar, is computed.
#[derive(Clone, Copy)]
enum Threshold {
    Minimum,
    Maximum,
    AutoMinimum,
    AutoMaximum,
    Percentile(f64),
}

impl Threshold {
    fn type_and_value(self) -> (&'static str, f64) {
        match self {
            Threshold::Minimum => ("minimum", 0.0),
            Threshold::Maximum => ("maximum", 0.0),
            Threshold::AutoMinimum => ("auto-minimum", 0.0),
            Threshold::AutoMaximum => ("auto-maximum", 0.0),
            Threshold::Percentile(value) => ("percentile", value),
        }
    }
}

/// A conditional format of LibreOffice.
enum CondFormat {
    /// Two or three points, each with its color.
    ColorScale(Vec<(Threshold, &'static str)>),
    DataBar {
        positive_color: &'static str,
        negative_color: &'static str,
        min: Threshold,
        max: Threshold,
    },
}

/// A conditional format and the cells it applies to.
struct CondRule {
    range: CellRange,
    format: CondFormat,
}

const RED: &str = "#f8696b";
const YELLOW: &str = "#ffeb84";
const GREEN: &str = "#63be7b";
const BLUE: &str = "#638ec6";

/// Points: the worst red, the best green.
fn points_scale() -> CondFormat {
    CondFormat::ColorScale(vec![
        (Threshold::Minimum, RED),
        (Threshold::Percentile(50.0), YELLOW),
        (Threshold::Maximum, GREEN),
    ])
}

/// Time to answer: the fastest green, the slowest red.
fn seconds_scale() -> CondFormat {
    CondFormat::ColorScale(vec![
        (Threshold::Minimum, GREEN),
        (Threshold::Percentile(50.0), YELLOW),
        (Threshold::Maximum, RED),
    ])
}

const POINTS_SHEET: &str = "Points";
const SECONDS_SHEET: &str = "Time";

/// The spreadsheet of the room, next to the room CSV at `csv`: `room_….ods`.
pub fn ods_path(csv: &Path) -> std::path::PathBuf {
    csv.with_extension("ods")
}

/// Writes the points of every student in the room to the sheet `Points`,
/// one column per question and the total, and the seconds every student
/// took to answer to the sheet `Time`. The students are in the order of the
/// room CSV.
pub fn write_room_ods(path: &Path, questions: &[Results]) -> Result<()> {
    let room = room(questions);
    let (mut book, rules) = workbook(&room);
    let ods = write_with_rules(&mut book, &rules)
        .with_context(|| format!("cannot write {}", path.display()))?;
    std::fs::write(path, ods).with_context(|| format!("cannot write {}", path.display()))
}

/// Styles shared by both sheets.
struct Styles {
    header: CellStyleRef,
    rotated: CellStyleRef,
    seconds: CellStyleRef,
}

/// The workbook of `room` and the conditional formats of each of its
/// sheets, by the index of the sheet.
fn workbook(room: &Room) -> (WorkBook, BTreeMap<usize, Vec<CondRule>>) {
    let mut book = WorkBook::new_empty();

    let mut header = CellStyle::new_empty();
    header.set_name("header");
    header.set_font_bold();
    header.set_vertical_align(CellAlignVertical::Bottom);
    let header = book.add_cellstyle(header);

    // The file names of the questions read from bottom to top, so that the
    // columns can stay narrow.
    let mut rotated = CellStyle::new_empty();
    rotated.set_name("header_rotated");
    rotated.set_font_bold();
    rotated.set_rotation_angle(Angle::Deg(90.0));
    rotated.set_vertical_align(CellAlignVertical::Bottom);
    rotated.set_text_align(TextAlign::Center);
    let rotated = book.add_cellstyle(rotated);

    let tenths = book.add_number_format(format::create_number_format_fixed("tenths", 1, false));
    let seconds = book.add_cellstyle(CellStyle::new("seconds", &tenths));

    let styles = Styles {
        header,
        rotated,
        seconds,
    };
    let mut rules = BTreeMap::new();

    let (sheet, sheet_rules) = points_sheet(room, &styles);
    rules.insert(book.num_sheets(), sheet_rules);
    book.push_sheet(sheet);

    let (sheet, sheet_rules) = seconds_sheet(room, &styles);
    rules.insert(book.num_sheets(), sheet_rules);
    book.push_sheet(sheet);

    (book, rules)
}

/// The first column of the questions; login and name come before them.
const FIRST: u32 = 2;

/// A sheet with the header row and the login and name of every student.
fn sheet(name: &str, room: &Room, styles: &Styles) -> Sheet {
    let mut sheet = Sheet::new(name);
    sheet.set_styled_value(0, 0, "login", &styles.header);
    sheet.set_styled_value(0, 1, "name", &styles.header);
    for (column, title) in (FIRST..).zip(&room.columns) {
        sheet.set_styled_value(0, column, title.as_str(), &styles.rotated);
        sheet.set_col_width(column, Length::Cm(1.3));
    }
    for (row, student) in (1..).zip(&room.students) {
        sheet.set_value(row, 0, student.login);
        sheet.set_value(row, 1, student.name);
    }
    sheet.set_col_width(0, Length::Cm(2.5));
    sheet.set_col_width(1, Length::Cm(5.0));
    // Tall enough for the longest rotated file name.
    let longest = room
        .columns
        .iter()
        .map(|title| title.chars().count())
        .max()
        .unwrap_or(0);
    sheet.set_row_height(0, Length::Cm((0.2 * longest as f64 + 0.4).clamp(1.0, 10.0)));
    // The header and the students stay in sight while scrolling.
    sheet.split_row_header(0);
    sheet.split_col_header(1);
    sheet
}

/// One color scale per question, so that each is colored by its own range
/// of values; none without students, where there are no cells to color.
fn column_scales(room: &Room, scale: fn() -> CondFormat) -> Vec<CondRule> {
    let last = room.students.len() as u32;
    if last == 0 {
        return Vec::new();
    }
    (FIRST..)
        .take(room.columns.len())
        .map(|column| CondRule {
            range: CellRange::local(1, column, last, column),
            format: scale(),
        })
        .collect()
}

fn points_sheet(room: &Room, styles: &Styles) -> (Sheet, Vec<CondRule>) {
    let mut sheet = sheet(POINTS_SHEET, room, styles);
    let questions = room.columns.len() as u32;
    let total = FIRST + questions;
    sheet.set_styled_value(0, total, "total", &styles.header);
    sheet.set_col_width(total, Length::Cm(2.5));
    for (row, student) in (1..).zip(&room.students) {
        for (column, points) in (FIRST..).zip(&student.points) {
            if let Some(points) = points {
                sheet.set_value(row, column, *points);
            }
        }
        // A formula, so that points changed by hand are added up too; the
        // value is there for applications that do not recalculate.
        let sum = frangeref(row, FIRST, row, total - 1);
        sheet.set_formula(row, total, format!("of:=SUM({sum})"));
        sheet.set_value(row, total, student.total());
    }

    let mut rules = column_scales(room, points_scale);
    let last = room.students.len() as u32;
    if last > 0 {
        rules.push(CondRule {
            range: CellRange::local(1, total, last, total),
            format: CondFormat::DataBar {
                positive_color: BLUE,
                negative_color: "#ff0000",
                min: Threshold::AutoMinimum,
                max: Threshold::AutoMaximum,
            },
        });
    }
    (sheet, rules)
}

fn seconds_sheet(room: &Room, styles: &Styles) -> (Sheet, Vec<CondRule>) {
    let mut sheet = sheet(SECONDS_SHEET, room, styles);
    for (row, student) in (1..).zip(&room.students) {
        for (column, seconds) in (FIRST..).zip(&student.seconds) {
            if let Some(seconds) = seconds {
                sheet.set_styled_value(row, column, *seconds, &styles.seconds);
            }
        }
    }
    (sheet, column_scales(room, seconds_scale))
}

/// `range` on the sheet `sheet`, the way a target range address of a
/// conditional format is written: `'Time'.C2:'Time'.C9`.
fn range_address(sheet: &str, range: &CellRange) -> String {
    let sheet = format!("'{}'", sheet.replace('\'', "''"));
    range
        .to_formula()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(':')
        .map(|cell| format!("{sheet}{cell}"))
        .collect::<Vec<_>>()
        .join(":")
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// The `<calcext:conditional-formats>` element with `rules`.
fn rules_xml(sheet: &str, rules: &[CondRule]) -> String {
    let mut xml = String::from("<calcext:conditional-formats>");
    for rule in rules {
        let address = xml_escape(&range_address(sheet, &rule.range));
        xml += &format!(r#"<calcext:conditional-format calcext:target-range-address="{address}">"#);
        match &rule.format {
            CondFormat::ColorScale(points) => {
                xml += "<calcext:color-scale>";
                for (threshold, color) in points {
                    let (kind, value) = threshold.type_and_value();
                    xml += &format!(
                        r#"<calcext:color-scale-entry calcext:value="{value}" calcext:type="{kind}" calcext:color="{color}"/>"#
                    );
                }
                xml += "</calcext:color-scale>";
            }
            CondFormat::DataBar {
                positive_color,
                negative_color,
                min,
                max,
            } => {
                xml += &format!(
                    r##"<calcext:data-bar calcext:positive-color="{positive_color}" calcext:negative-color="{negative_color}" calcext:axis-position="automatic" calcext:axis-color="#000000" calcext:gradient="true" calcext:show-value="true" calcext:min-length="0" calcext:max-length="100">"##
                );
                for threshold in [min, max] {
                    let (kind, value) = threshold.type_and_value();
                    xml += &format!(
                        r#"<calcext:formatting-entry calcext:value="{value}" calcext:type="{kind}"/>"#
                    );
                }
                xml += "</calcext:data-bar>";
            }
        }
        xml += "</calcext:conditional-format>";
    }
    xml += "</calcext:conditional-formats>";
    xml
}

/// The ODS file of `book`, with the conditional formats `rules` of its
/// sheets, by the index of the sheet.
fn write_with_rules(
    book: &mut WorkBook,
    rules: &BTreeMap<usize, Vec<CondRule>>,
) -> Result<Vec<u8>> {
    let names: Vec<String> = (0..book.num_sheets())
        .map(|index| book.sheet(index).name().to_owned())
        .collect();
    let ods = spreadsheet_ods::write_ods_buf(book, Vec::new())?;

    let mut archive = ZipArchive::new(Cursor::new(ods))?;
    let mut content = String::new();
    archive
        .by_name("content.xml")?
        .read_to_string(&mut content)?;

    // The conditional formats are the last child of `<table:table>`, so
    // they go right before the closing tag of each sheet.
    const END: &str = "</table:table>";
    let mut patched = String::with_capacity(content.len() + 4096);
    let mut rest = content.as_str();
    let mut index = 0;
    while let Some(position) = rest.find(END) {
        patched.push_str(&rest[..position]);
        if let Some(rules) = rules.get(&index).filter(|rules| !rules.is_empty()) {
            patched.push_str(&rules_xml(&names[index], rules));
        }
        patched.push_str(END);
        rest = &rest[position + END.len()..];
        index += 1;
    }
    patched.push_str(rest);

    // Everything else is copied as it is, which keeps `mimetype` first and
    // uncompressed, as ODF requires.
    let mut out = ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..archive.len() {
        let entry = archive.by_index_raw(index)?;
        if entry.name()? == "content.xml" {
            drop(entry);
            out.start_file(
                "content.xml",
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
            )?;
            out.write_all(patched.as_bytes())?;
        } else {
            out.raw_copy_file(entry)?;
        }
    }
    Ok(out.finish()?.into_inner())
}

#[cfg(test)]
mod tests {
    use chrono::{TimeDelta, Utc};
    use quiz_common::api::{QuestionInfo, ResultRow, Results, StudentView};
    use spreadsheet_ods::Value;

    use super::*;

    /// A closed question of `source_name`, answered by `rows` of login,
    /// name, points and seconds.
    fn results(source_name: &str, manual: bool, rows: &[(&str, &str, f64, i64)]) -> Results {
        let asked = Utc::now();
        Results {
            question: QuestionInfo {
                id: "id".to_owned(),
                view: StudentView::default(),
                code: "428173".to_owned(),
                title: None,
                source_name: source_name.to_owned(),
                answer_url: String::new(),
                display_url: String::new(),
                presented: false,
                answers: Vec::new(),
                open: false,
                answer_count: rows.len(),
                people: rows.len(),
                counts: Vec::new(),
                teacher: "sojkam1".to_owned(),
                created_at: asked,
                closed_at: Some(asked),
                revealed: false,
                manual,
            },
            rows: rows
                .iter()
                .map(|&(login, name, points, seconds)| ResultRow {
                    login: login.to_owned(),
                    name: name.to_owned(),
                    points: Some(points),
                    answer: "A".to_owned(),
                    answer_text: String::new(),
                    answered_at: asked + TimeDelta::seconds(seconds),
                })
                .collect(),
        }
    }

    fn questions() -> Vec<Results> {
        vec![
            results(
                "pipes.md",
                false,
                &[("svobodpe", "Petra", 1.0, 12), ("novakja1", "Jan", 0.5, 30)],
            ),
            results("signals.md", false, &[("novakja1", "Jan", -1.0, 7)]),
            results("pipes.md", false, &[("svobodpe", "Petra", 0.0, 45)]),
            results("manual", true, &[("novakja1", "Jan", 2.0, 0)]),
        ]
    }

    fn write(name: &str) -> std::path::PathBuf {
        // Kept, to be looked at in a spreadsheet application, when asked for.
        let path = match std::env::var_os("QUIZ_ODS_SAMPLE") {
            Some(path) => std::path::PathBuf::from(path),
            None => std::env::temp_dir().join(format!("quiz-{}-{name}.ods", std::process::id())),
        };
        write_room_ods(&path, &questions()).unwrap();
        path
    }

    #[test]
    fn the_sheets_have_the_points_and_the_seconds_of_the_room_csv() {
        let path = write("sheets");
        let book = spreadsheet_ods::read_ods(&path).unwrap();
        let content = read_content(&path);
        if std::env::var_os("QUIZ_ODS_SAMPLE").is_none() {
            std::fs::remove_file(&path).unwrap();
        }

        let text = |sheet: &Sheet, row, column| match sheet.value(row, column) {
            Value::Text(text) => text.clone(),
            other => panic!("not text at {row},{column}: {other:?}"),
        };
        let number = |sheet: &Sheet, row, column| sheet.value(row, column).as_f64_opt();

        let points = book.sheet(0);
        assert_eq!(points.name(), POINTS_SHEET);
        let header: Vec<_> = (0..7).map(|column| text(points, 0, column)).collect();
        assert_eq!(
            header,
            [
                "login",
                "name",
                "pipes.md",
                "signals.md",
                "pipes.md (2)",
                "manual",
                "total"
            ]
        );
        // Sorted by login, like the room CSV.
        assert_eq!(text(points, 1, 0), "novakja1");
        assert_eq!(text(points, 2, 0), "svobodpe");
        let row: Vec<_> = (2..7).map(|column| number(points, 1, column)).collect();
        assert_eq!(row, [Some(0.5), Some(-1.0), None, Some(2.0), Some(1.5)]);
        assert_eq!(
            points.formula(1, 6).map(String::as_str),
            Some("of:=SUM([.C2:.F2])")
        );

        let seconds = book.sheet(1);
        assert_eq!(seconds.name(), SECONDS_SHEET);
        assert_eq!(text(seconds, 0, 4), "pipes.md (2)");
        let row: Vec<_> = (2..6).map(|column| number(seconds, 2, column)).collect();
        // Points given by hand took no time.
        assert_eq!(row, [Some(12.0), None, Some(45.0), None]);

        // The file names of the questions are rotated, login and name not.
        let rotation = |sheet: &Sheet, column| {
            let style = sheet.cellstyle(0, column).unwrap();
            book.cellstyle(style)
                .unwrap()
                .cellstyle()
                .attr("style:rotation-angle")
                .map(str::to_owned)
        };
        for sheet in [points, seconds] {
            assert_eq!(rotation(sheet, 0), None);
            assert_eq!(rotation(sheet, 2).as_deref(), Some("90deg"));
        }

        // A color scale for each question on both sheets, and a data bar
        // for the total.
        for column in ["C", "D", "E", "F"] {
            for sheet in ["Points", "Time"] {
                let address =
                    format!("&apos;{sheet}&apos;.{column}2:&apos;{sheet}&apos;.{column}3");
                assert!(content.contains(&address), "{address} in {content}");
            }
        }
        assert_eq!(content.matches("<calcext:color-scale>").count(), 8);
        assert_eq!(content.matches("<calcext:data-bar ").count(), 1);
        assert!(content.contains("&apos;Points&apos;.G2:&apos;Points&apos;.G3"));
    }

    fn read_content(path: &Path) -> String {
        let mut archive = ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
        let mut content = String::new();
        archive
            .by_name("content.xml")
            .unwrap()
            .read_to_string(&mut content)
            .unwrap();
        content
    }

    #[test]
    fn a_question_nobody_answered_has_nothing_to_color() {
        let questions = [results("pipes.md", false, &[])];
        let (_, rules) = workbook(&room(&questions));
        assert!(rules.values().all(Vec::is_empty));
    }
}
