use std::collections::BTreeMap;

use crate::GitLabError;

const MAX_COORDINATE: u64 = i32::MAX as u64;

#[derive(Debug)]
pub(super) struct MappedLine {
    pub old_line: Option<u64>,
    pub old_position: u64,
    pub new_line: u64,
    pub text: String,
    pub hunk: usize,
}

struct Hunk {
    old: u64,
    new: u64,
    old_left: u64,
    new_left: u64,
}

impl Hunk {
    fn complete(&self) -> bool {
        self.old_left == 0 && self.new_left == 0
    }

    fn consume(
        &mut self,
        prefix: u8,
        text: &str,
        hunk: usize,
    ) -> Result<Option<MappedLine>, GitLabError> {
        let consumes_old = matches!(prefix, b' ' | b'-');
        let consumes_new = matches!(prefix, b' ' | b'+');
        if (!consumes_old && !consumes_new)
            || (consumes_old && (self.old_left == 0 || self.old == 0 || self.old > MAX_COORDINATE))
            || (consumes_new && (self.new_left == 0 || self.new == 0 || self.new > MAX_COORDINATE))
        {
            return Err(GitLabError::UnsupportedPosition);
        }
        let mapped = consumes_new.then(|| MappedLine {
            old_line: consumes_old.then_some(self.old),
            old_position: self.old,
            new_line: self.new,
            text: text.to_owned(),
            hunk,
        });
        if consumes_old {
            self.old_left -= 1;
            self.old += 1;
        }
        if consumes_new {
            self.new_left -= 1;
            self.new += 1;
        }
        Ok(mapped)
    }
}

pub(super) fn parse(diff: &str) -> Result<BTreeMap<u64, MappedLine>, GitLabError> {
    let mut map = BTreeMap::new();
    let mut current: Option<Hunk> = None;
    let mut hunk_count = 0;
    let mut can_mark_no_newline = false;
    for line in diff.split_terminator('\n') {
        if line.starts_with("@@") {
            let next = parse_header(line)?;
            if let Some(previous) = &current
                && (!previous.complete() || next.old < previous.old || next.new < previous.new)
            {
                return Err(GitLabError::UnsupportedPosition);
            }
            current = Some(next);
            hunk_count += 1;
            can_mark_no_newline = false;
        } else if line == "\\ No newline at end of file" {
            if !can_mark_no_newline {
                return Err(GitLabError::UnsupportedPosition);
            }
            can_mark_no_newline = false;
        } else {
            let prefix = line
                .as_bytes()
                .first()
                .copied()
                .ok_or(GitLabError::UnsupportedPosition)?;
            let hunk = current.as_mut().ok_or(GitLabError::UnsupportedPosition)?;
            let text = line.get(1..).ok_or(GitLabError::UnsupportedPosition)?;
            if let Some(mapped) = hunk.consume(prefix, text, hunk_count)?
                && map.insert(mapped.new_line, mapped).is_some()
            {
                return Err(GitLabError::UnsupportedPosition);
            }
            can_mark_no_newline = true;
        }
    }
    if !current.is_some_and(|hunk| hunk.complete()) {
        return Err(GitLabError::UnsupportedPosition);
    }
    Ok(map)
}

fn parse_header(line: &str) -> Result<Hunk, GitLabError> {
    let (old, rest) = line
        .strip_prefix("@@ -")
        .and_then(|value| value.split_once(" +"))
        .ok_or(GitLabError::UnsupportedPosition)?;
    let (new, suffix) = rest
        .split_once(" @@")
        .ok_or(GitLabError::UnsupportedPosition)?;
    if !suffix.is_empty() && !suffix.starts_with(' ') {
        return Err(GitLabError::UnsupportedPosition);
    }
    let (old, old_left) = parse_range(old)?;
    let (new, new_left) = parse_range(new)?;
    if old_left == 0 && new_left == 0 {
        return Err(GitLabError::UnsupportedPosition);
    }
    Ok(Hunk {
        old,
        new,
        old_left,
        new_left,
    })
}

fn parse_range(value: &str) -> Result<(u64, u64), GitLabError> {
    let (start, count) = value.split_once(',').unwrap_or((value, "1"));
    let decimal = |text: &str| -> Result<u64, GitLabError> {
        if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(GitLabError::UnsupportedPosition);
        }
        text.parse().map_err(|_| GitLabError::UnsupportedPosition)
    };
    let start = decimal(start)?;
    let count = decimal(count)?;
    if start > MAX_COORDINATE
        || (count != 0 && start == 0)
        || start
            .checked_add(count)
            .is_none_or(|end| end > MAX_COORDINATE + 1)
    {
        return Err(GitLabError::UnsupportedPosition);
    }
    Ok((start, count))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn added_and_context_lines_retain_distinct_old_coordinates() {
        let map = parse("@@ -3,3 +3,4 @@ function\n first\n-old\n+new\n+extra\n final\n").unwrap();
        assert_eq!(map[&3].old_line, Some(3));
        assert_eq!(map[&4].old_line, None);
        assert_eq!(map[&4].old_position, 5);
        assert_eq!(map[&5].old_position, 5);
        assert_eq!(map[&6].old_line, Some(5));
        assert_eq!(map[&6].text, "final");
    }

    #[test]
    fn new_files_and_missing_terminal_newlines_preserve_content() {
        let map =
            parse("@@ -0,0 +1,2 @@\n+first\r\n+last\n\\ No newline at end of file\n").unwrap();
        assert_eq!(map[&1].old_position, 0);
        assert_eq!(map[&1].old_line, None);
        assert_eq!(map[&1].text, "first\r");
        assert_eq!(map[&2].text, "last");
        assert!(parse("@@ -1 +0,0 @@\n-only\n").unwrap().is_empty());
    }

    #[test]
    fn truncated_overlapping_invalid_and_out_of_bounds_hunks_are_rejected() {
        for diff in [
            "",
            "Binary files a/x and b/x differ\n",
            "@@ -1 +1,2 @@\n one\n",
            "@@ -1 +1 @@\n one\n+extra\n",
            "@@ -1 +1 @@\n one\n@@ -1 +1 @@\n one\n",
            "@@ -0 +1 @@\n+x\n",
            "@@ -1 +1 @@\n?unknown\n",
            "@@ -1 +1 @@\n\n",
            "@@ -1 +1 @@suffix\n one\n",
            "@@ -1 +2147483648 @@\n one\n",
            "@@ -1 +1 @@\n\\ No newline at end of file\n one\n",
            "@@ -1 +1 @@\n one\n\\ No newline at end of file\n\\ No newline at end of file\n",
        ] {
            assert!(parse(diff).is_err(), "unexpected mapping for {diff:?}");
        }
    }

    #[test]
    fn separate_hunks_keep_a_range_boundary() {
        let map = parse("@@ -1 +1 @@\n first\n@@ -2 +2 @@\n second\n").unwrap();
        assert_ne!(map[&1].hunk, map[&2].hunk);
    }
}
