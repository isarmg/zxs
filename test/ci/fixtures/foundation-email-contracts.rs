// Public email equality and hashing contract regressions.
mod email_public_contract_regressions {
    use email_address::{EmailAddress, Options};
    use std::collections::{HashMap, hash_map::DefaultHasher};
    use std::hash::{BuildHasherDefault, Hash, Hasher};
    use std::str::FromStr;

    #[derive(Debug, PartialEq, Eq)]
    enum HashWrite {
        Bytes(Vec<u8>),
        Byte(u8),
    }

    #[derive(Debug, Default)]
    struct RecordingHasher {
        writes: Vec<HashWrite>,
    }

    impl Hasher for RecordingHasher {
        fn finish(&self) -> u64 {
            // Call traces are the oracle; this observer is not used by HashMap.
            0
        }

        fn write(&mut self, bytes: &[u8]) {
            self.writes.push(HashWrite::Bytes(bytes.to_vec()));
        }

        fn write_u8(&mut self, byte: u8) {
            self.writes.push(HashWrite::Byte(byte));
        }
    }

    fn trace(address: &EmailAddress) -> Vec<HashWrite> {
        let mut hasher = RecordingHasher::default();
        address.hash(&mut hasher);
        hasher.writes
    }

    fn feed(address: &EmailAddress) -> Vec<u8> {
        let mut bytes = Vec::new();
        for write in trace(address) {
            match write {
                HashWrite::Bytes(chunk) => bytes.extend(chunk),
                HashWrite::Byte(byte) => bytes.push(byte),
            }
        }
        bytes
    }

    fn parse(value: &str) -> EmailAddress {
        EmailAddress::from_str(value)
            .expect("invariant: fixture input satisfies the current email grammar")
    }

    fn assert_equal_feed(left: &str, right: &str) {
        let left = parse(left);
        let right = parse(right);
        assert_eq!(left, right);
        assert_eq!(trace(&left), trace(&right));
    }

    #[test]
    fn parsed_ascii_domain_case_has_identical_hash_write_sequence() {
        assert_equal_feed("User@example.org", "User@EXAMPLE.ORG");
        assert_equal_feed("User@ExAmPlE.OrG", "User@eXaMpLe.oRg");
    }

    #[test]
    fn last_at_partition_preserves_quoted_local_and_display_prefix() {
        assert_equal_feed(r#""User@Tag"@example.org"#, r#""User@Tag"@EXAMPLE.ORG"#);
        assert_equal_feed("Desk <User@example.org>", "Desk <User@EXAMPLE.ORG>");
        assert_equal_feed(
            "Desk@Host <User@example.org>",
            "Desk@Host <User@EXAMPLE.ORG>",
        );
    }

    #[test]
    fn accepted_domain_literal_uses_the_existing_ascii_case_equality() {
        // The current parser accepts generic dtext, without IP normalization.
        assert_equal_feed("User@[IPv6:AbCd::1]", "User@[ipv6:aBcD::1]");
    }

    #[test]
    fn unicode_bytes_are_preserved_and_only_ascii_domain_case_is_folded() {
        assert_equal_feed("User@пример.Example.org", "User@пример.EXAMPLE.ORG");
        assert_equal_feed("User@ä.Example.org", "User@ä.EXAMPLE.ORG");
        assert_ne!(parse("User@ä.example.org"), parse("User@Ä.example.org"));
    }

    #[test]
    fn casefold_boundaries_do_not_depend_on_domain_capitalization() {
        // Domain byte lengths are 63/64/65 and 127/128/129; all labels fit 63 bytes.
        for label_len in [51, 52, 53] {
            let one_block = format!("{}.EXAMPLE.ORG", "A".repeat(label_len));
            let two_blocks = format!("{}.{}.EXAMPLE.ORG", "A".repeat(63), "B".repeat(label_len));
            for domain in [one_block, two_blocks] {
                assert_equal_feed(
                    &format!("User@{domain}"),
                    &format!("User@{}", domain.to_ascii_lowercase()),
                );
            }
        }

        let domain = format!(
            "{}.{}.{}.{}",
            "A".repeat(63),
            "B".repeat(63),
            "C".repeat(63),
            "D".repeat(62),
        );
        assert_eq!(domain.len(), 254);
        assert_equal_feed(
            &format!("User@{domain}"),
            &format!("User@{}", domain.to_ascii_lowercase()),
        );
    }

    #[test]
    fn public_construction_paths_share_the_same_equality_and_hash_contract() {
        let parsed = parse("User@EXAMPLE.ORG");
        let configured = EmailAddress::parse_with_options(
            "User@example.org",
            Options::default().with_required_tld(),
        )
        .expect("invariant: fixture satisfies the configured domain-segment rule");
        let unchecked = EmailAddress::new_unchecked(String::from("User@Example.Org"));
        let cloned = parsed.clone();
        for value in [configured, unchecked, cloned] {
            assert_eq!(parsed, value);
            assert_eq!(trace(&parsed), trace(&value));
        }
    }

    #[test]
    fn hashmap_reuses_equivalent_domains_and_retains_local_case_distinctions() {
        type EmailMap<V> = HashMap<EmailAddress, V, BuildHasherDefault<DefaultHasher>>;
        let mut values = EmailMap::default();
        assert_eq!(values.insert(parse("User@example.org"), 1), None);
        assert_eq!(values.get(&parse("User@EXAMPLE.ORG")), Some(&1));
        assert_eq!(values.insert(parse("User@Example.Org"), 2), Some(1));
        assert_eq!(values.len(), 1);
        assert_eq!(values.insert(parse("user@example.org"), 3), None);
        assert_eq!(values.len(), 2);
        assert_eq!(values.get(&parse("User@EXAMPLE.ORG")), Some(&2));
        assert_eq!(values.get(&parse("user@EXAMPLE.ORG")), Some(&3));
    }

    #[test]
    fn hashing_does_not_change_stored_spelling_or_equality() {
        let address = parse("Desk@Host <User@ExAmPlE.OrG>");
        let before = address.as_str().to_owned();
        let _ = trace(&address);
        assert_eq!(address.as_str(), before.as_str());
        assert_ne!(address, parse("desk@Host <User@ExAmPlE.OrG>"));
        assert_ne!(address, parse("Desk@Host <user@ExAmPlE.OrG>"));
    }

    #[test]
    fn canonical_hash_feed_remains_prefix_free_for_nested_domain_text() {
        let mut feeds: Vec<Vec<u8>> = ["u@x", "u@xy", "u@[x]", "u@[xy]"]
            .into_iter()
            .map(|value| feed(&parse(value)))
            .collect();

        // Unchecked strings remain hashable without being treated as parsed addresses.
        for value in ["", "NoAt", "NoAtSuffix", "Ä"] {
            let unchecked = EmailAddress::new_unchecked(value);
            feeds.push(feed(&unchecked));
            assert_eq!(unchecked.as_str(), value);
        }

        for left in &feeds {
            for right in &feeds {
                if left.len() < right.len() {
                    assert!(!right.starts_with(left));
                }
            }
        }
    }
}
