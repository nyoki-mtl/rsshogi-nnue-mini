use serde::Serialize;

use crate::{Error, Result};

pub fn to_vec<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    serde_json_canonicalizer::to_vec(value)
        .map_err(|error| Error::InvalidManifest(error.to_string()))
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        env, fs,
        process::Command,
        time::{SystemTime, UNIX_EPOCH},
    };

    use serde::Serialize;

    use super::to_vec;

    #[derive(Serialize)]
    struct MapFixture {
        metadata: HashMap<String, String>,
    }

    fn fixture(reverse: bool) -> MapFixture {
        let entries = [("zeta", "last"), ("alpha", "first"), ("日本語", "value")];
        let mut metadata = HashMap::new();
        if reverse {
            for (key, value) in entries.into_iter().rev() {
                metadata.insert(key.to_owned(), value.to_owned());
            }
        } else {
            for (key, value) in entries {
                metadata.insert(key.to_owned(), value.to_owned());
            }
        }
        MapFixture { metadata }
    }

    #[test]
    fn object_keys_are_sorted_independent_of_map_insertion_order() {
        assert_eq!(to_vec(&fixture(false)).unwrap(), to_vec(&fixture(true)).unwrap());
    }

    #[test]
    fn non_finite_numbers_are_rejected() {
        assert!(to_vec(&f32::NAN).is_err());
        assert!(to_vec(&f32::INFINITY).is_err());
        assert!(to_vec(&f64::NEG_INFINITY).is_err());
    }

    #[test]
    fn canonical_child() {
        let Some(output) = env::var_os("RSSHOGI_NNUE_CANONICAL_CHILD_OUTPUT") else {
            return;
        };
        let reverse = env::var_os("RSSHOGI_NNUE_CANONICAL_CHILD_REVERSE").is_some();
        fs::write(output, to_vec(&fixture(reverse)).unwrap()).unwrap();
    }

    #[test]
    fn canonical_bytes_match_across_processes() {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let root = env::temp_dir().join(format!("rsnn-canonical-{nonce}"));
        fs::create_dir(&root).unwrap();
        let first = root.join("first.json");
        let second = root.join("second.json");
        for (path, reverse) in [(&first, false), (&second, true)] {
            let mut child = Command::new(env::current_exe().unwrap());
            child
                .arg("--exact")
                .arg("canonical::tests::canonical_child")
                .arg("--nocapture")
                .env("RSSHOGI_NNUE_CANONICAL_CHILD_OUTPUT", path);
            if reverse {
                child.env("RSSHOGI_NNUE_CANONICAL_CHILD_REVERSE", "1");
            }
            assert!(child.status().unwrap().success());
        }
        assert_eq!(fs::read(&first).unwrap(), fs::read(&second).unwrap());
        fs::remove_dir_all(root).unwrap();
    }
}
