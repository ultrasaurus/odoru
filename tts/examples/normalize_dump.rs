//! Throwaway tool: read text from stdin, write `normalizer::normalize()` output to stdout.
//! Usage: cargo run --example normalize_dump < input.txt > output.txt
use std::io::{self, Read, Write};
use tts::f5::normalizer::normalize;

fn main() {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input).unwrap();

    let mut out = io::stdout();
    for line in input.lines() {
        writeln!(out, "{}", normalize(line)).unwrap();
    }
}
