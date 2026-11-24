//! Offline, explicitly fictional interface fixture. No credentials or network.
use relic_app::{config::Config, store::Store};
use serde_json::Value;
use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};
fn main() {
    let destination = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("Pass an empty temporary data directory"),
    );
    assert!(!destination.exists(), "Use a new fixture directory");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned();
    let store = Store::open(Config::local(
        root,
        destination,
        "127.0.0.1:8795".parse().unwrap(),
    ))
    .unwrap();
    let bundle: Value =
        serde_json::from_str(include_str!("../../examples/media-continuations.json")).unwrap();
    store
        .import_job(
            bundle,
            "fixture:media-continuations".into(),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
    let hex = "89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4890000000b49444154789c636000020000050001a5f645400000000049454e44ae426082";
    let bytes = hex
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect();
    store
        .retain_media("telegram:fixture:peer:2", "v1", 0, bytes, "image/png")
        .unwrap();

    println!("Fictional media continuations are ready; no source connection was made.");
}
