use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

use crate::mail::parser::ParsedEmail;

pub struct ThreadRef {
    pub thread_id: Uuid,
    pub message_ids: Vec<String>,
    pub subject: String,
    pub last_message_at: DateTime<Utc>,
}

pub enum ThreadMatch {
    Existing(Uuid),
    New,
}

pub fn assign_thread(message: &ParsedEmail, existing_threads: &[ThreadRef]) -> ThreadMatch {
    if let Some(ref reply_to) = message.in_reply_to {
        for thread in existing_threads {
            if thread.message_ids.iter().any(|id| id == reply_to) {
                return ThreadMatch::Existing(thread.thread_id);
            }
        }
    }

    // Prefer later reference IDs — most likely to be the direct parent
    for ref_id in message.references.iter().rev() {
        for thread in existing_threads {
            if thread.message_ids.iter().any(|id| id == ref_id) {
                return ThreadMatch::Existing(thread.thread_id);
            }
        }
    }

    if let Some(ref subject) = message.subject {
        let normalized = normalize_subject(subject);
        if !normalized.is_empty() {
            let now = Utc::now();
            let cutoff = now - Duration::days(7);

            let best = existing_threads
                .iter()
                .filter(|t| {
                    normalize_subject(&t.subject) == normalized && t.last_message_at > cutoff
                })
                .max_by_key(|t| t.last_message_at);

            if let Some(thread) = best {
                return ThreadMatch::Existing(thread.thread_id);
            }
        }
    }

    ThreadMatch::New
}

