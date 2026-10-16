use crate::GitLabError;
use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};
use review_kit_domain::Finding;

pub(crate) fn validate_quick_actions(body: &str) -> Result<(), GitLabError> {
    // GitLab removes carriage returns before extracting slash commands.
    let normalized = body.replace('\r', "");
    let mut literal_ranges = Vec::new();
    let mut block_start = None;
    for (event, range) in Parser::new(&normalized).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(_)) => block_start = Some(range.start),
            Event::End(TagEnd::CodeBlock) => {
                if let Some(start) = block_start.take() {
                    literal_ranges.push(start..range.end);
                }
            }
            _ => {}
        }
    }
    let mut offset = 0;
    for line in normalized.split_inclusive('\n') {
        if line.starts_with('/') && !literal_ranges.iter().any(|range| range.contains(&offset)) {
            return Err(GitLabError::UnsafeBody);
        }
        offset += line.len();
    }
    Ok(())
}

fn suggestions(body: &str) -> Result<Vec<(String, String)>, GitLabError> {
    // GitLab removes carriage returns before rendering suggestion markup.
    let normalized = body.replace('\r', "");
    let mut fence: Option<(String, String)> = None;
    let mut suggestions = Vec::new();
    for event in Parser::new(&normalized) {
        match event {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info)))
                if info.starts_with("suggestion") =>
            {
                fence = Some((info.into_string(), String::new()))
            }
            Event::Text(text) => {
                if let Some((_, content)) = &mut fence {
                    content.push_str(&text);
                }
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some(value) = fence.take() {
                    suggestions.push(value);
                }
            }
            Event::Html(html) | Event::InlineHtml(html)
                if html.to_lowercase().contains("<pre")
                    || html.to_lowercase().contains("<code") =>
            {
                return Err(GitLabError::UnsupportedPosition);
            }
            _ => {}
        }
    }
    if fence.is_some() {
        return Err(GitLabError::Marker);
    }
    Ok(suggestions)
}

pub(crate) fn validate_no_suggestions(body: &str) -> Result<(), GitLabError> {
    if suggestions(body)?.is_empty() {
        Ok(())
    } else {
        Err(GitLabError::UnsupportedPosition)
    }
}

pub(crate) fn validate_suggestion(body: &str, finding: &Finding) -> Result<(), GitLabError> {
    let suggestions = suggestions(body)?;
    match (finding.suggestion(), suggestions.as_slice()) {
        (None, []) => Ok(()),
        (Some(suggestion), [(info, replacement)]) => {
            let actual = if info == "suggestion" {
                Some((0, 0))
            } else {
                info.strip_prefix("suggestion:-")
                    .and_then(|range| range.split_once('+'))
                    .and_then(|(above, below)| {
                        Some((above.parse::<u64>().ok()?, below.parse::<u64>().ok()?))
                    })
            };
            let anchor = finding.location().line().get();
            let expected = (
                anchor - suggestion.start_line().get(),
                suggestion.end_line().get() - anchor,
            );
            let intended = suggestion.replacement().join("\n");
            if !suggestion.replacement().is_empty() && intended.trim().is_empty() {
                return Err(GitLabError::UnsupportedPosition);
            }
            let parsed = if intended.is_empty() {
                String::new()
            } else {
                format!("{intended}\n")
            };
            if actual == Some(expected) && replacement == &parsed {
                Ok(())
            } else {
                Err(GitLabError::UnsupportedPosition)
            }
        }
        _ => Err(GitLabError::UnsupportedPosition),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use review_kit_domain::{
        Disposition, FindingDraft, FindingId, FindingStatus, LineNumber, PolicyId, Revision,
        Severity, SourceLocation, SourcePath, Suggestion,
    };

    #[test]
    fn slash_commands_are_rejected_after_gitlab_carriage_return_removal() {
        for body in [
            "Review\n\n/close",
            "Review\n/APPROVE ",
            "\r/close",
            "/cl\rose",
            "<div>\n/close",
            "future action\n/new_command argument",
            "`start\n/close\nend`",
            "``/close `\n/approve\ntext` ``",
        ] {
            assert_eq!(validate_quick_actions(body), Err(GitLabError::UnsafeBody));
        }
    }

    #[test]
    fn literal_code_preserves_slash_prefixed_lines() {
        for body in [
            "```rust\n/close\n```",
            "~~~suggestion\n/close\n~~~",
            "    /close",
            "\t/close",
            "`/close`",
            "> /close",
        ] {
            assert!(
                validate_quick_actions(body).is_ok(),
                "literal body was rejected: {body}"
            );
        }
    }

    fn finding(replacement: Option<Vec<String>>) -> Finding {
        let location = SourceLocation::new(
            SourcePath::try_from("file.rs".to_owned()).unwrap(),
            LineNumber::try_from(2).unwrap(),
        );
        let suggestion = replacement.map(|replacement| {
            Suggestion::new(
                location.clone(),
                Revision::new("1".repeat(40), vec!["2".repeat(40), "3".repeat(40)]).unwrap(),
                LineNumber::try_from(1).unwrap(),
                vec!["one".into(), "two".into(), "three".into()],
                replacement,
                "Verified correction".into(),
            )
            .unwrap()
        });
        Finding::new(FindingDraft {
            id: FindingId::try_from(1).unwrap(),
            policy_id: PolicyId::try_from("policy".to_owned()).unwrap(),
            title: "Preserve the result".into(),
            body: "The result disappears".into(),
            severity: Severity::P1,
            disposition: Disposition::Advisory,
            status: FindingStatus::Open,
            evidence: "Observed a dropped value".into(),
            validated: true,
            location,
            suggestion,
        })
        .unwrap()
    }

    #[test]
    fn nested_extra_missing_or_wrong_suggestions_are_rejected() {
        let with = finding(Some(vec!["new".into()]));
        let without = finding(None);
        let valid = "```suggestion:-1+1\nnew\n```";
        assert!(validate_suggestion(valid, &with).is_ok());
        assert!(
            validate_suggestion(
                &format!("{valid}\n\n> ```suggestion\n> unexpected\n> ```"),
                &with
            )
            .is_err()
        );
        assert!(validate_suggestion("> ```suggestion\n> unexpected\n> ```", &without).is_err());
        for invalid in [
            "No block",
            "```suggestion\nnew\n```",
            "```suggestion:-1+1\nother\n```",
            "<pre lang=\"suggestion\">new</pre>",
        ] {
            assert!(validate_suggestion(invalid, &with).is_err());
        }
    }

    #[test]
    fn long_fences_and_deletion_preserve_replacement_intent() {
        assert!(
            validate_suggestion(
                "````suggestion:-1+1\n```\n````",
                &finding(Some(vec!["```".into()]))
            )
            .is_ok()
        );
        assert!(validate_suggestion("```suggestion:-1+1\n```", &finding(Some(vec![]))).is_ok());
        assert!(
            validate_suggestion(
                "```suggestion:-1+1\n \n```",
                &finding(Some(vec![" ".into()]))
            )
            .is_err()
        );
    }
}
