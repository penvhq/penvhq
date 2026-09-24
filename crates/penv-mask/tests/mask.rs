use penv_mask::{BLOCKS, Masker, redaction};

const SECRET: &str = "sk_test_FAKE0000";

fn through(secrets: &[&str], chunks: &[&[u8]]) -> String {
    let mut masker = Masker::new(secrets.iter().map(|s| s.to_string()).collect());
    let mut out = Vec::new();
    for chunk in chunks {
        masker.feed(chunk, &mut out);
    }
    masker.finish(&mut out);
    String::from_utf8(out).expect("the scrubber only ever splits on pattern boundaries")
}

fn one(secrets: &[&str], input: &str) -> String {
    through(secrets, &[input.as_bytes()])
}

#[test]
fn a_secret_is_replaced_by_its_first_two_characters_and_blocks() {
    assert_eq!(redaction(SECRET), format!("sk{BLOCKS}"));
    assert_eq!(
        one(&[SECRET], &format!("token={SECRET}\n")),
        format!("token=sk{BLOCKS}\n")
    );
}

#[test]
fn a_secret_split_at_any_byte_is_still_caught() {
    let line = format!("before {SECRET} after");
    let start = line.find(SECRET).unwrap();
    for split in start..start + SECRET.len() {
        let (head, tail) = line.split_at(split);
        let out = through(&[SECRET], &[head.as_bytes(), tail.as_bytes()]);
        assert!(!out.contains(SECRET), "split at {split}: {out}");
        assert_eq!(out, format!("before sk{BLOCKS} after"), "split at {split}");
    }
}

#[test]
fn a_secret_split_one_byte_at_a_time_is_still_caught() {
    let chunks: Vec<Vec<u8>> = format!("a{SECRET}b").bytes().map(|b| vec![b]).collect();
    let borrowed: Vec<&[u8]> = chunks.iter().map(|c| c.as_slice()).collect();
    assert_eq!(through(&[SECRET], &borrowed), format!("ask{BLOCKS}b"));
}

#[test]
fn a_secret_that_is_a_prefix_of_another_does_not_win() {
    let short = "sk_test_FAKE";
    let long = "sk_test_FAKE0000";
    let out = one(&[short, long], &format!("{long}\n"));
    assert!(!out.contains(short), "{out}");
    assert_eq!(out, format!("sk{BLOCKS}\n"));

    // The order the secrets arrive in does not change which one matches.
    assert_eq!(one(&[long, short], &format!("{long}\n")), out);
}

#[test]
fn overlapping_secrets_both_disappear() {
    let out = one(&["abcdef00", "ef0011223"], "xxabcdef0011223yy");
    assert!(!out.contains("abcdef00"), "{out}");
    assert!(!out.contains("ef0011223"), "{out}");
    assert!(out.starts_with("xx") && out.ends_with("yy"));
}

#[test]
fn the_base64_forms_are_caught_too() {
    // Padded and unpadded, in both alphabets. Computed here, not hard-coded.
    let value = "pw?A_FAKE1234~";
    let forms = [
        "cHc/QV9GQUtFMTIzNH4=",
        "cHc/QV9GQUtFMTIzNH4",
        "cHc_QV9GQUtFMTIzNH4=",
        "cHc_QV9GQUtFMTIzNH4",
    ];
    for form in forms {
        let out = one(&[value], &format!("body {form} end"));
        assert_eq!(out, format!("body pw{BLOCKS} end"), "{form}");
    }
}

#[test]
fn the_json_escaped_form_is_caught_too() {
    let value = "ab\"c\\FAKE";
    let escaped = "ab\\\"c\\\\FAKE";
    let out = one(&[value], &format!("{{\"k\":\"{escaped}\"}}"));
    assert_eq!(out, format!("{{\"k\":\"ab{BLOCKS}\"}}"));

    // A value that needs no escaping still matches as written.
    assert_eq!(one(&[SECRET], SECRET), format!("sk{BLOCKS}"));
}

