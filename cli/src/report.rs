//! Terminal output and the CSV export.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use chrono::{DateTime, Local, Utc};
use quiz_common::{
    Question,
    api::{AnswerInfo, QuestionInfo, ResultRow, Results, RoomInfo},
};
use serde::Serialize;

/// Shows the parsed question, so that the teacher can check the Markdown was
/// understood as intended before students see it.
pub fn print_question(question: &Question) {
    if let Some(title) = &question.title {
        println!("{title}");
        println!("{}", "=".repeat(title.chars().count()));
    }
    println!("{}\n", question.prompt_markdown.trim());
    for answer in &question.answers {
        println!(
            "  {}) {}{}",
            answer.label,
            ellipsis(&answer.plain, 100),
            points_note(answer.points)
        );
    }
    println!();
    if let Some(notes) = &question.notes {
        println!("Teacher notes (not shown to students):");
        println!("{notes}\n");
    }
}

/// Lists the answers of a published question, in the order the students see
/// them, which after a shuffle is not the order of the Markdown file.
pub fn print_answers(answers: &[AnswerInfo]) {
    for answer in answers {
        println!(
            "  {}) {}{}",
            answer.label,
            ellipsis(&answer.text, 100),
            points_note(answer.points)
        );
    }
}

/// Prints the final counts as a bar graph, the same data the browser shows.
pub fn print_summary(question: &QuestionInfo, results: &Results) {
    let total = results.rows.len();
    let max = question.counts.iter().copied().max().unwrap_or(0);

    println!("\nAnswers: {total}");
    for (answer, &count) in question.answers.iter().zip(&question.counts) {
        let width = count * 30 / max.max(1);
        let share = if total == 0 {
            0.0
        } else {
            count as f64 * 100.0 / total as f64
        };
        println!(
            "  {} {:<30} {count:>4} {share:>5.1} %  {}{}",
            answer.label,
            "#".repeat(width),
            ellipsis(&answer.text, 60),
            points_note(answer.points),
        );
    }

    if let Some(average) = average_points(results) {
        let best = question
            .answers
            .iter()
            .filter_map(|answer| answer.points)
            .fold(f64::NEG_INFINITY, f64::max);
        println!("Average points: {average:.2} of {best}");
    }
}

/// The points students got on average, `None` when the question is not
/// scored or nobody answered.
fn average_points(results: &Results) -> Option<f64> {
    let points: Vec<f64> = results.rows.iter().filter_map(|row| row.points).collect();
    (!points.is_empty()).then(|| points.iter().sum::<f64>() / points.len() as f64)
}

/// ` (1 point)` after an answer of a scored question, nothing otherwise.
fn points_note(points: Option<f64>) -> String {
    match points {
        Some(1.0) => " (1 point)".to_owned(),
        Some(points) => format!(" ({points} points)"),
        None => String::new(),
    }
}

/// Where the answers are stored: the base name of the question file followed
/// by the date and time, as the assignment requires. It goes to `dir`, or
/// next to the question file without one.
pub fn csv_path(source: &Path, dir: Option<&Path>, when: DateTime<Local>) -> PathBuf {
    let stem = source
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "question".to_owned());
    place(
        source,
        dir,
        format!("{stem}_{}.csv", when.format("%Y-%m-%d_%H-%M-%S")),
    )
}

/// Where the points of the whole room are kept: the date and time the room
/// was opened followed by the room code, so that every question of one
/// lecture lands in the same file. It goes where the answers of the question
/// go: to `dir`, or next to the question file.
pub fn room_csv_path(
    source: &Path,
    dir: Option<&Path>,
    code: &str,
    opened: DateTime<Local>,
) -> PathBuf {
    place(source, dir, format!("{}.csv", room_name(code, opened)))
}

/// The directory `quiz ask --room-dir` saves into, named like the CSV with
/// the points of the room, in `dir` or next to the question file.
pub fn room_dir_path(
    source: &Path,
    dir: Option<&Path>,
    code: &str,
    opened: DateTime<Local>,
) -> PathBuf {
    place(source, dir, room_name(code, opened))
}