pub fn normalize_subject(subject: &str) -> String {
    let mut s = subject.trim();
    loop {
        let b = s.as_bytes();
        let skip = if b.len() >= 3 && b[..3].eq_ignore_ascii_case(b"re:") {
            3
        } else if b.len() >= 4 && b[..4].eq_ignore_ascii_case(b"fwd:") {
            4
        } else if b.len() >= 3 && b[..3].eq_ignore_ascii_case(b"fw:") {
            3
        } else {
            break;
        };
        s = s[skip..].trim_start();
    }
    s.to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_thread(id: Uuid, msg_ids: &[&str], subject: &str, days_ago: i64) -> ThreadRef {
        ThreadRef {
            thread_id: id,
            message_ids: msg_ids.iter().map(|s| s.to_string()).collect(),
            subject: subject.to_string(),
            last_message_at: Utc::now() - Duration::days(days_ago),
        }
    }

    fn make_message(
        in_reply_to: Option<&str>,
        references: &[&str],
        subject: Option<&str>,
    ) -> ParsedEmail {
        ParsedEmail {
            message_id: Some("test-msg@example.com".into()),
            in_reply_to: in_reply_to.map(String::from),
            references: references.iter().map(|s| s.to_string()).collect(),
            from: "test@example.com".into(),
            to: vec![],
            cc: vec![],
            subject: subject.map(String::from),
            text_body: None,
            html_body: None,
            date: None,
            raw_headers: serde_json::Value::Null,
        }
    }

    #[test]
    fn test_assign_thread_in_reply_to_exact_match() {
        let tid = Uuid::new_v4();
        let threads = vec![make_thread(tid, &["msg001@ex.com"], "Hello", 1)];
        let msg = make_message(Some("msg001@ex.com"), &[], Some("Re: Hello"));
        match assign_thread(&msg, &threads) {
            ThreadMatch::Existing(id) => assert_eq!(id, tid),
            ThreadMatch::New => panic!("expected Existing"),
        }
    }

    #[test]
    fn test_assign_thread_in_reply_to_no_match_falls_through() {
        let tid = Uuid::new_v4();
        let threads = vec![make_thread(tid, &["msg001@ex.com"], "Hello", 1)];
        let msg = make_message(
            Some("nomatch@ex.com"),
            &["msg001@ex.com"],
            Some("Re: Hello"),
        );
        match assign_thread(&msg, &threads) {
            ThreadMatch::Existing(id) => assert_eq!(id, tid),
            ThreadMatch::New => panic!("expected Existing via references"),
        }
    }

    #[test]
    fn test_assign_thread_references_last_id_matches() {
        let tid = Uuid::new_v4();
        let threads = vec![make_thread(tid, &["ref002@ex.com"], "Test", 1)];
        let msg = make_message(None, &["ref001@ex.com", "ref002@ex.com"], Some("Re: Test"));
        match assign_thread(&msg, &threads) {
            ThreadMatch::Existing(id) => assert_eq!(id, tid),
            ThreadMatch::New => panic!("expected Existing"),
        }
    }

    #[test]
    fn test_assign_thread_references_middle_id_matches() {
        let tid = Uuid::new_v4();
        let threads = vec![make_thread(tid, &["ref002@ex.com"], "Test", 1)];
        let msg = make_message(
            None,
            &["ref001@ex.com", "ref002@ex.com", "ref003@ex.com"],
            Some("Re: Test"),
        );
        match assign_thread(&msg, &threads) {
            ThreadMatch::Existing(id) => assert_eq!(id, tid),
            ThreadMatch::New => panic!("expected Existing via middle ref"),
        }
    }

    #[test]
    fn test_assign_thread_in_reply_to_wins_over_references() {
        let tid1 = Uuid::new_v4();
        let tid2 = Uuid::new_v4();
        let threads = vec![
            make_thread(tid1, &["irt@ex.com"], "Hello", 1),
            make_thread(tid2, &["ref@ex.com"], "Hello", 1),
        ];
        let msg = make_message(Some("irt@ex.com"), &["ref@ex.com"], Some("Re: Hello"));
        match assign_thread(&msg, &threads) {
            ThreadMatch::Existing(id) => assert_eq!(id, tid1),
            ThreadMatch::New => panic!("expected Existing via In-Reply-To"),
        }
    }

    #[test]
    fn test_assign_thread_subject_match_within_7_days() {
        let tid = Uuid::new_v4();
        let threads = vec![make_thread(tid, &["old@ex.com"], "hello", 3)];
        let msg = make_message(None, &[], Some("Re: Hello"));
        match assign_thread(&msg, &threads) {
            ThreadMatch::Existing(id) => assert_eq!(id, tid),
            ThreadMatch::New => panic!("expected subject match"),
        }
    }

    #[test]
    fn test_assign_thread_subject_match_beyond_7_days_returns_new() {
        let tid = Uuid::new_v4();
        let threads = vec![make_thread(tid, &["old@ex.com"], "hello", 30)];
        let msg = make_message(None, &[], Some("Re: Hello"));
        match assign_thread(&msg, &threads) {
            ThreadMatch::New => {}
            ThreadMatch::Existing(_) => panic!("expected New for stale subject"),
        }
    }

    #[test]
    fn test_normalize_subject_re() {
        assert_eq!(normalize_subject("Re: Hello"), "hello");
    }

    #[test]
    fn test_normalize_subject_repeated_re() {
        assert_eq!(normalize_subject("RE: RE: Hello"), "hello");
    }

    #[test]
    fn test_normalize_subject_fwd_chain() {
        assert_eq!(normalize_subject("Fwd: FW: Fw: Test"), "test");
    }

    #[test]
    fn test_normalize_subject_mixed_prefixes() {
        assert_eq!(normalize_subject("Re: Fwd: Re: Mixed"), "mixed");
    }

    #[test]
    fn test_normalize_subject_spaces() {
        assert_eq!(normalize_subject("  Re:  spaces  "), "spaces");
    }

    #[test]
    fn test_normalize_subject_no_prefix() {
        assert_eq!(normalize_subject("No prefix here"), "no prefix here");
    }

    #[test]
    fn test_normalize_subject_empty() {
        assert_eq!(normalize_subject(""), "");
    }

    #[test]
    fn test_normalize_subject_only_prefix() {
        assert_eq!(normalize_subject("Re: "), "");
    }

    #[test]
    fn test_normalize_subject_no_space_after_colon() {
        assert_eq!(normalize_subject("Re:Re:no space"), "no space");
    }

    #[test]
    fn test_assign_thread_case_insensitive_subject() {
        let tid = Uuid::new_v4();
        let threads = vec![make_thread(tid, &[], "hello", 1)];
        let msg = make_message(None, &[], Some("HELLO"));
        match assign_thread(&msg, &threads) {
            ThreadMatch::Existing(id) => assert_eq!(id, tid),
            ThreadMatch::New => panic!("expected case-insensitive subject match"),
        }
    }

    #[test]
    fn test_assign_thread_no_match_returns_new() {
        let threads = vec![make_thread(Uuid::new_v4(), &["x@ex.com"], "other topic", 1)];
        let msg = make_message(None, &[], Some("Completely Different"));
        match assign_thread(&msg, &threads) {
            ThreadMatch::New => {}
            ThreadMatch::Existing(_) => panic!("expected New"),
        }
    }

    #[test]
    fn test_assign_thread_empty_references_no_panic() {
        let threads = vec![make_thread(Uuid::new_v4(), &["x@ex.com"], "Test", 1)];
        let msg = make_message(None, &[], Some("Unrelated"));
        match assign_thread(&msg, &threads) {
            ThreadMatch::New => {}
            ThreadMatch::Existing(_) => panic!("expected New"),
        }
    }

    #[test]
    fn test_assign_thread_empty_threads_returns_new() {
        let msg = make_message(Some("x@ex.com"), &["y@ex.com"], Some("Hello"));
        match assign_thread(&msg, &[]) {
            ThreadMatch::New => {}
            ThreadMatch::Existing(_) => panic!("expected New with empty threads"),
        }
    }

    #[test]
    fn test_assign_thread_multiple_subject_matches_prefers_most_recent() {
        let tid_old = Uuid::new_v4();
        let tid_new = Uuid::new_v4();
        let threads = vec![
            make_thread(tid_old, &[], "hello", 5),
            make_thread(tid_new, &[], "hello", 1),
        ];
        let msg = make_message(None, &[], Some("Hello"));
        match assign_thread(&msg, &threads) {
            ThreadMatch::Existing(id) => assert_eq!(id, tid_new),
            ThreadMatch::New => panic!("expected most recent thread"),
        }
    }

    #[test]
    fn test_assign_thread_none_subject_does_not_match_empty_thread_subject() {
        let tid = Uuid::new_v4();
        let threads = vec![make_thread(tid, &[], "", 1)];
        let msg = make_message(None, &[], None);
        match assign_thread(&msg, &threads) {
            ThreadMatch::New => {}
            ThreadMatch::Existing(_) => panic!("None subject should not match empty string"),
        }
    }

    #[test]
    fn test_assign_thread_empty_subject_does_not_match() {
        let tid = Uuid::new_v4();
        let threads = vec![make_thread(tid, &[], "", 1)];
        let msg = make_message(None, &[], Some(""));
        match assign_thread(&msg, &threads) {
            ThreadMatch::New => {}
            ThreadMatch::Existing(_) => panic!("empty subject should not match"),
        }
    }
}
