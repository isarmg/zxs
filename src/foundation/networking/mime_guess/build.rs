#[cfg(feature = "rev-mappings")]
extern crate unicase;

#[cfg(feature = "rev-mappings")]
use unicase::UniCase;

use std::env;
use std::fs::File;
use std::io;
#[cfg(feature = "rev-mappings")]
use std::io::prelude::*;
#[cfg(feature = "rev-mappings")]
use std::io::BufWriter;
use std::path::Path;

#[cfg(feature = "rev-mappings")]
use std::collections::BTreeMap;

#[cfg(feature = "rev-mappings")]
use mime_types::MIME_TYPES;

#[cfg(feature = "rev-mappings")]
#[path = "src/mime_types.rs"]
mod mime_types;


fn main() -> io::Result<()> {
    let out_dir = env::var("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("mime_types_generated.rs");
    let output = File::create(&dest_path)?;
    #[cfg(feature = "rev-mappings")]
    let mut outfile = BufWriter::new(output);
    #[cfg(not(feature = "rev-mappings"))]
    drop(output);

    println!(
        "cargo:rustc-env=MIME_TYPES_GENERATED_PATH={}",
        dest_path.display()
    );


    #[cfg(feature = "rev-mappings")]
    build_rev_map(&mut outfile)?;

    Ok(())
}

#[cfg(feature = "rev-mappings")]
fn build_rev_map<W: Write>(out: &mut W) -> io::Result<()> {
    use std::fmt::Write as _;

    macro_rules! unicase_const {
        ($s:expr) => ({
            format_args!("{}({:?})", (if $s.is_ascii() {
                "UniCase::ascii"
            } else {
                "UniCase::unicode"
            }), $s)
        })
    }

    let dyn_map = get_rev_mappings();

    write!(out, "static REV_MAPPINGS: &'static [(UniCase<&'static str>, TopLevelExts)] = &[")?;

    let mut exts = Vec::new();

    for (top, subs) in dyn_map {
        let top_start = exts.len();

        let mut sub_map = String::new();

        for (sub, sub_exts) in subs {
            let sub_start = exts.len();
            exts.extend(sub_exts);
            let sub_end = exts.len();

            write!(
                sub_map,
                "({}, ({}, {})),",
                unicase_const!(sub), sub_start, sub_end
            ).unwrap();
        }

        let top_end = exts.len();

        write!(
            out,
            "({}, TopLevelExts {{ start: {}, end: {}, subs: &[{}] }}),",
            unicase_const!(top), top_start, top_end, sub_map
        )?;
    }

    writeln!(out, "];")?;

    writeln!(out, "const EXTS: &'static [&'static str] = &{:?};", exts)?;

    out.flush()
}

#[cfg(feature = "rev-mappings")]
fn get_rev_mappings(
) -> BTreeMap<UniCase<&'static str>, BTreeMap<UniCase<&'static str>, Vec<&'static str>>> {
    // First, collect all the mime type -> ext mappings)
    let mut dyn_map = BTreeMap::new();
    for &(key, types) in MIME_TYPES {
        for val in types {
            let (top, sub) = split_mime(val);
            dyn_map
                .entry(UniCase::new(top))
                .or_insert_with(BTreeMap::new)
                .entry(UniCase::new(sub))
                .or_insert_with(Vec::new)
                .push(key);
        }
    }
    dyn_map
}

#[cfg(feature = "rev-mappings")]
fn split_mime(mime: &str) -> (&str, &str) {
    let split_idx = mime.find('/').unwrap();
    (&mime[..split_idx], &mime[split_idx + 1..])
}
