//! The collapsed `<details>` sections of a rendered summary, the headed groups
//! inside a section and the blocks a reader sees, located through a CommonMark
//! parser so that escaped text, code and table cells never open or close a
//! section or a group.

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

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

/// Section content that opens with a level-four heading.
#[derive(Debug)]
pub struct Group<'a> {
    /// The heading's decoded text.
    pub heading: String,
    /// The text after the heading up to the next level-four heading or the end.
    pub content: &'a str,
}

/// Section content divided at its level-four headings.
#[derive(Debug)]
pub struct Groups<'a> {
    /// The text before the first heading; all of it when there is none.
    pub lead: &'a str,
    /// Every group in the order it appears.
    pub groups: Vec<Group<'a>>,
}

impl<'a> Groups<'a> {
    /// Each group's heading, in order.
    pub fn headings(&self) -> Vec<&str> {
        self.groups
            .iter()
            .map(|group| group.heading.as_str())
            .collect()
    }

    /// The content of the one group whose heading is exactly `heading`.
    pub fn group(&self, heading: &str) -> Result<&'a str, String> {
        let mut found = self.groups.iter().filter(|group| group.heading == heading);
        let group = found
            .next()
            .ok_or_else(|| format!("missing group {heading:?}: {:?}", self.headings()))?;
        if found.next().is_some() {
            return Err(format!("repeated group {heading:?}: {:?}", self.headings()));
        }
        Ok(group.content)
    }
}

/// Divides `content` at the level-four headings that the parser reports.
pub fn groups(content: &str) -> Groups<'_> {
    let mut headings = Vec::new();
    let mut open: Option<(usize, String)> = None;
    for (event, range) in Parser::new_ext(content, Options::ENABLE_TABLES).into_offset_iter() {
        match event {
            Event::Start(Tag::Heading {
                level: HeadingLevel::H4,
                ..
            }) => open = Some((range.start, String::new())),
            Event::Text(text) | Event::Code(text) => {
                if let Some((_, heading)) = open.as_mut() {
                    heading.push_str(&text);
                }
            }
            Event::End(TagEnd::Heading(HeadingLevel::H4)) => {
                if let Some((start, heading)) = open.take() {
                    headings.push((start, range.end, heading));
                }
            }
            _ => {}
        }
    }
    let lead_end = headings
        .first()
        .map_or(content.len(), |(start, _, _)| *start);
    let ends = headings
        .iter()
        .skip(1)
        .map(|(start, _, _)| *start)
        .chain(std::iter::once(content.len()))
        .collect::<Vec<_>>();
    Groups {
        lead: &content[..lead_end],
        groups: headings
            .into_iter()
            .zip(ends)
            .map(|((_, start, heading), end)| Group {
                heading,
                content: &content[start..end],
            })
            .collect(),
    }
}

/// The blocks of a Markdown text, each reduced to its decoded text.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Blocks {
    /// Each heading's level and text.
    pub headings: Vec<(usize, String)>,
    /// Each paragraph outside a list item.
    pub paragraphs: Vec<String>,
    /// Each list item.
    pub items: Vec<String>,
    /// Each table's header cells.
    pub tables: Vec<Vec<String>>,
}

/// The headings, paragraphs, list items and table headers that the parser
/// reports in `text`, in order within each kind.
pub fn blocks(text: &str) -> Blocks {
    let mut blocks = Blocks::default();
    let mut heading: Option<(usize, String)> = None;
    let mut paragraph: Option<String> = None;
    let mut item: Option<String> = None;
    let mut head: Option<Vec<String>> = None;
    let mut cell: Option<String> = None;
    for event in Parser::new_ext(text, Options::ENABLE_TABLES) {
        let target = cell
            .as_mut()
            .or(item.as_mut())
            .or(heading.as_mut().map(|(_, text)| text))
            .or(paragraph.as_mut());
        match event {
            Event::Text(text) | Event::Code(text) => {
                if let Some(target) = target {
                    target.push_str(&text);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some(target) = target {
                    target.push('\n');
                }
            }
            Event::Start(Tag::Heading { level, .. }) => {
                heading = Some((level as usize, String::new()));
            }
            Event::End(TagEnd::Heading(_)) => blocks.headings.extend(heading.take()),
            Event::Start(Tag::Paragraph) if item.is_none() => paragraph = Some(String::new()),
            Event::End(TagEnd::Paragraph) => blocks.paragraphs.extend(paragraph.take()),
            Event::Start(Tag::Item) => item = Some(String::new()),
            Event::End(TagEnd::Item) => blocks.items.extend(item.take()),
            Event::Start(Tag::TableHead) => head = Some(Vec::new()),
            Event::End(TagEnd::TableHead) => blocks.tables.extend(head.take()),
            Event::Start(Tag::TableCell) if head.is_some() => cell = Some(String::new()),
            Event::End(TagEnd::TableCell) => {
                if let (Some(text), Some(head)) = (cell.take(), head.as_mut()) {
                    head.push(text);
                }
            }
            _ => {}
        }
    }
    blocks
}
