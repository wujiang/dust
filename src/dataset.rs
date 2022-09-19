use crate::stores::{sqlite::SQLiteStore, store::Store};
use crate::utils;
use anyhow::Result;
use async_fs::File;
use futures::prelude::*;
use serde_json::Value;
use std::{collections::HashSet, slice::Iter};

pub struct Dataset {
    created: u64,
    dataset_id: String,
    hash: String,
    keys: HashSet<String>,
    // Guaranteed to be objects with keys.
    data: Vec<Value>,
}

impl Dataset {
    /// Creates a new Dataset object in memory from raw data (used by Store implementations when
    /// loading datasets).
    pub fn new_from_store(
        created: u64,
        dataset_id: &str,
        hash: &str,
        data: Vec<Value>,
    ) -> Result<Self> {
        let mut keys: Option<HashSet<String>> = None;
        let mut hasher = blake3::Hasher::new();

        data.iter()
            .map(|d| {
                match d.as_object() {
                    Some(obj) => {
                        let record_keys: HashSet<String> = obj.keys().cloned().collect();
                        if let Some(keys) = &keys {
                            assert!(*keys == record_keys);
                        } else {
                            keys = Some(record_keys);
                        }
                    }
                    None => unreachable!(),
                };

                // Reserialize json to hash it.
                hasher.update(serde_json::to_string(&d)?.as_bytes());
                Ok(())
            })
            .collect::<Result<_>>()?;

        let recomputed_hash = format!("{}", hasher.finalize().to_hex());
        assert!(recomputed_hash == hash);
        assert!(keys.is_some());

        Ok(Dataset {
            created,
            dataset_id: dataset_id.to_string(),
            hash: hash.to_string(),
            keys: keys.unwrap(),
            data,
        })
    }

    pub fn created(&self) -> u64 {
        self.created
    }

    pub fn dataset_id(&self) -> &str {
        &self.dataset_id
    }

    pub fn hash(&self) -> &str {
        &self.hash
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn keys(&self) -> Vec<String> {
        self.keys.iter().map(|k| k.clone()).collect::<Vec<_>>()
    }

    pub fn iter(&self) -> Iter<Value> {
        self.data.iter()
    }

    pub async fn from_hash(
        store: &dyn Store,
        dataset_id: &str,
        hash: &str,
    ) -> Result<Option<Self>> {
        store.load_dataset(dataset_id, hash).await
    }

    pub async fn new_from_jsonl(id: &str, jsonl_path: &str) -> Result<Self> {
        let jsonl_path = &shellexpand::tilde(jsonl_path).into_owned();
        let jsonl_path = std::path::Path::new(jsonl_path);

        let file = File::open(jsonl_path).await?;
        let reader = futures::io::BufReader::new(file);

        let mut keys: Option<HashSet<String>> = None;
        let mut hasher = blake3::Hasher::new();

        let data: Vec<Value> = reader
            .lines()
            .enumerate()
            .map(|(line_number, line)| {
                let line = line.unwrap();
                let json: Value = serde_json::from_str(&line)?;

                // Check that json is an Object and its keys match `all_keys`, error otherwise.
                match json.as_object() {
                    Some(obj) => {
                        let record_keys: HashSet<String> = obj.keys().cloned().collect();
                        if let Some(keys) = &keys {
                            if *keys != record_keys {
                                Err(anyhow::anyhow!(
                                    "Line {}: JSON Object has different keys from previous lines.",
                                    line_number
                                ))?;
                            }
                        } else {
                            // This is the first object we've seen, so store its keys.
                            keys = Some(record_keys);
                        }
                    }
                    None => Err(anyhow::anyhow!(
                        "Line {}: Not a JSON object. Only JSON Objects are expected \
                         at each line of the JSONL file.",
                        line_number
                    ))?,
                };

                // Reserialize json to hash it.
                hasher.update(serde_json::to_string(&json)?.as_bytes());

                Ok(json.to_owned())
            })
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>>>()?;

        let hash = format!("{}", hasher.finalize().to_hex());

        Ok(Dataset {
            created: utils::now(),
            dataset_id: String::from(id),
            hash,
            keys: keys.unwrap(),
            data,
        })
    }

    pub fn data_as_value(&self) -> Value {
        self.data
            .iter()
            .map(|r| r.clone())
            .collect::<Vec<_>>()
            .into()
    }
}

pub async fn cmd_register(dataset_id: &str, jsonl_path: &str) -> Result<()> {
    let root_path = utils::init_check().await?;
    let store = SQLiteStore::new(root_path.join("store.sqlite"))?;
    store.init().await?;

    let d = Dataset::new_from_jsonl(dataset_id, jsonl_path).await?;
    store.register_dataset(&d).await?;

    utils::done(&format!(
        "Registered dataset `{}` version ({}) with {} records (record keys: {:?})",
        d.dataset_id(),
        d.hash(),
        d.len(),
        d.keys(),
    ));

    Ok(())
}