fn room_name(code: &str, opened: DateTime<Local>) -> String {
    format!("room_{}_{code}", opened.format("%Y-%m-%d_%H-%M-%S"))
}

/// `name` in `dir`, or in the directory of `source` without one.
fn place(source: &Path, dir: Option<&Path>, name: String) -> PathBuf {
    match dir.or(source.parent()) {
        Some(dir) if !dir.as_os_str().is_empty() => dir.join(name),
        _ => PathBuf::from(name),
    }
}

/// One row of the CSV of a question: a [`ResultRow`] and how long the
/// student took to answer.
#[derive(Serialize)]
struct CsvRow<'a> {
    login: &'a str,
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    points: Option<f64>,
    answer: &'a str,
    answer_text: &'a str,
    answered_at: DateTime<Utc>,
    /// Seconds from asking the question to the answer, to a tenth.
    seconds: f64,
}

impl<'a> CsvRow<'a> {
    fn new(row: &'a ResultRow, asked_at: DateTime<Utc>) -> Self {
        let millis = (row.answered_at - asked_at).num_milliseconds();
        CsvRow {
            login: &row.login,
            name: &row.name,
            points: row.points,
            answer: &row.answer,
            answer_text: &row.answer_text,
            answered_at: row.answered_at,
            seconds: (millis as f64 / 100.0).round() / 10.0,
        }
    }
}

/// Writes one row per student: login, name, chosen answer, when, and how
/// long after the question was asked.
pub fn write_csv(path: &Path, results: &Results) -> Result<()> {
    let mut writer =
        csv::Writer::from_path(path).with_context(|| format!("cannot write {}", path.display()))?;
    for row in &results.rows {
        writer
            .serialize(CsvRow::new(row, results.question.created_at))
            .with_context(|| format!("cannot write {}", path.display()))?;
    }
    writer
        .flush()
        .with_context(|| format!("cannot write {}", path.display()))?;
    Ok(())
}

/// Writes the points of every student in the room: one column per scored
/// question, in the order they were asked, and the total in the last one.
pub fn write_room_csv(path: &Path, questions: &[Results]) -> Result<()> {
    let mut writer =
        csv::Writer::from_path(path).with_context(|| format!("cannot write {}", path.display()))?;
    for record in room_table(questions) {
        writer
            .write_record(&record)
            .with_context(|| format!("cannot write {}", path.display()))?;
    }
    writer
        .flush()
        .with_context(|| format!("cannot write {}", path.display()))?;
    Ok(())
}

/// The records of the room CSV, the header first. Students are sorted by
/// login; a question a student did not answer is left empty and counts as
/// nothing towards the total.
fn room_table(questions: &[Results]) -> Vec<Vec<String>> {
    let mut header = vec!["login".to_owned(), "name".to_owned()];
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    for results in questions {
        // The same file asked twice still gets two columns that can be
        // told apart.
        let name = results.question.source_name.as_str();
        let count = seen.entry(name).or_default();
        *count += 1;
        header.push(match *count {
            1 => name.to_owned(),
            n => format!("{name} ({n})"),
        });
    }
    header.push("total".to_owned());

    let mut students: BTreeMap<&str, (&str, Vec<Option<f64>>)> = BTreeMap::new();
    for (column, results) in questions.iter().enumerate() {
        for row in &results.rows {
            let student = students
                .entry(&row.login)
                .or_insert_with(|| ("", vec![None; questions.len()]));
            student.0 = &row.name;
            student.1[column] = row.points;
        }
    }

    let mut table = vec![header];
    for (login, (name, points)) in students {
        let mut record = vec![login.to_owned(), name.to_owned()];
        record.extend(
            points
                .iter()
                .map(|points| points.map(number).unwrap_or_default()),
        );
        // Rounded, so that 0.1 + 0.2 does not end up as 0.30000000000000004.
        let total: f64 = points.iter().flatten().sum();
        record.push(number((total * 1e6).round() / 1e6));
        table.push(record);
    }
    table
}

