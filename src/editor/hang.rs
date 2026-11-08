//! Hanging indent: the rows a long line wraps into start under its text,
//! not at the margin, under the words of an `\item`, of a list item in a
//! note, or of an indented paragraph.

use super::highlight::Lang;
use egui::text::{ByteIndex, LayoutJob, LayoutSection};
use egui::{Galley, Rect};
use std::sync::Arc;

/// How many chars of `line` its wrapped rows should start under: the
/// leading spaces, plus the `\item [label]` or list marker after them.
/// `None` when the rows should start at the margin.
pub fn prefix(line: &str, lang: Lang) -> Option<usize> {
    let ws = line.chars().take_while(|c| *c == ' ' || *c == '\t').count();
    let rest = &line[ws..];
    let marker = match lang {
        Lang::Tex => item(rest),
        Lang::Markdown => list_marker(rest),
        Lang::Plain => 0,
    };
    let n = ws + marker;
    // A line that is only its prefix has nothing to hang under.
    (n > 0 && n < line.chars().count()).then_some(n)
}

/// `\item`, its optional `[label]` and the spaces after them, in chars.
fn item(rest: &str) -> usize {
    let Some(after) = rest.strip_prefix("\\item") else { return 0 };
    if after.starts_with(|c: char| c.is_alphabetic()) {
        return 0; // \itemsep, \itemize…
    }
    let mut n = "\\item".len();
    let mut tail = after;
    if let Some(label) = tail.strip_prefix('[') {
        match label.find(']') {
            Some(end) => {
                n += label[..=end].chars().count() + 1;
                tail = &label[end + 1..];
            }
            None => return 0,
        }
    }
    n + tail.chars().take_while(|c| *c == ' ').count()
}

/// `- `, `* `, `+ `, `1. ` and a `[ ] ` task box after them, in chars.
fn list_marker(rest: &str) -> usize {
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    let n = if rest.starts_with(['-', '*', '+']) {
        1
    } else if digits > 0 && rest[digits..].starts_with(['.', ')']) {
        digits + 1
    } else {
        return 0;
    };
    if !rest[n..].starts_with(' ') {
        return 0;
    }
    let mut n = n + rest[n..].chars().take_while(|c| *c == ' ').count();
    for task in ["[ ] ", "[x] ", "[X] "] {
        if rest[n..].starts_with(task) {
            n += task.len();
        }
    }
    n
}

/// `full` with each wrapped line that has a prefix laid out again,
/// narrower by the prefix's width, its rows after the first moved right by
/// it. Also returns the jobs laid out for that: egui keeps a layout only
/// while it is asked for every frame.
pub fn apply(ctx: &egui::Context, full: Arc<Galley>, job: &LayoutJob, lang: Lang) -> (Arc<Galley>, Vec<LayoutJob>) {
    let text = job.text.as_str();
    let width = job.wrap.max_width;
    let mut jobs = Vec::new();
    if !width.is_finite() || full.rows.len() == text.matches('\n').count() + 1 {
        return (full, jobs); // Nothing wraps.
    }
    let mut rows = Vec::with_capacity(full.rows.len());
    let mut changed = false;
    let mut dy = 0.0;
    let mut start = 0; // Byte where the line starts.
    let mut i = 0;
    while i < full.rows.len() {
        let mut end = i;
        while !full.rows[end].ends_with_newline && end + 1 < full.rows.len() {
            end += 1;
        }
        let line_end = text[start..].find('\n').map_or(text.len(), |n| start + n);
        let line = &text[start..line_end];
        let first = &full.rows[i];
        let indent = match prefix(line, lang) {
            Some(n) if end > i => first.row.glyphs.get(n).map(|g| g.pos.x).filter(|x| *x > 0.0 && *x < width * 0.4),
            _ => None,
        };
        match indent {
            Some(indent) => {
                let part = paragraph(job, start, line_end, width - indent);
                let galley = ctx.fonts_mut(|f| f.layout_job(part.clone()));
                jobs.push(part);
                let top = first.pos.y + dy;
                for (k, r) in galley.rows.iter().enumerate() {
                    let mut r = r.clone();
                    r.pos.y += top;
                    if k > 0 {
                        r.pos.x += indent;
                    }
                    r.ends_with_newline = k + 1 == galley.rows.len() && full.rows[end].ends_with_newline;
                    rows.push(r);
                }
                let old_bottom = full.rows[end].pos.y + full.rows[end].size.y;
                dy = top + galley.rect.height() - old_bottom;
                changed = true;
            }
            None => {
                for r in &full.rows[i..=end] {
                    let mut r = r.clone();
                    r.pos.y += dy;
                    rows.push(r);
                }
            }
        }
        start = (line_end + 1).min(text.len());
        i = end + 1;
    }
    if !changed {
        return (full, jobs);
    }
    let mut out = (*full).clone();
    out.rect = Rect::NOTHING;
    out.mesh_bounds = Rect::NOTHING;
    out.num_vertices = 0;
    out.num_indices = 0;
    for r in &rows {
        out.rect |= Rect::from_min_size(r.pos, r.size);
        out.mesh_bounds |= r.visuals.mesh_bounds.translate(r.pos.to_vec2());
        out.num_vertices += r.visuals.mesh.vertices.len();
        out.num_indices += r.visuals.mesh.indices.len();
    }
    out.rect.min = full.rect.min;
    out.rect.max.x = out.rect.max.x.max(full.rect.max.x);
    out.rows = rows;
    (Arc::new(out), jobs)
}