#[test]
fn the_json_forms_other_encoders_write_are_caught_too() {
    let value = "a/b<c>&d\u{e9}\u{1f511}\u{1b}FAKE";
    for encoded in [
        // PHP and some Java encoders escape the slash.
        "a\\/b<c>&d\u{e9}\u{1f511}\\u001bFAKE",
        // Go escapes the HTML characters.
        "a/b\\u003cc\\u003e\\u0026d\u{e9}\u{1f511}\\u001bFAKE",
        // Python's ensure_ascii, a surrogate pair above the BMP.
        "a/b<c>&d\\u00e9\\ud83d\\udd11\\u001bFAKE",
        // Upper-case hex digits.
        "a/b<c>&d\\u00E9\\uD83D\\uDD11\\u001BFAKE",
        "a\\/b\\u003Cc\\u003E\\u0026d\\u00E9\\uD83D\\uDD11\\u001BFAKE",
    ] {
        let out = one(&[value], &format!("{{\"k\":\"{encoded}\"}}"));
        assert_eq!(out, format!("{{\"k\":\"a/{BLOCKS}\"}}"), "{encoded}");
    }
}

#[test]
fn overlapping_secrets_leave_nothing_of_either_behind() {
    let out = one(&["abcdef00", "ef0011223"], "xxabcdef0011223yy");
    assert_eq!(out, format!("xxab{BLOCKS}yy"));
    // A chain of overlaps is one masked run.
    let out = one(&["abcd0000", "0000efgh", "efghijkl"], "<abcd0000efghijkl>");
    assert_eq!(out, format!("<ab{BLOCKS}>"));
    // The same, however the stream was split.
    let input = "xxabcdef0011223yy";
    for split in 0..input.len() {
        let (head, tail) = input.split_at(split);
        let out = through(
            &["abcdef00", "ef0011223"],
            &[head.as_bytes(), tail.as_bytes()],
        );
        assert_eq!(out, format!("xxab{BLOCKS}yy"), "split at {split}");
    }
}

#[test]
fn short_secrets_are_left_alone() {
    // Masking "1" would destroy the output; the design would rather print it.
    let out = one(&["1", "on", "abc"], "port 1 mode on abc");
    assert_eq!(out, "port 1 mode on abc");
}

#[test]
fn an_empty_secret_list_is_a_pass_through() {
    let mut masker = Masker::new(Vec::new());
    assert!(masker.is_pass_through());
    let mut out = Vec::new();
    masker.feed(b"anything at all", &mut out);
    masker.finish(&mut out);
    assert_eq!(String::from_utf8(out).unwrap(), "anything at all");
}

#[test]
fn interleaved_chunks_keep_the_rest_of_the_stream_intact() {
    let out = through(
        &[SECRET, "hunter2_FAKE"],
        &[
            b"line one\n",
            b"sk_test_",
            b"FAKE0000 and hun",
            b"ter2_FAKE\n",
            b"line three\n",
        ],
    );
    assert_eq!(
        out,
        format!("line one\nsk{BLOCKS} and hu{BLOCKS}\nline three\n")
    );
}

#[test]
fn nothing_is_held_back_once_the_stream_ends() {
    let mut masker = Masker::new(vec![SECRET.to_string()]);
    let mut out = Vec::new();
    masker.feed(b"sk_test_FAKE00", &mut out);
    assert!(!out.ends_with(b"00"), "a possible prefix is held back");
    masker.finish(&mut out);
    assert_eq!(String::from_utf8(out).unwrap(), "sk_test_FAKE00");
}

#[test]
fn a_megabyte_through_twenty_secrets_is_quick() {
    let secrets: Vec<String> = (0..20).map(|i| format!("sk_test_FAKE{i:04}")).collect();
    let mut masker = Masker::new(secrets.clone());
    let block = format!(
        "{}\n",
        "the quick brown fox jumps over the lazy dog. ".repeat(4)
    );
    let mut input = String::with_capacity(1 << 20);
    while input.len() < (1 << 20) {
        input.push_str(&block);
        input.push_str(&secrets[input.len() % 20]);
    }

    let started = std::time::Instant::now();
    let mut out = Vec::with_capacity(input.len() + 4096);
    for chunk in input.as_bytes().chunks(8192) {
        masker.feed(chunk, &mut out);
    }
    masker.finish(&mut out);
    let elapsed = started.elapsed();

    let text = String::from_utf8(out).unwrap();
    for secret in &secrets {
        assert!(!text.contains(secret.as_str()), "{secret} survived");
    }
    assert!(elapsed.as_secs_f64() < 1.0, "took {elapsed:?}");
}