/// A number written the way the question CSV writes its points: `1.0`,
/// `-0.5`.
fn number(value: f64) -> String {
    format!("{value:?}")
}

/// What `quiz status` shows: the room, and a line for every question the
/// server still keeps in it.
pub fn print_room(room: &RoomInfo) {
    let now = Local::now();
    println!("Question code:      {}", room.code);
    println!("Students answer at: {}", room.answer_url);
    println!("Presentation view:  {}", room.display_url);
    println!(
        "                    {}",
        if room.presented {
            "open in a browser"
        } else {
            "not open anywhere"
        }
    );
    println!("Students in room:   {}", room.students);
    println!(
        "Opened at {}, last question at {}; dropped in {} unless another is asked.",
        time(room.created_at, now),
        time(room.last_used_at, now),
        remaining(room.expires_at, now)
    );

    if room.questions.is_empty() {
        println!("\nNo questions are kept in the room.");
        return;
    }
    println!("\n  #  Asked  Dropped in  State     Answers  Id                                File");
    for question in &room.questions {
        let info = &question.info;
        let state = match (info.open, info.revealed) {
            (true, _) => "open",
            (false, false) => "closed",
            (false, true) => "revealed",
        };
        let current = room.current.as_ref() == Some(&info.id);
        let title = info
            .title
            .as_ref()
            .map(|title| format!(": {}", ellipsis(title, 40)))
            .unwrap_or_default();
        println!(
            "{} {:>2}  {:<5}  {:<10}  {state:<8}  {:>7}  {}  {}{title}",
            if current { "*" } else { " " },
            question.number,
            time(info.created_at, now),
            remaining(question.expires_at, now),
            info.answer_count,
            info.id,
            info.source_name,
        );
    }
    if room.current.is_some() {
        println!("(* is the question the students see)");
    }
    if room.questions.iter().any(|question| question.info.open) {
        println!(
            "\nAn open question whose `quiz ask` is not running any more is closed, \
             and its answers saved, by `quiz close <id>`."
        );
    }
}

/// How long until `when`, roughly: the server drops things every few
/// minutes, not to the second.
fn remaining(when: DateTime<Utc>, now: DateTime<Local>) -> String {
    let minutes = (when - now.with_timezone(&Utc)).num_minutes().max(0);
    match (minutes / 60, minutes % 60) {
        (0, 0) => "a moment".to_owned(),
        (0, minutes) => format!("{minutes}m"),
        (hours, minutes) => format!("{hours}h {minutes:02}m"),
    }
}

/// A time in the local zone, with the date only when it is not today.
fn time(when: DateTime<Utc>, now: DateTime<Local>) -> String {
    let when = when.with_timezone(&Local);
    if when.date_naive() == now.date_naive() {
        when.format("%H:%M").to_string()
    } else {
        when.format("%Y-%m-%d %H:%M").to_string()
    }
}

