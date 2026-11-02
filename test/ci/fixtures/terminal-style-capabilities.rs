#[cfg(feature = "output")]
use terminal_style::ColorChoice;
use terminal_style::{AnsiColor, Style};
#[test]
fn style_protocol_is_canonical() {
    let style = Style::new().bold().fg_color(Some(AnsiColor::Green.into()));
    assert_eq!(format!("{style}ok{style:#}"), "\x1b[1m\x1b[32mok\x1b[0m");
}
#[cfg(feature = "parser")]
#[test]
fn parser_dispatches_split_sequences() {
    use terminal_style::parser::{DefaultCharAccumulator, Params, Parser, Perform};
    #[derive(Default)]
    struct Recorder {
        text: String,
        csi: Vec<u16>,
        osc: Vec<Vec<u8>>,
    }
    impl Perform for Recorder {
        fn print(&mut self, c: char) {
            self.text.push(c);
        }
        fn csi_dispatch(&mut self, params: &Params, _: &[u8], _: bool, action: u8) {
            assert_eq!(action, b'm');
            self.csi.extend(params.iter().flatten());
        }
        fn osc_dispatch(&mut self, params: &[&[u8]], _: bool) {
            self.osc = params.iter().map(|x| x.to_vec()).collect();
        }
    }
    let mut parser = Parser::<DefaultCharAccumulator>::new();
    let mut out = Recorder::default();
    for b in b"a\x1b[1;32mb\x1b]0;title\x07c" {
        parser.advance(&mut out, *b);
    }
    assert_eq!(out.text, "abc");
    assert_eq!(out.csi, vec![1, 32]);
    assert_eq!(out.osc, vec![b"0".to_vec(), b"title".to_vec()]);
}
#[cfg(feature = "parser")]
#[test]
fn parameter_iterator_size_hint_bounds_remaining_groups() {
    use terminal_style::parser::{DefaultCharAccumulator, Params, Parser, Perform};

    #[derive(Default)]
    struct Recorder(usize);

    impl Perform for Recorder {
        fn csi_dispatch(&mut self, params: &Params, _: &[u8], _: bool, _: u8) {
            let mut remaining = params.iter().count();
            let mut iter = params.iter();
            loop {
                let (lower, upper) = iter.size_hint();
                assert!(lower <= remaining, "lower={lower}, remaining={remaining}");
                assert!(upper.is_none_or(|upper| upper >= remaining));
                if iter.next().is_none() {
                    break;
                }
                remaining -= 1;
            }
            self.0 += 1;
        }
    }

    let mut parser = Parser::<DefaultCharAccumulator>::new();
    let mut recorder = Recorder::default();
    for byte in b"\x1b[1:2:3;4;5:6m\x1b[7m\x1b[8:9m" {
        parser.advance(&mut recorder, *byte);
    }
    assert_eq!(recorder.0, 3);
}
#[cfg(feature = "output")]
#[test]
fn output_strips_incrementally_and_shares_global_choice() {
    use std::io::Write;
    use terminal_style::output::{adapter, AutoStream, StripStream};
    let text = "héllo\x1b[31m世界\x1b[0m\n";
    for size in 1..=text.len() {
        let mut out = StripStream::new(Vec::new());
        for part in text.as_bytes().chunks(size) {
            out.write_all(part).unwrap();
        }
        assert_eq!(String::from_utf8(out.into_inner()).unwrap(), "héllo世界\n");
    }
    for choice in [
        ColorChoice::Always,
        ColorChoice::AlwaysAnsi,
        ColorChoice::Never,
    ] {
        choice.write_global();
        assert_eq!(AutoStream::choice(&Vec::new()), choice);
        let mut out = AutoStream::new(Vec::new(), ColorChoice::Auto);
        out.write_all(text.as_bytes()).unwrap();
        assert_eq!(
            String::from_utf8(out.into_inner()).unwrap(),
            if choice == ColorChoice::Never {
                "héllo世界\n"
            } else {
                text
            }
        );
    }
    ColorChoice::Auto.write_global();
    assert_eq!(
        adapter::strip_str("\x1b]8;;https://example.org\x1b\\link\x1b]8;;\x1b\\").to_string(),
        "link"
    );
}
