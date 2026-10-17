//! Terminal output and the CSV export.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Local, Utc};
use quiz_common::{
    Question,
    api::{AnswerInfo, QuestionInfo, Results, RoomInfo},
};

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
/// by the date and time, as the assignment requires.
pub fn csv_path(source: &Path, when: DateTime<Local>) -> PathBuf {
    let stem = source
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "question".to_owned());
    let name = format!("{stem}_{}.csv", when.format("%Y-%m-%d_%H-%M-%S"));
    match source.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.join(name),
        _ => PathBuf::from(name),
    }
}

/// Writes one row per student: login, name, chosen answer and when.
pub fn write_csv(path: &Path, results: &Results) -> Result<()> {
    let mut writer =
        csv::Writer::from_path(path).with_context(|| format!("cannot write {}", path.display()))?;
    for row in &results.rows {
        writer
            .serialize(row)
            .with_context(|| format!("cannot write {}", path.display()))?;
    }
    writer
        .flush()
        .with_context(|| format!("cannot write {}", path.display()))?;
    Ok(())
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
    use super::{csv_path, ellipsis, points_note, remaining};
    use chrono::TimeZone;
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
            csv_path(Path::new("cviceni/01regex/otazky.md"), when),
            Path::new("cviceni/01regex/otazky_2026-09-21_14-05-03.csv")
        );
        assert_eq!(
            csv_path(Path::new("question.md"), when),
            Path::new("question_2026-09-21_14-05-03.csv")
        );
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