/// The job of `job`'s text from `start` to `end` alone, `width` wide.
fn paragraph(job: &LayoutJob, start: usize, end: usize, width: f32) -> LayoutJob {
    let mut part = LayoutJob {
        text: job.text[start..end].to_owned(),
        wrap: egui::text::TextWrapping { max_width: width, ..job.wrap.clone() },
        sections: Vec::new(),
        break_on_newline: job.break_on_newline,
        halign: job.halign,
        justify: job.justify,
        first_row_min_height: 0.0,
        round_output_to_gui: job.round_output_to_gui,
        keep_trailing_whitespace: job.keep_trailing_whitespace,
    };
    let from = job.sections.partition_point(|s| s.byte_range.end.0 <= start);
    for s in job.sections[from..].iter().take_while(|s| s.byte_range.start.0 < end.max(start + 1)) {
        let a = s.byte_range.start.0.max(start) - start;
        let b = s.byte_range.end.0.min(end) - start;
        part.sections.push(LayoutSection { leading_space: 0.0, byte_range: ByteIndex(a)..ByteIndex(b), format: s.format.clone() });
    }
    part
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes() {
        assert_eq!(prefix("\\item Um item longo", Lang::Tex), Some(6));
        assert_eq!(prefix("  \\item[a)] texto", Lang::Tex), Some(12));
        assert_eq!(prefix("    texto recuado", Lang::Tex), Some(4));
        assert_eq!(prefix("\\itemsep 2pt", Lang::Tex), None);
        assert_eq!(prefix("Texto na margem", Lang::Tex), None);
        assert_eq!(prefix("\\item", Lang::Tex), None);
        assert_eq!(prefix("- [ ] tarefa", Lang::Markdown), Some(6));
        assert_eq!(prefix("12. passo", Lang::Markdown), Some(4));
        assert_eq!(prefix("-sem espaço", Lang::Markdown), None);
        assert_eq!(prefix("- item", Lang::Plain), None);
    }

    #[test]
    fn wrapped_rows_start_under_the_text() {
        let ctx = egui::Context::default();
        ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
        let font = egui::FontId::monospace(14.0);
        let text = "\\item uma frase bem longa que quebra em várias linhas no editor\nfim";
        let mut job = LayoutJob::simple(text.to_owned(), font.clone(), egui::Color32::WHITE, 200.0);
        job.wrap.max_width = 200.0;
        let full = ctx.fonts_mut(|f| f.layout_job(job.clone()));
        assert!(full.rows.len() > 2, "the line wraps");
        let (hung, jobs) = apply(&ctx, full.clone(), &job, Lang::Tex);
        assert_eq!(jobs.len(), 1);
        let under = hung.rows[0].row.glyphs[6].pos.x;
        assert!(under > 0.0);
        let last = hung.rows.len() - 1;
        for r in &hung.rows[1..last] {
            assert!((r.pos.x - under).abs() < 0.5, "continuation starts under the text");
            assert!(r.pos.x + r.size.x <= 200.5, "and still fits");
        }
        assert_eq!(hung.rows[last].pos.x, 0.0, "the next line is at the margin");
        assert!(hung.rows[last - 1].ends_with_newline);
        assert_eq!(hung.text(), text);
        // The cursor still lands where it is drawn.
        use egui::text::CCursor;
        let far = text.find("editor").unwrap();
        let rect = hung.pos_from_cursor(CCursor::new(text[..far].chars().count()));
        assert_eq!(hung.cursor_from_pos(rect.center().to_vec2()).index.0, text[..far].chars().count());
        ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
    }
}