/// A base64 encoder for the test only: the crate's own is private, and a test
/// that reuses it would prove nothing.
fn b64(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for group in input.chunks(3) {
        let n = (group[0] as u32) << 16
            | (*group.get(1).unwrap_or(&0) as u32) << 8
            | *group.get(2).unwrap_or(&0) as u32;
        for shift in [18, 12, 6, 0].iter().take(group.len() + 1) {
            out.push(ALPHABET[((n >> shift) & 63) as usize] as char);
        }
        for _ in group.len() + 1..4 {
            out.push('=');
        }
    }
    out
}

#[test]
fn the_hex_forms_are_caught_in_both_cases() {
    let value = "sk_test_FAKE0000";
    let lower: String = value.bytes().map(|b| format!("{b:02x}")).collect();
    let upper = lower.to_uppercase();
    for form in [lower, upper] {
        let out = one(&[value], &format!("hex {form} end"));
        assert_eq!(out, format!("hex sk{BLOCKS} end"), "{form}");
    }
}

#[test]
fn the_percent_encoded_form_is_caught_in_both_cases() {
    let value = "pw?A_FAKE1234~";
    for form in ["pw%3FA_FAKE1234~", "pw%3fA_FAKE1234~"] {
        let out = one(&[value], &format!("url=https://example.test/?q={form}"));
        assert_eq!(
            out,
            format!("url=https://example.test/?q=pw{BLOCKS}"),
            "{form}"
        );
    }
}

#[test]
fn a_secret_base64ed_inside_a_larger_body_is_caught_at_every_phase() {
    let secret = "sk_test_FAKE0000";
    // One prefix per phase: the secret starts at byte 0, 1 and 2 of a group.
    for prefix in ["", "k", "k="] {
        let blob = b64(format!("{prefix}{secret};tail").as_bytes());
        let out = one(&[secret], &format!("body {blob} end"));
        assert!(!out.contains(&blob), "phase {}: {out}", prefix.len());
        assert!(out.contains(BLOCKS), "phase {}: {out}", prefix.len());
    }
}

#[test]
fn base64_wrapped_across_lines_is_still_caught() {
    let secret = "sk_test_FAKE0000";
    let encoded = b64(secret.as_bytes());
    let wrapped: String = encoded
        .as_bytes()
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join("\r\n");
    let out = one(&[secret], &format!("body\n{wrapped}\nend\n"));
    assert!(!out.contains(&encoded), "{out}");
    assert!(!out.contains("QUtF"), "a wrapped group survived: {out}");
    assert!(
        out.starts_with("body\n") && out.ends_with("\nend\n"),
        "{out}"
    );
}

#[test]
fn a_chunk_that_cannot_grow_into_a_secret_is_written_at_once() {
    let mut masker = Masker::new(vec!["sk_test_FAKE0000".to_string()]);
    let mut out = Vec::new();
    masker.feed(b"Password: ", &mut out);
    assert_eq!(
        String::from_utf8(out).unwrap(),
        "Password: ",
        "an interactive prompt must not wait for the next chunk"
    );
}

#[test]
fn a_secret_split_across_chunks_is_still_held_until_it_is_whole() {
    let secret = "sk_test_FAKE0000";
    let mut masker = Masker::new(vec![secret.to_string()]);
    let mut out = Vec::new();
    masker.feed(b"user: sk_test_", &mut out);
    assert_eq!(String::from_utf8(out.clone()).unwrap(), "user: ");
    masker.feed(b"FAKE0000\n", &mut out);
    masker.finish(&mut out);
    assert_eq!(
        String::from_utf8(out).unwrap(),
        format!("user: sk{BLOCKS}\n")
    );
}
