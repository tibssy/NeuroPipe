use std::sync::mpsc;
use serde_json::{json, Value};
use crate::zmq_client;

fn query(addr: &str) -> Value {
    match zmq_client::send_cmd(addr, &json!({"command": "get_state"})) {
        Ok(reply) => reply,
        Err(_) => Value::String("unavailable".into()),
    }
}

pub fn status() {
    let (tx, rx) = mpsc::channel();

    let tx1 = tx.clone();
    std::thread::spawn(move || {
        tx1.send(("tts", query(zmq_client::tts_cmd()))).ok();
    });

    let tx2 = tx.clone();
    std::thread::spawn(move || {
        tx2.send(("stt", query(zmq_client::stt_cmd()))).ok();
    });

    let tx3 = tx.clone();
    std::thread::spawn(move || {
        tx3.send(("assistant", query(zmq_client::assistant_cmd()))).ok();
    });

    drop(tx);

    let mut result = json!({});
    for (label, state) in rx {
        result[label] = state;
    }

    println!("{}", serde_json::to_string_pretty(&result).unwrap());
}
