//! Compare the former publication path loop with cached typed paths on one fixture.
use name_policy::{PortablePath, path_key, validate_path};
use std::{collections::BTreeMap, hint::black_box, time::Instant};

struct RawEntry {
    path: String,
    name: String,
    key: String,
}

fn main() {
    println!("entries,rounds,raw_ns,typed_ns,speedup");
    for count in [1_000, 10_000, 100_000] {
        let fixture = (0..count)
            .map(|index| format!("Straße-Café-报告-{index:06}.txt"))
            .collect::<Vec<_>>();
        let mut raw = fixture
            .iter()
            .map(|path| RawEntry {
                path: path.clone(),
                name: path.clone(),
                key: path_key(path),
            })
            .collect::<Vec<_>>();
        let mut typed = fixture
            .iter()
            .map(|path| PortablePath::parse(path).unwrap())
            .collect::<Vec<_>>();
        let rounds = 20;
        let start = Instant::now();
        for _ in 0..rounds {
            for entry in &mut raw {
                let path = entry.path.clone();
                validate_path(&path).unwrap();
                entry.name = path.rsplit('/').next().unwrap().into();
                entry.key = path_key(&path);
                entry.path = path;
            }
            let mut keys = BTreeMap::new();
            for entry in &raw {
                assert!(keys.insert(entry.key.as_str(), ()).is_none());
            }
            black_box(keys);
            raw.sort_by(|a, b| a.key.as_bytes().cmp(b.key.as_bytes()));
        }
        let old = start.elapsed();
        let start = Instant::now();
        for _ in 0..rounds {
            let mut keys = BTreeMap::new();
            for path in &typed {
                assert!(keys.insert(path.key(), ()).is_none());
            }
            black_box(keys);
            typed.sort_by(|a, b| a.key().as_bytes().cmp(b.key().as_bytes()));
        }
        let new = start.elapsed();
        assert!(
            raw.iter()
                .zip(&typed)
                .all(|(raw, path)| raw.path == path.as_str()
                    && raw.name == path.file_name()
                    && raw.key == path.key())
        );
        println!(
            "{count},{rounds},{},{},{:.2}",
            old.as_nanos(),
            new.as_nanos(),
            old.as_secs_f64() / new.as_secs_f64()
        );
    }
}
