//! Terminal output and the CSV export.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Local};
use quiz_common::{
    Question,
    api::{QuestionInfo, Results},
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
        println!("  {}) {}", answer.label, ellipsis(&answer.plain, 100));
    }
    println!();
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
            "  {} {:<30} {count:>4} {share:>5.1} %  {}",
            answer.label,
            "#".repeat(width),
            ellipsis(&answer.text, 60),
        );
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
    use super::{csv_path, ellipsis};
    use chrono::TimeZone;
    use std::path::Path;

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
}
