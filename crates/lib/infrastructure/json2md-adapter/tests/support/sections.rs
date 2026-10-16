//! The collapsed `<details>` sections of a rendered summary, located through a
//! CommonMark parser so that escaped text, code and table cells never open or
//! close a section.

use pulldown_cmark::{Event, Options, Parser};

const DETAILS: &str = "<details>";
const DETAILS_END: &str = "</details>";
const SUMMARY: &str = "<summary>";
const SUMMARY_END: &str = "</summary>";

/// One `<details>` element.
#[derive(Debug)]
pub struct Section<'a> {
    /// The text between `<summary>` and `</summary>`.
    pub summary: &'a str,
    /// The number of `<details>` elements that enclose this one.
    pub depth: usize,
    /// The text after `</summary>` up to this element's `</details>`.
    pub content: &'a str,
}

/// A rendered summary divided at its sections.
#[derive(Debug)]
pub struct Layout<'a> {
    /// The text before the first section; all of it when there is none.
    pub visible: &'a str,
    /// Every section in the order it opens.
    pub sections: Vec<Section<'a>>,
    /// The text after the last top-level section closes.
    pub trailing: &'a str,
}

impl<'a> Layout<'a> {
    /// Each section's depth and summary, in the order the sections open.
    pub fn outline(&self) -> Vec<(usize, &'a str)> {
        self.sections
            .iter()
            .map(|section| (section.depth, section.summary))
            .collect()
    }

    /// The content of the one section whose summary is exactly `summary`.
    pub fn section(&self, summary: &str) -> Result<&'a str, String> {
        let mut found = self
            .sections
            .iter()
            .filter(|section| section.summary == summary);
        let section = found
            .next()
            .ok_or_else(|| format!("missing section {summary:?}: {:?}", self.outline()))?;
        if found.next().is_some() {
            return Err(format!(
                "repeated section {summary:?}: {:?}",
                self.outline()
            ));
        }
        Ok(section.content)
    }
}

struct Open<'a> {
    slot: usize,
    depth: usize,
    summary_start: Option<usize>,
    summary: Option<&'a str>,
    content_start: Option<usize>,
}

/// Divides `body` at the `<details>`, `<summary>` and their closing tags that
/// the parser reports as HTML. Unbalanced tags or a section without a summary
/// are errors.
pub fn layout(body: &str) -> Result<Layout<'_>, String> {
    let mut tags = Vec::new();
    for (event, range) in Parser::new_ext(body, Options::ENABLE_TABLES).into_offset_iter() {
        if matches!(event, Event::Html(_) | Event::InlineHtml(_)) {
            let html = &body[range.clone()];
            for (at, _) in html.match_indices('<') {
                if let Some(tag) = [DETAILS, DETAILS_END, SUMMARY, SUMMARY_END]
                    .into_iter()
                    .find(|tag| html[at..].starts_with(tag))
                {
                    tags.push((range.start + at, tag));
                }
            }
        }
    }

    let mut slots: Vec<Option<Section<'_>>> = Vec::new();
    let mut open: Vec<Open<'_>> = Vec::new();
    let mut visible_end = body.len();
    let mut trailing_start = body.len();
    for (at, tag) in tags {
        let end = at + tag.len();
        match tag {
            DETAILS => {
                if slots.is_empty() {
                    visible_end = at;
                }
                open.push(Open {
                    slot: slots.len(),
                    depth: open.len(),
                    summary_start: None,
                    summary: None,
                    content_start: None,
                });
                slots.push(None);
            }
            SUMMARY => {
                open.last_mut()
                    .ok_or_else(|| format!("summary outside a section at byte {at}"))?
                    .summary_start = Some(end);
            }
            SUMMARY_END => {
                let section = open
                    .last_mut()
                    .ok_or_else(|| format!("summary end outside a section at byte {at}"))?;
                let start = section
                    .summary_start
                    .take()
                    .ok_or_else(|| format!("summary end without a start at byte {at}"))?;
                section.summary = Some(&body[start..at]);
                section.content_start = Some(end);
            }
            _ => {
                let section = open
                    .pop()
                    .ok_or_else(|| format!("unopened section closes at byte {at}"))?;
                let (Some(summary), Some(content_start)) = (section.summary, section.content_start)
                else {
                    return Err(format!("section without a summary closes at byte {at}"));
                };
                slots[section.slot] = Some(Section {
                    summary,
                    depth: section.depth,
                    content: &body[content_start..at],
                });
                if open.is_empty() {
                    trailing_start = end;
                }
            }
        }
    }
    if !open.is_empty() {
        return Err(format!("{} sections never close", open.len()));
    }
    Ok(Layout {
        visible: &body[..visible_end],
        sections: slots
            .into_iter()
            .collect::<Option<_>>()
            .ok_or("a section never closes")?,
        trailing: &body[trailing_start..],
    })
}
