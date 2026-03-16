use clap::ValueEnum;
use serde::Serialize;

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum OutputFormat {
    Text,
    Json,
    Jsonl,
}

/// Print a value as JSON (compact or pretty).
pub fn print_json<T: Serialize>(val: &T) {
    println!("{}", serde_json::to_string_pretty(val).unwrap());
}

/// Print a value as a single JSON line.
pub fn print_jsonl<T: Serialize>(val: &T) {
    println!("{}", serde_json::to_string(val).unwrap());
}

/// Print a list as a JSON array.
pub fn print_json_list<T: Serialize>(vals: &[T]) {
    println!("{}", serde_json::to_string_pretty(vals).unwrap());
}
