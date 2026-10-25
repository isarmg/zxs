use super::*;
use json_codec::json;
fn response(count: usize) -> Value {
    json!({"entries":vec![json!({"entry_id":"id"});count],"total_entries":"100000","snapshot":"snapshot","next_cursor":"next","previous_cursor":null})
}
#[test]
fn bounds_the_retained_page_independently_of_directory_size() {
    assert_eq!(Page::parse(&response(200)).unwrap().total, 100000);
    assert!(Page::parse(&response(201)).is_err());
}
#[test]
fn rejects_missing_or_malformed_pagination_instead_of_displaying_partial_success() {
    for key in [
        "entries",
        "total_entries",
        "snapshot",
        "next_cursor",
        "previous_cursor",
    ] {
        let mut value = response(1);
        value.as_object_mut().unwrap().remove(key);
        assert!(Page::parse(&value).is_err(), "{key}");
    }
    let mut value = response(1);
    value["next_cursor"] = "".into();
    assert!(Page::parse(&value).is_err());
    let mut value = response(2);
    value["total_entries"] = "1".into();
    assert!(Page::parse(&value).is_err());
}

#[test]
fn selected_identity_is_independent_of_page_and_rejects_replacement() {
    let requested = json!({"entry_id":"old","path":"dir/item"});
    let page = json!({"entries":[],"selected_entry":requested.clone()});
    assert_eq!(
        selected(&page, Some(&requested)).unwrap(),
        Some(requested.clone())
    );
    assert!(
        selected(
            &json!({"selected_entry":{"entry_id":"new","path":"dir/item"}}),
            Some(&requested)
        )
        .is_err()
    );
    assert!(selected(&json!({}), Some(&requested)).is_err());
    assert_eq!(
        selected(&json!({"selected_entry":null}), Some(&requested)).unwrap(),
        None
    );
}

#[test]
fn selected_identity_accepts_canonical_equivalence_but_not_aliases_or_malformed_paths() {
    let requested = json!({"entry_id":"stable","path":"Cafe\u{301}/A\u{30a}.txt"});
    let actual = json!({"entry_id":"stable","path":"Café/Å.txt"});
    assert_eq!(
        selected(&json!({"selected_entry":actual.clone()}), Some(&requested)).unwrap(),
        Some(actual.clone())
    );
    for rejected in [
        json!({"entry_id":"stable","path":"Other/Å.txt"}),
        json!({"entry_id":"recreated","path":"Café/Å.txt"}),
        json!({"entry_id":"stable"}),
        json!({"entry_id":"stable","path":null}),
        json!({"entry_id":"stable","path":7}),
    ] {
        assert!(selected(&json!({"selected_entry":rejected}), Some(&requested)).is_err());
    }
    for malformed in [
        json!({"entry_id":"stable"}),
        json!({"entry_id":"stable","path":null}),
        json!({"entry_id":"stable","path":7}),
    ] {
        assert!(selected(&json!({"selected_entry":actual.clone()}), Some(&malformed)).is_err());
        assert!(
            selected(
                &json!({"selected_entry":malformed.clone()}),
                Some(&malformed)
            )
            .is_err()
        );
    }
}
