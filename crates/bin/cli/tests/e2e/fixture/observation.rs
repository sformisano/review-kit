use serde_json::Value;

pub fn authored_notes(observed: &Value) -> Vec<Value> {
    observed["notes"]
        .as_array()
        .expect("observed provider notes")
        .iter()
        .filter(|note| note["system"] != true)
        .map(authored_note)
        .collect()
}

pub fn authored_discussions(observed: &Value) -> Vec<Value> {
    observed["discussions"]
        .as_array()
        .expect("observed provider discussions")
        .iter()
        .filter_map(|discussion| {
            let notes = authored_notes(discussion);
            if notes.is_empty() {
                return None;
            }
            let mut authored = discussion.clone();
            authored["notes"] = notes.into();
            Some(authored)
        })
        .collect()
}

fn authored_note(note: &Value) -> Value {
    let mut authored = note.as_object().expect("observed provider note").clone();
    // GitLab remaps positions and creates system annotations asynchronously after a push.
    // Publication cannot edit an existing note's position; creation checks inspect raw positions.
    authored.remove("position");
    authored.into()
}

#[cfg(test)]
mod tests {
    use super::{authored_discussions, authored_notes};
    use serde_json::{Value, json};

    #[test]
    fn observations_keep_every_author_and_exclude_system_annotations() {
        let system = json!({"id": 1, "author": {"id": 17}, "system": true});
        let author = json!({"id": 2, "author": {"id": 17}, "system": false});
        let reviewer = json!({"id": 3, "author": {"id": 23}, "system": false});
        let foreign = json!({"id": 4, "author": {"id": 29}, "system": false});
        let observed = json!({
            "notes": [system, author, reviewer, foreign],
            "discussions": [
                {"id": "system", "notes": [system]},
                {"id": "author", "notes": [author]},
                {"id": "reviewer", "notes": [reviewer]},
                {"id": "foreign", "notes": [foreign]}
            ]
        });
        assert_eq!(
            authored_notes(&observed),
            vec![
                observed["notes"][1].clone(),
                observed["notes"][2].clone(),
                observed["notes"][3].clone()
            ]
        );
        assert_eq!(
            authored_discussions(&observed),
            vec![
                observed["discussions"][1].clone(),
                observed["discussions"][2].clone(),
                observed["discussions"][3].clone()
            ]
        );
        assert_eq!(observed["notes"].as_array().unwrap().len(), 4);
        assert_eq!(observed["discussions"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn delayed_system_annotation_does_not_change_authored_observations() {
        let before = json!({"notes": [], "discussions": []});
        let after = json!({
            "notes": [{"id": 1, "system": true}],
            "discussions": [{"id": "branch", "notes": [{"id": 1, "system": true}]}]
        });
        assert_eq!(authored_notes(&before), authored_notes(&after));
        assert_eq!(authored_discussions(&before), authored_discussions(&after));
    }

    #[test]
    fn missing_system_flag_and_mixed_discussions_are_preserved() {
        let observed = json!({
            "notes": [{"id": 2, "body": "foreign reply"}],
            "discussions": [{"id": "mixed", "notes": [
                {"id": 1, "system": true}, {"id": 2, "body": "foreign reply"}
            ]}]
        });
        assert_eq!(
            authored_notes(&observed),
            vec![observed["notes"][0].clone()]
        );
        assert_eq!(
            authored_discussions(&observed),
            vec![json!({"id": "mixed", "notes": [{"id": 2, "body": "foreign reply"}]})]
        );
        assert_eq!(
            observed["discussions"][0]["notes"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    fn observed_note(note: Value) -> Value {
        json!({"notes": [note], "discussions": [{"id": "review", "notes": [note]}]})
    }

    #[test]
    fn delayed_diff_tracking_does_not_change_authored_effects() {
        let before = observed_note(json!({
            "id": 1, "system": false, "body": "finding", "resolved": false,
            "position": {"head_sha": "old", "new_line": 3}
        }));
        let mut after = before.clone();
        after["notes"][0]["position"] = json!({"head_sha": "new", "new_line": 5});
        after["discussions"][0]["notes"][0] = after["notes"][0].clone();
        let annotation = json!({"id": 2, "system": true, "body": "changed this line"});
        after["notes"]
            .as_array_mut()
            .unwrap()
            .push(annotation.clone());
        after["discussions"][0]["notes"]
            .as_array_mut()
            .unwrap()
            .push(annotation);
        assert_eq!(authored_notes(&before), authored_notes(&after));
        assert_eq!(authored_discussions(&before), authored_discussions(&after));
    }

    #[test]
    fn authored_effect_changes_remain_observable() {
        let note = json!({
            "id": 1, "system": false, "body": "finding", "author": {"id": 23},
            "resolved": false, "suggestions": [{"to_line": 3}],
            "position": {"head_sha": "old", "new_line": 3}
        });
        let before = observed_note(note.clone());
        for (field, replacement) in [
            ("id", json!(2)),
            ("body", json!("edited")),
            ("author", json!({"id": 29})),
            ("resolved", json!(true)),
            ("suggestions", json!([{ "to_line": 4 }])),
        ] {
            let mut changed = note.clone();
            changed[field] = replacement;
            let after = observed_note(changed);
            assert_ne!(authored_notes(&before), authored_notes(&after), "{field}");
            assert_ne!(
                authored_discussions(&before),
                authored_discussions(&after),
                "{field}"
            );
        }
        let mut additional = before.clone();
        let reply = json!({"id": 2, "body": "reply", "system": false});
        additional["notes"]
            .as_array_mut()
            .unwrap()
            .push(reply.clone());
        additional["discussions"][0]["notes"]
            .as_array_mut()
            .unwrap()
            .push(reply);
        assert_ne!(authored_notes(&before), authored_notes(&additional));
        assert_ne!(
            authored_discussions(&before),
            authored_discussions(&additional)
        );
        let empty = json!({"notes": [], "discussions": []});
        assert_ne!(authored_notes(&before), authored_notes(&empty));
        assert_ne!(authored_discussions(&before), authored_discussions(&empty));
        let mut replaced = before.clone();
        replaced["discussions"][0]["id"] = json!("replacement");
        assert_ne!(
            authored_discussions(&before),
            authored_discussions(&replaced)
        );
    }
}