/// Shortens `text` to `limit` characters, for one-line terminal output.
fn ellipsis(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let kept: String = text.chars().take(limit.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}

#[cfg(test)]
mod tests {
    use super::{
        CsvRow, csv_path, ellipsis, points_note, remaining, room_csv_path, room_dir_path,
        room_table,
    };
    use chrono::TimeZone;
    use quiz_common::api::{QuestionInfo, ResultRow, Results, StudentView};
    use std::path::Path;

    #[test]
    fn remaining_time_is_in_hours_and_minutes() {
        let now = chrono::Local::now();
        let at = |minutes| now.with_timezone(&chrono::Utc) + chrono::TimeDelta::minutes(minutes);
        assert_eq!(remaining(at(12 * 60), now), "12h 00m");
        assert_eq!(remaining(at(65), now), "1h 05m");
        assert_eq!(remaining(at(7), now), "7m");
        assert_eq!(remaining(at(-3), now), "a moment");
    }

    #[test]
    fn csv_sits_next_to_the_question_with_date_and_time() {
        let when = chrono::Local
            .with_ymd_and_hms(2026, 9, 21, 14, 5, 3)
            .unwrap();
        assert_eq!(
            csv_path(Path::new("labs/01regex/questions.md"), None, when),
            Path::new("labs/01regex/questions_2026-09-21_14-05-03.csv")
        );
        assert_eq!(
            csv_path(Path::new("question.md"), None, when),
            Path::new("question_2026-09-21_14-05-03.csv")
        );
        assert_eq!(
            csv_path(
                Path::new("labs/01regex/questions.md"),
                Some(Path::new("results")),
                when
            ),
            Path::new("results/questions_2026-09-21_14-05-03.csv")
        );
    }

    #[test]
    fn room_csv_is_named_after_the_room_and_when_it_was_opened() {
        let opened = chrono::Local
            .with_ymd_and_hms(2026, 9, 21, 14, 0, 7)
            .unwrap();
        assert_eq!(
            room_csv_path(
                Path::new("labs/01regex/questions.md"),
                None,
                "428173",
                opened
            ),
            Path::new("labs/01regex/room_2026-09-21_14-00-07_428173.csv")
        );
        assert_eq!(
            room_csv_path(Path::new("questions.md"), None, "428173", opened),
            Path::new("room_2026-09-21_14-00-07_428173.csv")
        );
        assert_eq!(
            room_csv_path(
                Path::new("questions.md"),
                Some(Path::new("results")),
                "428173",
                opened
            ),
            Path::new("results/room_2026-09-21_14-00-07_428173.csv")
        );
        assert_eq!(
            room_dir_path(Path::new("labs/questions.md"), None, "428173", opened),
            Path::new("labs/room_2026-09-21_14-00-07_428173")
        );
        assert_eq!(
            room_dir_path(
                Path::new("questions.md"),
                Some(Path::new("results")),
                "428173",
                opened
            ),
            Path::new("results/room_2026-09-21_14-00-07_428173")
        );
    }

    fn results(source_name: &str, rows: &[(&str, &str, f64)]) -> Results {
        let now = chrono::Utc::now();
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
                created_at: now,
                closed_at: Some(now),
                revealed: false,
            },
            rows: rows
                .iter()
                .map(|&(login, name, points)| ResultRow {
                    login: login.to_owned(),
                    name: name.to_owned(),
                    points: Some(points),
                    answer: "A".to_owned(),
                    answer_text: String::new(),
                    answered_at: now,
                })
                .collect(),
        }
    }

    #[test]
    fn room_table_has_a_column_per_question_and_the_total() {
        let table = room_table(&[
            results(
                "q1.md",
                &[("svobodpe", "Petra", 1.0), ("novakja1", "Jan", 0.1)],
            ),
            results("q2.md", &[("novakja1", "Jan", 0.2)]),
            results("q1.md", &[("svobodpe", "Petra", -0.5)]),
        ]);
        assert_eq!(
            table,
            [
                vec!["login", "name", "q1.md", "q2.md", "q1.md (2)", "total"],
                vec!["novakja1", "Jan", "0.1", "0.2", "", "0.3"],
                vec!["svobodpe", "Petra", "1.0", "", "-0.5", "0.5"],
            ]
        );
        assert_eq!(room_table(&[]), [vec!["login", "name", "total"]]);
    }

    #[test]
    fn csv_has_seconds_from_asking_to_answering() {
        let results = results("q1.md", &[("novakja1", "Jan", 1.0)]);
        let mut row = results.rows[0].clone();
        row.answered_at = results.question.created_at + chrono::TimeDelta::milliseconds(12_345);
        assert_eq!(CsvRow::new(&row, results.question.created_at).seconds, 12.3);
    }

    #[test]
    fn long_answers_are_shortened() {
        assert_eq!(ellipsis("short", 10), "short");
        assert_eq!(ellipsis("abcdefghij", 5), "abcd…");
    }

    #[test]
    fn points_are_shown_only_for_a_scored_question() {
        assert_eq!(points_note(None), "");
        assert_eq!(points_note(Some(1.0)), " (1 point)");
        assert_eq!(points_note(Some(0.0)), " (0 points)");
        assert_eq!(points_note(Some(-0.5)), " (-0.5 points)");
    }
}
