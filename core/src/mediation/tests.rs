use super::*;

const WEEK: i64 = 7 * 86_400_000;
const NOW: i64 = 1_763_445_600_000;

fn request(file: &str, id: &str, from: &str, team: &str) -> Value {
    json!({"fileId":file,"type":"request","id":id,"from":from,"fromName":"","team":team,"teamName":"팀","text":"회의 자료를 하루 전에 공유해 줄 수 있을까?","createdAt":NOW})
}

#[test]
fn receiver_caps_each_sender_per_week_and_honours_mutes() {
    let mut book = json!({});
    let items: Vec<Value> = (0..4).map(|i| request(&format!("f{i}"), &format!("m{i}"), "B@Example.com", "team1")).collect();
    assert_eq!(accept(&mut book, &items, &[], NOW), (RECEIVE_LIMIT, 0, 4 - RECEIVE_LIMIT));
    // Seen files are not counted again, even the held ones.
    assert_eq!(accept(&mut book, &items, &[], NOW), (0, 0, 0));
    // A new week opens the limit again; another sender has their own limit.
    let next = [request("f9", "m9", "b@example.com", "team1"), request("f8", "m8", "c@example.com", "team1")];
    assert_eq!(accept(&mut book, &next, &[], NOW + WEEK), (2, 0, 0));
    // Muting a person (any letter case) or a whole team drops their requests.
    set_muted(&mut book, "person", "C@EXAMPLE.COM", true).unwrap();
    set_muted(&mut book, "team", "team2", true).unwrap();
    let muted = [request("f10", "m10", "c@example.com", "team1"), request("f11", "m11", "d@example.com", "team2")];
    assert_eq!(accept(&mut book, &muted, &[], NOW + WEEK), (0, 0, 2));
    set_muted(&mut book, "person", "c@example.com", false).unwrap();
    assert!(!is_muted(&book, "c@example.com", "team1"));
    assert!(set_muted(&mut book, "other", "x", true).is_err());
    // Rejected files (failed the roster check) are remembered so they are not fetched again.
    accept(&mut book, &[], &["bad".into()], NOW);
    assert!(seen(&book).contains(&"bad".to_owned()));
}

#[test]
fn reactions_only_count_from_the_person_the_request_went_to() {
    let mut book = json!({});
    record_sent(&mut book, json!({"id":"s1","to":"b@example.com","toName":"비","team":"team1","text":"부탁","at":NOW,"reaction":null}));
    let reaction = |file: &str, from: &str, re: &str, r: &str| {
        json!({"fileId":file,"type":"reaction","id":"r","re":re,"from":from,"team":"team1","reaction":r})
    };
    let items = [
        reaction("x1", "c@example.com", "s1", "ok"),  // someone else
        reaction("x2", "b@example.com", "s2", "ok"),  // unknown request
        reaction("x3", "b@example.com", "s1", "bad"), // unknown reaction
        reaction("x4", "B@example.com", "s1", "talk"),
        reaction("x5", "b@example.com", "s1", "ok"), // already answered
    ];
    assert_eq!(accept(&mut book, &items, &[], NOW), (0, 1, 0));
    let row = next_reaction(&book).unwrap();
    assert_eq!(row["reaction"], "talk");
    assert_eq!(reaction_bubble(&row), "비님이 전한 부탁에 “이야기해 보고 싶어”라고 답했어.");
    mark(&mut book, "sent", "s1", "noticed", json!(true));
    assert!(next_reaction(&book).is_none());
}

#[test]
fn sender_limit_bubbles_and_view() {
    let mut book = json!({});
    for i in 0..SEND_LIMIT {
        can_send(&book, "b@example.com", NOW).unwrap();
        record_sent(&mut book, json!({"id":format!("s{i}"),"to":"b@example.com","team":"t","text":"부탁","at":NOW,"reaction":null}));
    }
    assert!(can_send(&book, "B@example.com", NOW).is_err());
    assert!(can_send(&book, "c@example.com", NOW).is_ok());
    assert!(can_send(&book, "b@example.com", NOW + WEEK).is_ok());

    accept(&mut book, &[request("f1", "m1", "kim@example.com", "t")], &[], NOW);
    let row = next_request(&book).unwrap();
    let bubble = request_bubble(&row, "사이");
    assert!(bubble.starts_with("kim님이 사이에게 부탁한 말이야. 사이가 부드럽게 다듬었어."));
    assert!(bubble.contains("회의 자료를"));
    mark(&mut book, "inbox", "m1", "delivered", json!(true));
    assert!(next_request(&book).is_none());

    let v = view(&book);
    assert_eq!(v["inbox"][0]["id"], "m1");
    assert!(v["inbox"][0].get("fileId").is_none());
    assert_eq!(v["sent"].as_array().unwrap().len(), SEND_LIMIT);

    assert!(valid_text("자료를 미리 공유해 줄래?\n고마워."));
    for bad in ["", "   ", "숨은\u{202e}문자", "제어\u{7}"] {
        assert!(!valid_text(bad), "{bad:?}");
    }
    assert!(!valid_text(&"가".repeat(MAX_TEXT + 1)));
}
